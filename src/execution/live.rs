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
            round_shares(opportunity.suggested_size_usdc / opportunity.combined_ask_after_fees);
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

        // Post both orders as a batch
        let post_start = Instant::now();
        let responses = client
            .post_orders(vec![signed_up, signed_down])
            .await
            .map_err(|e| Error::Sdk(format!("failed to post orders: {e}")))?;
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

        // We expect exactly 2 responses (UP, DOWN)
        if responses.len() != 2 {
            return Err(Error::Execution(format!(
                "expected 2 order responses, got {}",
                responses.len()
            )));
        }

        let up_resp = &responses[0];
        let down_resp = &responses[1];

        // Log full response details for debugging
        info!(
            market_id = %opportunity.market_id,
            up_status = ?up_resp.status,
            up_success = up_resp.success,
            up_order = %up_resp.order_id,
            up_making = %up_resp.making_amount,
            up_taking = %up_resp.taking_amount,
            down_status = ?down_resp.status,
            down_success = down_resp.success,
            down_order = %down_resp.order_id,
            down_making = %down_resp.making_amount,
            down_taking = %down_resp.taking_amount,
            "[LIVE] order responses"
        );

        // An order is truly filled only if status is Matched.
        // success=true just means the CLOB accepted the order, not that it filled.
        // FOK orders that can't match will be accepted then immediately canceled.
        let up_filled = matches!(up_resp.status, OrderStatusType::Matched);
        let down_filled = matches!(down_resp.status, OrderStatusType::Matched);

        match (up_filled, down_filled) {
            // Both matched — ideal outcome
            (true, true) => {
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
                error!(
                    market_id = %opportunity.market_id,
                    up_order = %up_resp.order_id,
                    down_status = ?down_resp.status,
                    down_error = ?down_resp.error_msg,
                    "[LIVE] PARTIAL FILL — UP matched, DOWN not. Exposed to UP side."
                );

                Ok(ExecutionResult {
                    market_id: opportunity.market_id.clone(),
                    success: false,
                    message: format!(
                        "partial: UP={} matched, DOWN status={:?}: {}",
                        up_resp.order_id,
                        down_resp.status,
                        down_resp.error_msg.as_deref().unwrap_or("not matched")
                    ),
                })
            }

            (false, true) => {
                error!(
                    market_id = %opportunity.market_id,
                    down_order = %down_resp.order_id,
                    up_status = ?up_resp.status,
                    up_error = ?up_resp.error_msg,
                    "[LIVE] PARTIAL FILL — DOWN matched, UP not. Exposed to DOWN side."
                );

                Ok(ExecutionResult {
                    market_id: opportunity.market_id.clone(),
                    success: false,
                    message: format!(
                        "partial: DOWN={} matched, UP status={:?}: {}",
                        down_resp.order_id,
                        up_resp.status,
                        up_resp.error_msg.as_deref().unwrap_or("not matched")
                    ),
                })
            }

            // Neither matched — no exposure.
            // Distinguish expected (Canceled/Unmatched = book moved, normal)
            // from unexpected (Live = resting order we didn't intend,
            // Delayed/Unknown = something wrong with the CLOB).
            (false, false) => {
                let up_unexpected = is_unexpected_status(&up_resp.status);
                let down_unexpected = is_unexpected_status(&down_resp.status);

                if up_unexpected || down_unexpected {
                    warn!(
                        market_id = %opportunity.market_id,
                        up_status = ?up_resp.status,
                        down_status = ?down_resp.status,
                        "[LIVE] unexpected order status — FOK should not rest or delay"
                    );
                } else {
                    info!(
                        market_id = %opportunity.market_id,
                        up_status = ?up_resp.status,
                        down_status = ?down_resp.status,
                        "[LIVE] neither order matched (book moved, FOK killed)"
                    );
                }

                Ok(ExecutionResult {
                    market_id: opportunity.market_id.clone(),
                    success: false,
                    message: format!(
                        "no fill: UP={:?} DOWN={:?}",
                        up_resp.status, down_resp.status
                    ),
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

/// A FOK order status is "unexpected" if the order is resting or delayed
/// instead of being immediately filled or killed.
fn is_unexpected_status(status: &OrderStatusType) -> bool {
    matches!(
        status,
        OrderStatusType::Live | OrderStatusType::Delayed | OrderStatusType::Unknown(_)
    )
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

/// Round share size to 2 decimal places (CLOB lot size constraint).
fn round_shares(shares: Decimal) -> Decimal {
    shares.round_dp(2)
}
