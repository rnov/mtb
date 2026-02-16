use std::str::FromStr;
use std::time::Duration;

use alloy::signers::local::LocalSigner;
use alloy::signers::Signer as _;
use async_trait::async_trait;
use rust_decimal::Decimal;
use tracing::{error, info, warn};

use polymarket_client_sdk::clob::types::{OrderStatusType, OrderType, Side, SignatureType};
use polymarket_client_sdk::clob::{Client, Config};
use polymarket_client_sdk::types::U256;

use crate::error::{Error, Result};
use crate::strategy::Opportunity;

use super::{ExecutionResult, Executor};

/// Maximum retries for transient failures.
const MAX_RETRIES: u32 = 3;

/// Base delay between retries (doubles each attempt).
const RETRY_BASE_DELAY: Duration = Duration::from_millis(200);

/// Overall timeout for the entire execute cycle.
const EXECUTION_TIMEOUT: Duration = Duration::from_secs(10);

/// Live executor: places real orders on Polymarket CLOB.
pub struct LiveExecutor {
    clob_url: String,
    chain_id: u64,
    private_key: String,
    signature_type: SignatureType,
}

impl LiveExecutor {
    pub fn new(clob_url: String, chain_id: u64, private_key: String) -> Self {
        Self {
            clob_url,
            chain_id,
            private_key,
            // GnosisSafe = browser wallet (MetaMask) login; uses proxy wallet
            signature_type: SignatureType::GnosisSafe,
        }
    }

