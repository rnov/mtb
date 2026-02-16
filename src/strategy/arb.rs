use rust_decimal::Decimal;
use tracing::{debug, info};

use crate::config::types::StrategyConfig;
use crate::strategy::fees;
use crate::strategy::types::{MarketSnapshot, Opportunity, OpportunityKind};

/// Pure arb strategy: detect when up_ask + down_ask < 1.0 after fees.
///
/// This function is pure computation — no IO, no async. Fully unit-testable.
pub fn detect_arb(snapshot: &MarketSnapshot, config: &StrategyConfig) -> Option<Opportunity> {
    let up_ask = snapshot.up_best_ask?;
    let down_ask = snapshot.down_best_ask?;

    // Sanity: prices must be in (0, 1)
    if up_ask <= Decimal::ZERO
        || up_ask >= Decimal::ONE
        || down_ask <= Decimal::ZERO
        || down_ask >= Decimal::ONE
    {
        debug!(
            market_id = %snapshot.market.market_id,
            up_ask = %up_ask,
            down_ask = %down_ask,
            "skipping: prices out of range"
        );
        return None;
    }

    // Cap individual leg prices to limit directional exposure on partial fill.
    // E.g., up=0.06 / down=0.93 is a valid arb but if down fails to fill,
    // you hold a $0.06 token that's almost certainly worthless.
    if up_ask > config.max_leg_price || down_ask > config.max_leg_price {
        debug!(
            market_id = %snapshot.market.market_id,
            up_ask = %up_ask,
            down_ask = %down_ask,
            max = %config.max_leg_price,
            "skipping: leg price exceeds max"
        );
        return None;
    }

    let (combined_after_fees, up_fee, down_fee) = fees::combined_cost_after_fees(up_ask, down_ask);
    let combined_raw = up_ask + down_ask;
    let profit_per_unit = Decimal::ONE - combined_after_fees;

    if profit_per_unit <= Decimal::ZERO {
        debug!(
            market_id = %snapshot.market.market_id,
            combined_raw = %combined_raw,
            combined_after_fees = %combined_after_fees,
            "no arb: combined cost >= 1.0"
        );
        return None;
    }

    if profit_per_unit < config.min_arb_profit {
        debug!(
            market_id = %snapshot.market.market_id,
            profit = %profit_per_unit,
            min = %config.min_arb_profit,
            "arb below threshold"
        );
        return None;
    }

    // Size: limited by max_order_size_usdc. Each unit costs combined_after_fees in USDC.
    let max_units = config.max_order_size_usdc / combined_after_fees;
    let suggested_size = config.max_order_size_usdc;
    let expected_profit = max_units * profit_per_unit;

    let opp = Opportunity {
        market_id: snapshot.market.market_id.clone(),
        asset: snapshot.market.asset.clone(),
        question: snapshot.market.question.clone(),
        kind: OpportunityKind::Arb,
        combined_ask: combined_raw,
        up_ask,
        down_ask,
        up_fee,
        down_fee,
        combined_ask_after_fees: combined_after_fees,
        profit_per_unit,
        suggested_size_usdc: suggested_size,
        expected_profit_usdc: expected_profit.round_dp(6),
        up_token_id: snapshot.market.up_token_id.clone(),
        down_token_id: snapshot.market.down_token_id.clone(),
        timestamp: snapshot.timestamp,
    };

    info!(
        market_id = %opp.market_id,
        asset = %opp.asset,
        profit_per_unit = %opp.profit_per_unit,
        expected_profit = %opp.expected_profit_usdc,
        "ARB DETECTED"
    );

    Some(opp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use rust_decimal_macros::dec;

    use crate::market::MarketPair;

    fn test_config() -> StrategyConfig {
        StrategyConfig {
            min_arb_profit: dec!(0.001),
            assets: vec!["BTC".into()],
            max_order_size_usdc: dec!(100),
            min_fresh_sources: 2,
            max_price_age_ms: 2000,
            max_divergence_pct: dec!(0.5),
            max_leg_price: dec!(0.90),
        }
    }

    fn test_market() -> MarketPair {
        MarketPair {
            market_id: "test-market-1".into(),
            question: "Bitcoin Up or Down - test".into(),
            asset: "BTC".into(),
            slug: "btc-updown-15m-0".into(),
            up_token_id: "up-token-123".into(),
            down_token_id: "down-token-456".into(),
            end_date: None,
            liquidity: None,
            neg_risk: false,
        }
    }

    fn snapshot(up_ask: Option<Decimal>, down_ask: Option<Decimal>) -> MarketSnapshot {
        MarketSnapshot {
            market: test_market(),
            up_best_bid: None,
            up_best_ask: up_ask,
            down_best_bid: None,
            down_best_ask: down_ask,
            timestamp: Utc::now(),
        }
    }

    #[test]
    fn detects_arb_opportunity() {
        let snap = snapshot(Some(dec!(0.45)), Some(dec!(0.45)));
        let opp = detect_arb(&snap, &test_config());
        assert!(opp.is_some());
        let opp = opp.unwrap();
        assert!(opp.profit_per_unit > Decimal::ZERO);
        assert!(matches!(opp.kind, OpportunityKind::Arb));
    }

    #[test]
    fn no_arb_when_prices_too_high() {
        let snap = snapshot(Some(dec!(0.55)), Some(dec!(0.55)));
        assert!(detect_arb(&snap, &test_config()).is_none());
    }

    #[test]
    fn no_arb_with_missing_ask() {
        let snap = snapshot(Some(dec!(0.45)), None);
        assert!(detect_arb(&snap, &test_config()).is_none());
    }

    #[test]
    fn no_arb_below_threshold() {
        let config = StrategyConfig {
            min_arb_profit: dec!(0.10),
            ..test_config()
        };
        // Small arb that's below the 10% threshold
        let snap = snapshot(Some(dec!(0.48)), Some(dec!(0.48)));
        assert!(detect_arb(&snap, &config).is_none());
    }

    #[test]
    fn no_arb_when_leg_price_too_high() {
        // up=0.05, down=0.92 — valid arb but down exceeds max_leg_price=0.90
        let snap = snapshot(Some(dec!(0.05)), Some(dec!(0.92)));
        assert!(detect_arb(&snap, &test_config()).is_none());
    }

    #[test]
    fn arb_within_leg_price_limit() {
        // up=0.40, down=0.50 — both within max_leg_price=0.90
        let snap = snapshot(Some(dec!(0.40)), Some(dec!(0.50)));
        assert!(detect_arb(&snap, &test_config()).is_some());
    }

    #[test]
    fn rejects_invalid_prices() {
        let snap = snapshot(Some(dec!(0.00)), Some(dec!(0.45)));
        assert!(detect_arb(&snap, &test_config()).is_none());

        let snap = snapshot(Some(dec!(1.00)), Some(dec!(0.45)));
        assert!(detect_arb(&snap, &test_config()).is_none());
    }
}
