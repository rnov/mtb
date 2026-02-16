use std::time::Duration;

use rust_decimal::Decimal;

use crate::config::types::StrategyConfig;
use crate::feed::price_state::PriceState;
use crate::strategy::types::Opportunity;

/// Reason an arb was rejected by the freshness guard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FreshnessReject {
    /// Not enough exchanges have fresh price data.
    InsufficientSources { have: usize, need: usize },
    /// Exchange prices are too stale.
    StalePrice { asset: String, age_ms: u128 },
    /// Exchanges disagree on price (possible rapid movement / phantom arb).
    PriceDivergence {
        asset: String,
        divergence_pct: Decimal,
        max_pct: Decimal,
    },
}

impl std::fmt::Display for FreshnessReject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FreshnessReject::InsufficientSources { have, need } => {
                write!(f, "insufficient fresh sources: have {have}, need {need}")
            }
            FreshnessReject::StalePrice { asset, age_ms } => {
                write!(f, "stale price for {asset}: {age_ms}ms old")
            }
            FreshnessReject::PriceDivergence {
                asset,
                divergence_pct,
                max_pct,
            } => {
                write!(
                    f,
                    "price divergence for {asset}: {divergence_pct}% (max {max_pct}%)"
                )
            }
        }
    }
}

/// Validate that an arb opportunity is backed by fresh, agreeing exchange prices.
///
/// Only uses direct exchange feeds (Binance, Coinbase, OKX, Bybit) for validation.
/// Chainlink is excluded — it's the resolution source with ~1.5s relay latency,
/// which would cause false divergence rejections during fast price moves and
/// weak validation when counted as a "fresh source."
///
/// Returns `Ok(())` if the arb passes all freshness checks, or `Err(reason)` if rejected.
/// This is pure logic — no IO, no async.
pub fn validate_freshness(
    opportunity: &Opportunity,
    price_state: &PriceState,
    config: &StrategyConfig,
) -> Result<(), FreshnessReject> {
    let asset = &opportunity.asset;
    let max_age = Duration::from_millis(config.max_price_age_ms);

    // Check 1: Do we have enough fresh direct exchange sources?
    let fresh_entries = price_state.count_fresh_exchange_sources(asset, max_age);
    if fresh_entries < config.min_fresh_sources {
        return Err(FreshnessReject::InsufficientSources {
            have: fresh_entries,
            need: config.min_fresh_sources,
        });
    }

    // Check 2: Is the freshest exchange price recent enough?
    if let Some(staleness) = price_state.exchange_staleness(asset) {
        if staleness > max_age {
            return Err(FreshnessReject::StalePrice {
                asset: asset.clone(),
                age_ms: staleness.as_millis(),
            });
        }
    } else {
        // No exchange price data at all
        return Err(FreshnessReject::InsufficientSources {
            have: 0,
            need: config.min_fresh_sources,
        });
    }

    // Check 3: Do the direct exchanges agree on price?
    if !price_state.exchanges_agree(asset, config.max_divergence_pct, config.min_fresh_sources) {
        let divergence = price_state
            .exchange_max_divergence_pct(asset)
            .unwrap_or(Decimal::ZERO);
        return Err(FreshnessReject::PriceDivergence {
            asset: asset.clone(),
            divergence_pct: divergence,
            max_pct: config.max_divergence_pct,
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feed::price_state::PriceState;
    use chrono::Utc;
    use rust_decimal_macros::dec;
    use std::thread;

    fn test_config() -> StrategyConfig {
        StrategyConfig {
            min_arb_profit: dec!(0.005),
            assets: vec!["BTC".into()],
            max_order_size_usdc: dec!(100),
            min_fresh_sources: 2,
            max_price_age_ms: 2000,
            max_divergence_pct: dec!(0.5),
        }
    }

    fn test_opportunity(asset: &str) -> Opportunity {
        Opportunity {
            market_id: "test-market".into(),
            asset: asset.into(),
            question: "test".into(),
            kind: crate::strategy::types::OpportunityKind::Arb,
            combined_ask: dec!(0.96),
            combined_ask_after_fees: dec!(0.98),
            profit_per_unit: dec!(0.02),
            suggested_size_usdc: dec!(50),
            expected_profit_usdc: dec!(1.0),
            up_token_id: "up-123".into(),
            down_token_id: "down-456".into(),
            up_ask: dec!(0.48),
            down_ask: dec!(0.48),
            up_fee: dec!(0.0096),
            down_fee: dec!(0.0096),
            timestamp: Utc::now(),
        }
    }

    #[test]
    fn passes_when_sources_agree() {
        let mut state = PriceState::new();
        state.update("binance", "btcusdt", dec!(69000), 1000);
        state.update("coinbase", "BTC-USD", dec!(69010), 1000);

        let opp = test_opportunity("BTC");
        let result = validate_freshness(&opp, &state, &test_config());
        assert!(result.is_ok());
    }

    #[test]
    fn rejects_insufficient_sources() {
        let mut state = PriceState::new();
        // Only 1 source, need 2
        state.update("binance", "btcusdt", dec!(69000), 1000);

        let opp = test_opportunity("BTC");
        let result = validate_freshness(&opp, &state, &test_config());
        assert!(matches!(
            result,
            Err(FreshnessReject::InsufficientSources { have: 1, need: 2 })
        ));
    }

    #[test]
    fn rejects_no_data() {
        let state = PriceState::new();
        let opp = test_opportunity("BTC");
        let result = validate_freshness(&opp, &state, &test_config());
        assert!(matches!(
            result,
            Err(FreshnessReject::InsufficientSources { have: 0, .. })
        ));
    }

    #[test]
    fn rejects_stale_prices() {
        let mut state = PriceState::new();
        state.update("binance", "btcusdt", dec!(69000), 1000);
        state.update("coinbase", "BTC-USD", dec!(69010), 1000);

        // Wait for prices to go stale
        let config = StrategyConfig {
            max_price_age_ms: 50, // 50ms — will be stale after sleep
            ..test_config()
        };

        thread::sleep(Duration::from_millis(60));

        let opp = test_opportunity("BTC");
        let result = validate_freshness(&opp, &state, &config);
        // Stale prices → 0 fresh sources → InsufficientSources
        assert!(matches!(
            result,
            Err(FreshnessReject::InsufficientSources { have: 0, need: 2 })
        ));
    }

    #[test]
    fn rejects_partially_stale() {
        let mut state = PriceState::new();
        // One fresh, one that will go stale
        state.update("binance", "btcusdt", dec!(69000), 1000);

        let config = StrategyConfig {
            max_price_age_ms: 50,
            min_fresh_sources: 2,
            ..test_config()
        };

        thread::sleep(Duration::from_millis(60));

        // Add a fresh source after sleep — now 1 fresh, 1 stale
        state.update("coinbase", "BTC-USD", dec!(69010), 1000);

        let opp = test_opportunity("BTC");
        let result = validate_freshness(&opp, &state, &config);
        // Only 1 fresh source, need 2
        assert!(matches!(
            result,
            Err(FreshnessReject::InsufficientSources { have: 1, need: 2 })
        ));
    }

    #[test]
    fn rejects_price_divergence() {
        let mut state = PriceState::new();
        state.update("binance", "btcusdt", dec!(69000), 1000);
        state.update("coinbase", "BTC-USD", dec!(70000), 1000); // 1.4% divergence

        let config = StrategyConfig {
            max_divergence_pct: dec!(0.5), // 0.5% tolerance
            ..test_config()
        };

        let opp = test_opportunity("BTC");
        let result = validate_freshness(&opp, &state, &config);
        assert!(matches!(
            result,
            Err(FreshnessReject::PriceDivergence { .. })
        ));
    }

    #[test]
    fn passes_with_tight_but_agreeing_prices() {
        let mut state = PriceState::new();
        // 0.029% divergence — within 0.5% tolerance
        state.update("binance", "btcusdt", dec!(69000), 1000);
        state.update("coinbase", "BTC-USD", dec!(69020), 1000);
        state.update("okx", "BTC-USDT", dec!(68990), 1000);

        let opp = test_opportunity("BTC");
        let result = validate_freshness(&opp, &state, &test_config());
        assert!(result.is_ok());
    }

    #[test]
    fn unknown_asset_rejected() {
        let state = PriceState::new();
        let opp = test_opportunity("DOGE");
        let result = validate_freshness(&opp, &state, &test_config());
        assert!(matches!(
            result,
            Err(FreshnessReject::InsufficientSources { .. })
        ));
    }
}