    /// Inner execution logic, called by the retry wrapper.
    async fn execute_inner(&self, opportunity: &Opportunity) -> Result<ExecutionResult> {
        use std::time::Instant;

        // Build signer from private key (strip 0x prefix if present)
        let key = self
            .private_key
            .strip_prefix("0x")
            .unwrap_or(&self.private_key);
        let signer = LocalSigner::from_str(key)
            .map_err(|e| Error::Execution(format!("invalid private key: {e}")))?
            .with_chain_id(Some(self.chain_id));

        // Authenticate with CLOB (GnosisSafe = browser wallet proxy)
        let auth_start = Instant::now();
        let client = Client::new(&self.clob_url, Config::default())
            .map_err(|e| Error::Sdk(format!("failed to create CLOB client: {e}")))?
            .authentication_builder(&signer)
            .signature_type(self.signature_type)
            .authenticate()
            .await
            .map_err(|e| Error::Sdk(format!("authentication failed: {e}")))?;
        let auth_ms = auth_start.elapsed().as_millis();

        // Parse token IDs from strings to U256
        let up_token_id = U256::from_str(&opportunity.up_token_id)
            .map_err(|e| Error::Execution(format!("invalid up token ID: {e}")))?;
        let down_token_id = U256::from_str(&opportunity.down_token_id)
            .map_err(|e| Error::Execution(format!("invalid down token ID: {e}")))?;

        // For parity arb, we need EQUAL shares on both sides:
        // 1 Up + 1 Down = $1 payout. suggested_size_usdc is the total
        // budget, so units = budget / combined_cost_after_fees.
        let units =
            floor_shares(opportunity.suggested_size_usdc / opportunity.combined_ask_after_fees);
        let up_shares = units;
        let down_shares = units;

        // Build UP order: buy up_token at the ask price (FOK = fill-or-kill)
        let build_start = Instant::now();
        let up_order = client
            .limit_order()
            .token_id(up_token_id)
            .price(opportunity.up_ask)
            .size(up_shares)
            .side(Side::Buy)
            .order_type(OrderType::FOK)
            .build()
            .await
            .map_err(|e| Error::Sdk(format!("failed to build UP order: {e}")))?;

        // Build DOWN order: buy down_token at the ask price
        let down_order = client
            .limit_order()
            .token_id(down_token_id)
            .price(opportunity.down_ask)
            .size(down_shares)
            .side(Side::Buy)
            .order_type(OrderType::FOK)
            .build()
            .await
            .map_err(|e| Error::Sdk(format!("failed to build DOWN order: {e}")))?;
        let build_ms = build_start.elapsed().as_millis();

        // Sign both orders
        let sign_start = Instant::now();
        let signed_up = client
            .sign(&signer, up_order)
            .await
            .map_err(|e| Error::Sdk(format!("failed to sign UP order: {e}")))?;
        let signed_down = client
            .sign(&signer, down_order)
            .await
            .map_err(|e| Error::Sdk(format!("failed to sign DOWN order: {e}")))?;
        let sign_ms = sign_start.elapsed().as_millis();

        // Post both orders concurrently via individual endpoints.
        // The batch POST /orders endpoint doesn't return status/order_id for FOK.
        // Individual POST /order returns proper responses, and tokio::join! fires
        // both requests simultaneously for minimal timing gap.
        let post_start = Instant::now();
        let (up_result, down_result) = tokio::join!(
            client.post_order(signed_up),
            client.post_order(signed_down),
        );
        let post_ms = post_start.elapsed().as_millis();

        info!(
            market_id = %opportunity.market_id,
            units = %units,
            up_price = %opportunity.up_ask,
            down_price = %opportunity.down_ask,
            auth_ms = auth_ms,
            build_ms = build_ms,
            sign_ms = sign_ms,
            post_ms = post_ms,
            "latency: execution breakdown"
        );

        // For FOK orders via POST /order:
        // - Ok(response) with status Matched = filled
        // - Err containing "FOK" / "fully filled" = FOK killed, not filled (normal)
        // - Err other = actual problem
        let up_filled = match &up_result {
            Ok(resp) => {
                info!(
                    market_id = %opportunity.market_id,
                    status = ?resp.status,
                    order_id = %resp.order_id,
                    making = %resp.making_amount,
                    taking = %resp.taking_amount,
                    "[LIVE] UP order response"
                );
                matches!(resp.status, OrderStatusType::Matched)
            }
            Err(e) => {
                let msg = e.to_string();
                if is_fok_rejection(&msg) {
                    info!(
                        market_id = %opportunity.market_id,
                        "[LIVE] UP order FOK killed (no liquidity at price)"
                    );
                } else {
                    error!(
                        market_id = %opportunity.market_id,
                        error = %e,
                        "[LIVE] UP order error"
                    );
                }
                false
            }
        };

        let down_filled = match &down_result {
            Ok(resp) => {
                info!(
                    market_id = %opportunity.market_id,
                    status = ?resp.status,
                    order_id = %resp.order_id,
                    making = %resp.making_amount,
                    taking = %resp.taking_amount,
                    "[LIVE] DOWN order response"
                );
                matches!(resp.status, OrderStatusType::Matched)
            }
            Err(e) => {
                let msg = e.to_string();
                if is_fok_rejection(&msg) {
                    info!(
                        market_id = %opportunity.market_id,
                        "[LIVE] DOWN order FOK killed (no liquidity at price)"
                    );
                } else {
                    error!(
                        market_id = %opportunity.market_id,
                        error = %e,
                        "[LIVE] DOWN order error"
                    );
                }
                false
            }
        };

        match (up_filled, down_filled) {
            // Both matched — ideal outcome
            (true, true) => {
                let up_resp = up_result.unwrap();
                let down_resp = down_result.unwrap();
                info!(
                    market_id = %opportunity.market_id,
                    up_order = %up_resp.order_id,
                    down_order = %down_resp.order_id,
                    "[LIVE] both orders MATCHED"
                );

                Ok(ExecutionResult {
                    market_id: opportunity.market_id.clone(),
                    success: true,
                    message: format!(
                        "matched: UP={} DOWN={}",
                        up_resp.order_id, down_resp.order_id
                    ),
                })
            }

            // Partial fill: one matched, the other didn't.
            // The matched side leaves us with directional exposure.
            (true, false) => {
                let up_resp = up_result.unwrap();
                let down_reason = match &down_result {
                    Ok(r) => format!("status={:?}", r.status),
                    Err(e) => format!("error={e}"),
                };
                error!(
                    market_id = %opportunity.market_id,
                    up_order = %up_resp.order_id,
                    down = %down_reason,
                    "[LIVE] PARTIAL FILL — UP matched, DOWN not. Exposed to UP side."
                );

                Ok(ExecutionResult {
                    market_id: opportunity.market_id.clone(),
                    success: false,
                    message: format!(
                        "partial: UP={} matched, DOWN {down_reason}",
                        up_resp.order_id,
                    ),
                })
            }

            (false, true) => {
                let down_resp = down_result.unwrap();
                let up_reason = match &up_result {
                    Ok(r) => format!("status={:?}", r.status),
                    Err(e) => format!("error={e}"),
                };
                error!(
                    market_id = %opportunity.market_id,
                    down_order = %down_resp.order_id,
                    up = %up_reason,
                    "[LIVE] PARTIAL FILL — DOWN matched, UP not. Exposed to DOWN side."
                );

                Ok(ExecutionResult {
                    market_id: opportunity.market_id.clone(),
                    success: false,
                    message: format!(
                        "partial: DOWN={} matched, UP {up_reason}",
                        down_resp.order_id,
                    ),
                })
            }

            // Neither matched — no exposure, normal for FOK when book moved.
            (false, false) => {
                let up_reason = match &up_result {
                    Ok(r) => format!("{:?}", r.status),
                    Err(e) => {
                        let msg = e.to_string();
                        if is_fok_rejection(&msg) {
                            "FOK killed".to_string()
                        } else {
                            format!("error: {msg}")
                        }
                    }
                };
                let down_reason = match &down_result {
                    Ok(r) => format!("{:?}", r.status),
                    Err(e) => {
                        let msg = e.to_string();
                        if is_fok_rejection(&msg) {
                            "FOK killed".to_string()
                        } else {
                            format!("error: {msg}")
                        }
                    }
                };

                // Check if any result is a non-FOK error (unexpected)
                let up_unexpected = matches!(&up_result, Err(e) if !is_fok_rejection(&e.to_string()));
                let down_unexpected = matches!(&down_result, Err(e) if !is_fok_rejection(&e.to_string()));

                if up_unexpected || down_unexpected {
                    warn!(
                        market_id = %opportunity.market_id,
                        up = %up_reason,
                        down = %down_reason,
                        "[LIVE] unexpected order error"
                    );
                } else {
                    info!(
                        market_id = %opportunity.market_id,
                        up = %up_reason,
                        down = %down_reason,
                        "[LIVE] neither order matched (book moved, FOK killed)"
                    );
                }

                Ok(ExecutionResult {
                    market_id: opportunity.market_id.clone(),
                    success: false,
                    message: format!("no fill: UP={up_reason} DOWN={down_reason}"),
                })
            }
        }
    }
}

