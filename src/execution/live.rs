use std::str::FromStr;
use std::time::Duration;

use alloy::signers::local::PrivateKeySigner;
use alloy::signers::Signer as _;
use async_trait::async_trait;
use rust_decimal::Decimal;
use tracing::{error, info, warn};

use polymarket_client_sdk::auth::state::Authenticated;
use polymarket_client_sdk::auth::Normal;
use polymarket_client_sdk::clob::types::{OrderStatusType, OrderType, Side, SignatureType};
use polymarket_client_sdk::clob::{Client, Config};
use polymarket_client_sdk::types::U256;

use crate::error::{Error, Result};
use crate::market::MarketPair;
use crate::strategy::Opportunity;

use super::{ExecutionResult, Executor};

/// Maximum retries for transient failures.
const MAX_RETRIES: u32 = 3;

/// Base delay between retries (doubles each attempt).
const RETRY_BASE_DELAY: Duration = Duration::from_millis(200);

/// Overall timeout for the entire execute cycle.
const EXECUTION_TIMEOUT: Duration = Duration::from_secs(10);

/// Live executor: places real orders on Polymarket CLOB.
///
/// The CLOB client is authenticated once at construction time and reused for
/// every execution — auth is not repeated per-arb or per-retry.
pub struct LiveExecutor {
    client: Client<Authenticated<Normal>>,
    signer: PrivateKeySigner,
}

impl LiveExecutor {
    /// Authenticate with the CLOB once and return a ready executor.
    pub async fn new(clob_url: String, chain_id: u64, private_key: String) -> Result<Self> {
        let key = private_key.strip_prefix("0x").unwrap_or(&private_key);
        let signer = PrivateKeySigner::from_str(key)
            .map_err(|e| Error::Execution(format!("invalid private key: {e}")))?
            .with_chain_id(Some(chain_id));

        let client = Client::new(&clob_url, Config::default())
            .map_err(|e| Error::Sdk(format!("failed to create CLOB client: {e}")))?
            .authentication_builder(&signer)
            .signature_type(SignatureType::GnosisSafe)
            .authenticate()
            .await
            .map_err(|e| Error::Sdk(format!("authentication failed: {e}")))?;

        info!("CLOB client authenticated");
        Ok(Self { client, signer })
    }