#[async_trait]
impl Executor for LiveExecutor {
    async fn execute(&self, opportunity: &Opportunity) -> Result<ExecutionResult> {
        info!(
            market_id = %opportunity.market_id,
            asset = %opportunity.asset,
            profit = %opportunity.expected_profit_usdc,
            size = %opportunity.suggested_size_usdc,
            "[LIVE] executing arb"
        );

        // Wrap the entire execution in a timeout
        let result = tokio::time::timeout(EXECUTION_TIMEOUT, async {
            let mut last_err = None;

            for attempt in 0..=MAX_RETRIES {
                if attempt > 0 {
                    let delay = RETRY_BASE_DELAY * 2u32.pow(attempt - 1);
                    warn!(
                        attempt = attempt,
                        delay_ms = delay.as_millis() as u64,
                        "[LIVE] retrying after transient failure"
                    );
                    tokio::time::sleep(delay).await;
                }

                match self.execute_inner(opportunity).await {
                    Ok(result) => return Ok(result),
                    Err(e) if is_transient(&e) => {
                        warn!(
                            attempt = attempt,
                            error = %e,
                            "[LIVE] transient error, will retry"
                        );
                        last_err = Some(e);
                    }
                    Err(e) => {
                        // Permanent error — don't retry
                        error!(error = %e, "[LIVE] permanent execution error");
                        return Err(e);
                    }
                }
            }

            // All retries exhausted
            Err(last_err.unwrap_or_else(|| Error::Execution("all retries exhausted".into())))
        })
        .await;

        match result {
            Ok(inner) => inner,
            Err(_elapsed) => {
                error!(
                    market_id = %opportunity.market_id,
                    timeout_secs = EXECUTION_TIMEOUT.as_secs(),
                    "[LIVE] execution timed out"
                );
                Err(Error::Execution(format!(
                    "execution timed out after {}s",
                    EXECUTION_TIMEOUT.as_secs()
                )))
            }
        }
    }
}

/// FOK rejection: the CLOB returns an error (not a response) when a FOK
/// order can't be fully filled. This is normal — the book moved.
fn is_fok_rejection(msg: &str) -> bool {
    let lower = msg.to_lowercase();
    lower.contains("fok") || lower.contains("fully filled") || lower.contains("not filled")
}

/// Determine if an error is transient (worth retrying).
fn is_transient(err: &Error) -> bool {
    let msg = err.to_string().to_lowercase();
    // Network errors, rate limits, temporary server issues
    msg.contains("timeout")
        || msg.contains("connection")
        || msg.contains("429")
        || msg.contains("rate limit")
        || msg.contains("503")
        || msg.contains("502")
        || msg.contains("504")
        || msg.contains("temporarily")
}

/// Floor share size to whole number.
/// The CLOB requires maker_amount (price × size) to have ≤2 decimal places.
/// Since prices are 2dp, integer shares guarantee this: 0.45 × 3 = 1.35 (2dp).
/// Fractional shares like 3.03 would produce 0.45 × 3.03 = 1.3635 (4dp) → rejected.
fn floor_shares(shares: Decimal) -> Decimal {
    shares.floor()
}