    /// Inner execution logic, called by the retry wrapper.
    async fn execute_inner(&self, opportunity: &Opportunity) -> Result<ExecutionResult> {
        use std::time::Instant;

        // Parse token IDs from strings to U256
        let up_token_id = U256::from_str(&opportunity.up_token_id)
            .map_err(|e| Error::Execution(format!("invalid up token ID: {e}")))?;
        let down_token_id = U256::from_str(&opportunity.down_token_id)
            .map_err(|e| Error::Execution(format!("invalid down token ID: {e}")))?;

        let units = floor_shares(opportunity.units);

        // Build UP and DOWN orders concurrently — two async HTTP calls in parallel.
        let build_start = Instant::now();
        let (up_order_res, down_order_res) = tokio::join!(
            self.client
                .limit_order()
                .token_id(up_token_id)
                .price(opportunity.up_ask)
                .size(units)
                .side(Side::Buy)
                .order_type(OrderType::FOK)
                .build(),
            self.client
                .limit_order()
                .token_id(down_token_id)
                .price(opportunity.down_ask)
                .size(units)
                .side(Side::Buy)
                .order_type(OrderType::FOK)
                .build(),
        );
        let up_order = up_order_res
            .map_err(|e| Error::Sdk(format!("failed to build UP order: {e}")))?;
        let down_order = down_order_res
            .map_err(|e| Error::Sdk(format!("failed to build DOWN order: {e}")))?;
        let build_ms = build_start.elapsed().as_millis();

        // Sign both orders concurrently — pure local ECDSA, but may involve async SDK calls.
        let sign_start = Instant::now();
        let (signed_up_res, signed_down_res) = tokio::join!(
            self.client.sign(&self.signer, up_order),
            self.client.sign(&self.signer, down_order),
        );
        let signed_up = signed_up_res
            .map_err(|e| Error::Sdk(format!("failed to sign UP order: {e}")))?;
        let signed_down = signed_down_res
            .map_err(|e| Error::Sdk(format!("failed to sign DOWN order: {e}")))?;
        let sign_ms = sign_start.elapsed().as_millis();

        // Post both orders concurrently via individual endpoints.
        // The batch POST /orders endpoint doesn't return status/order_id for FOK.
        // Individual POST /order returns proper responses, and tokio::join! fires
        // both requests simultaneously for minimal timing gap.
        let post_start = Instant::now();
        let ((up_result, up_post_ms), (down_result, down_post_ms)) = tokio::join!(
            async {
                let t = Instant::now();
                let r = self.client.post_order(signed_up).await;
                (r, t.elapsed().as_millis())
            },
            async {
                let t = Instant::now();
                let r = self.client.post_order(signed_down).await;
                (r, t.elapsed().as_millis())
            },
        );
        let post_ms = post_start.elapsed().as_millis();

        info!(
            market_id = %opportunity.market_id,
            units = %units,
            up_price = %opportunity.up_ask,
            down_price = %opportunity.down_ask,
            build_ms = build_ms,
            sign_ms = sign_ms,
            up_post_ms = up_post_ms,
            down_post_ms = down_post_ms,
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
    /// Pre-warm fee_rate + tick_size caches for every token in the market list.
    ///
    /// Calls `tick_size()` and `fee_rate_bps()` directly — the two HTTP endpoints that
    /// `build()` would otherwise hit on first use. Both results are cached by token ID
    /// in the SDK's internal DashMap, so subsequent `build()` calls in `execute_inner`
    /// are pure cache hits with zero HTTP.
    async fn warm_cache(&self, markets: &[MarketPair]) {
        for m in markets {
            let up_token = match U256::from_str(&m.up_token_id) {
                Ok(t) => t,
                Err(e) => { warn!(asset = %m.asset, error = %e, "warm_cache: invalid up token ID"); continue; }
            };
            let down_token = match U256::from_str(&m.down_token_id) {
                Ok(t) => t,
                Err(e) => { warn!(asset = %m.asset, error = %e, "warm_cache: invalid down token ID"); continue; }
            };

            // Fetch tick_size + fee_rate_bps for both legs concurrently.
            // These are the exact two HTTP calls build() makes per token — pulling them
            // here means the hot execution path never touches CLOB for this data.
            tokio::join!(
                async { let _ = self.client.tick_size(up_token).await; },
                async { let _ = self.client.fee_rate_bps(up_token).await; },
                async { let _ = self.client.tick_size(down_token).await; },
                async { let _ = self.client.fee_rate_bps(down_token).await; },
            );

            info!(asset = %m.asset, market_id = %m.market_id, "SDK cache warmed");
        }
    }

    async fn execute(&self, opportunity: &Opportunity) -> Result<ExecutionResult> {
        info!(
            market_id = %opportunity.market_id,
            asset = %opportunity.asset,
            profit = %opportunity.expected_profit_usdc,
            size = %opportunity.total_cost_usdc,
            "[LIVE] executing arb — client already authenticated"
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

/// Startup probe: confirm the CLOB's minimum notional per FOK taker order matches config.
///
/// Posts 1 share at $0.01 (notional $0.01). The CLOB rejects it before the matching engine:
///   "invalid amount for a marketable BUY order ($0.01), min size: $1"
/// Parses the reported minimum and compares it to `configured_min`. Aborts if they differ.
///
/// Cost: $0 — rejected before the matching engine.
pub async fn verify_clob_minimum(
    clob_url: &str,
    chain_id: u64,
    private_key: &str,
    asset: &str,
    token_id: &str,
    configured_min: Decimal,
) -> crate::error::Result<()> {
    info!(
        asset = %asset,
        token = %&token_id[..token_id.len().min(16)],
        configured_min = %configured_min,
        "probing CLOB minimum order size"
    );
    let key = private_key.strip_prefix("0x").unwrap_or(private_key);
    let signer = PrivateKeySigner::from_str(key)
        .map_err(|e| Error::Execution(format!("invalid private key: {e}")))?
        .with_chain_id(Some(chain_id));

    let client = Client::new(clob_url, Config::default())
        .map_err(|e| Error::Sdk(format!("failed to create CLOB client: {e}")))?
        .authentication_builder(&signer)
        .signature_type(SignatureType::GnosisSafe)
        .authenticate()
        .await
        .map_err(|e| Error::Sdk(format!("authentication failed: {e}")))?;

    let token = U256::from_str(token_id)
        .map_err(|e| Error::Execution(format!("invalid token ID: {e}")))?;

    let order = client
        .limit_order()
        .token_id(token)
        .price(Decimal::new(1, 2)) // $0.01
        .size(Decimal::ONE)         // 1 share → $0.01 notional, clearly sub-$1
        .side(Side::Buy)
        .order_type(OrderType::FOK)
        .build()
        .await
        .map_err(|e| Error::Sdk(format!("failed to build probe order: {e}")))?;

    let signed = client
        .sign(&signer, order)
        .await
        .map_err(|e| Error::Sdk(format!("failed to sign probe order: {e}")))?;

    match client.post_order(signed).await {
        Err(e) => {
            let msg = e.to_string();
            if let Some(clob_min) = parse_min_size(&msg) {
                if clob_min == configured_min {
                    info!(confirmed = %clob_min, "CLOB minimum order size confirmed");
                    Ok(())
                } else {
                    Err(Error::Execution(format!(
                        "CLOB minimum order size mismatch: CLOB requires ${clob_min} \
                         but config has min_order_usdc=${configured_min} — \
                         update config.toml before going live"
                    )))
                }
            } else if is_fok_rejection(&msg) {
                // $0.01 somehow reached the matching engine — minimum is below $0.01
                warn!(
                    configured = %configured_min,
                    "CLOB minimum probe: sub-$0.01 order reached matching engine — \
                     config min_order_usdc may be overly conservative"
                );
                Ok(())
            } else {
                // Network error, auth issue, etc. — warn but don't abort startup
                warn!(error = %e, "CLOB minimum probe returned unexpected error — skipping check");
                Ok(())
            }
        }
        Ok(_) => {
            // Very unexpected: a $0.01 order filled
            warn!("CLOB minimum probe order filled at $0.01 — check for unexpected position");
            Ok(())
        }
    }
}

/// Extract the minimum dollar amount from a CLOB min-size rejection.
/// Expected format: "invalid amount for a marketable BUY order ($0.01), min size: $1"
fn parse_min_size(msg: &str) -> Option<Decimal> {
    let marker = "min size: $";
    let start = msg.find(marker)? + marker.len();
    let end = msg[start..]
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(msg[start..].len());
    msg[start..start + end].parse().ok()
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
