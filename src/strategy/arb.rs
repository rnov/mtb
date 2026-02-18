use rust_decimal::Decimal;
use tracing::{debug, info};

use crate::config::types::StrategyConfig;
use crate::strategy::fees;
use crate::strategy::types::{ArbCheck, ArbRejection, MarketSnapshot, Opportunity, OpportunityKind};

/// Pure arb strategy: detect when up_ask + down_ask < 1.0 after fees.
///
/// This function is pure computation — no IO, no async. Fully unit-testable.
///
/// Returns:
/// - `ArbCheck::NoData` — prices missing or combined cost ≥ $1 (skip silently, too noisy)
/// - `ArbCheck::Rejected` — strategy filter fired (leg price / notional / depth); logged
/// - `ArbCheck::Found` — opportunity passes all filters
pub fn detect_arb(snapshot: &MarketSnapshot, config: &StrategyConfig) -> ArbCheck {
    let (up_ask, down_ask) = match (snapshot.up_best_ask, snapshot.down_best_ask) {
        (Some(u), Some(d)) => (u, d),
        _ => return ArbCheck::NoData,
    };

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
        return ArbCheck::NoData;
    }

    // Profitability check — must confirm a real arb exists before any strategy
    // filter can produce a Rejected record. Anything that returns NoData below
    // was never an arb to begin with and is not worth logging.
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
        return ArbCheck::NoData;
    }

    if profit_per_unit < config.min_arb_profit {
        debug!(
            market_id = %snapshot.market.market_id,
            profit = %profit_per_unit,
            min = %config.min_arb_profit,
            "arb below threshold"
        );
        return ArbCheck::NoData;
    }

    // From here on: a real arb exists (combined < $1, profit ≥ threshold).
    // Any return below is a strategy filter decision — log it as Rejected.

    // Cap individual leg prices to limit directional exposure on partial fill.
    // E.g., up=0.06 / down=0.93 combined=$0.99 is a real arb but if down fails
    // to fill, you hold a $0.06 token that's almost certainly worthless.
    if up_ask > config.max_leg_price || down_ask > config.max_leg_price {
        debug!(
            market_id = %snapshot.market.market_id,
            up_ask = %up_ask,
            down_ask = %down_ask,
            max = %config.max_leg_price,
            "skipping: leg price exceeds max"
        );
        return ArbCheck::Rejected(ArbRejection {
            market_id: snapshot.market.market_id.clone(),
            asset: snapshot.market.asset.clone(),
            up_ask,
            down_ask,
            reason: "leg_price_exceeds_max",
        });
    }

    // Units are floored to integers (CLOB requires maker_amount ≤ 2dp).
    let budget_units = (config.max_order_size_usdc / combined_after_fees).floor();

    // 8a: Min-notional filter.
    // CLOB enforces a minimum notional per order (default $1).
    // Each leg is an independent order, so both must individually clear the minimum.
    // min_shares = ceil(min_order_usdc / leg_price) — fewest shares where cost ≥ $1.
    let min_shares_up = (config.min_order_usdc / up_ask).ceil();
    let min_shares_down = (config.min_order_usdc / down_ask).ceil();
    let size_min = min_shares_up.max(min_shares_down);

    if budget_units < size_min {
        debug!(
            market_id = %snapshot.market.market_id,
            up_ask = %up_ask,
            down_ask = %down_ask,
            budget_units = %budget_units,
            size_min = %size_min,
            "skipping: budget yields too few units to clear CLOB $1 minimum per leg"
        );
        return ArbCheck::Rejected(ArbRejection {
            market_id: snapshot.market.market_id.clone(),
            asset: snapshot.market.asset.clone(),
            up_ask,
            down_ask,
            reason: "min_notional",
        });
    }

    // 8c: Executable size check.
    // For a FOK buy at the best ask, we can only fill shares available at that price level.
    // executable = size at first (cheapest) ask level. size_cap = min of both sides.
    let executable_up = snapshot.up_ask_levels.first().map(|(_, s)| *s).unwrap_or(Decimal::ZERO);
    let executable_down = snapshot.down_ask_levels.first().map(|(_, s)| *s).unwrap_or(Decimal::ZERO);
    let size_cap = executable_up.min(executable_down).floor();

    if size_cap < size_min {
        debug!(
            market_id = %snapshot.market.market_id,
            executable_up = %executable_up,
            executable_down = %executable_down,
            size_cap = %size_cap,
            size_min = %size_min,
            "skipping: book depth too thin to fill minimum order on both legs"
        );
        return ArbCheck::Rejected(ArbRejection {
            market_id: snapshot.market.market_id.clone(),
            asset: snapshot.market.asset.clone(),
            up_ask,
            down_ask,
            reason: "depth_too_thin",
        });
    }

    // Cap units to what the book can actually fill, then derive total cost.
    let units = budget_units.min(size_cap);
    let total_cost = units * combined_after_fees;
    let expected_profit = units * profit_per_unit;

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
        units,
        total_cost_usdc: total_cost,
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

    ArbCheck::Found(Box::new(opp))
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
            max_order_size_usdc: dec!(3),
            min_fresh_sources: 2,
            max_price_age_ms: 2000,
            max_divergence_pct: dec!(0.5),
            max_leg_price: dec!(0.90),
            min_order_usdc: dec!(1.0),
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

    /// Snapshot with liquid depth (100 shares at each ask price).
    fn snapshot(up_ask: Option<Decimal>, down_ask: Option<Decimal>) -> MarketSnapshot {
        MarketSnapshot {
            market: test_market(),
            up_best_bid: None,
            up_best_ask: up_ask,
            down_best_bid: None,
            down_best_ask: down_ask,
            up_ask_levels: up_ask.map(|p| vec![(p, dec!(100))]).unwrap_or_default(),
            down_ask_levels: down_ask.map(|p| vec![(p, dec!(100))]).unwrap_or_default(),
            timestamp: Utc::now(),
        }
    }

    /// Snapshot with explicit depth for thin-book tests.
    fn snapshot_with_depth(
        up_ask: Decimal,
        down_ask: Decimal,
        up_depth: Vec<(Decimal, Decimal)>,
        down_depth: Vec<(Decimal, Decimal)>,
    ) -> MarketSnapshot {
        MarketSnapshot {
            market: test_market(),
            up_best_bid: None,
            up_best_ask: Some(up_ask),
            down_best_bid: None,
            down_best_ask: Some(down_ask),
            up_ask_levels: up_depth,
            down_ask_levels: down_depth,
            timestamp: Utc::now(),
        }
    }

    /// Extract opportunity from ArbCheck::Found, or None.
    fn found(result: ArbCheck) -> Option<Opportunity> {
        match result {
            ArbCheck::Found(opp) => Some(*opp),
            _ => None,
        }
    }

    /// Returns true if the result is ArbCheck::Rejected.
    fn is_rejected(result: ArbCheck) -> bool {
        matches!(result, ArbCheck::Rejected(_))
    }

    #[test]
    fn detects_arb_opportunity() {
        let snap = snapshot(Some(dec!(0.45)), Some(dec!(0.45)));
        let opp = found(detect_arb(&snap, &test_config()));
        assert!(opp.is_some());
        let opp = opp.unwrap();
        assert!(opp.profit_per_unit > Decimal::ZERO);
        assert!(matches!(opp.kind, OpportunityKind::Arb));
    }

    #[test]
    fn no_arb_when_prices_too_high() {
        let snap = snapshot(Some(dec!(0.55)), Some(dec!(0.55)));
        assert!(found(detect_arb(&snap, &test_config())).is_none());
    }

    #[test]
    fn no_arb_with_missing_ask() {
        let snap = snapshot(Some(dec!(0.45)), None);
        assert!(found(detect_arb(&snap, &test_config())).is_none());
    }

    #[test]
    fn no_arb_below_threshold() {
        let config = StrategyConfig {
            min_arb_profit: dec!(0.10),
            ..test_config()
        };
        // Small arb that's below the 10% threshold
        let snap = snapshot(Some(dec!(0.48)), Some(dec!(0.48)));
        assert!(found(detect_arb(&snap, &config)).is_none());
    }

    #[test]
    fn no_arb_when_leg_price_too_high() {
        // up=0.05, down=0.92 — valid arb but down exceeds max_leg_price=0.90
        let snap = snapshot(Some(dec!(0.05)), Some(dec!(0.92)));
        assert!(is_rejected(detect_arb(&snap, &test_config())));
    }

    #[test]
    fn arb_within_leg_price_limit() {
        // up=0.40, down=0.50 — both within max_leg_price=0.90
        let snap = snapshot(Some(dec!(0.40)), Some(dec!(0.50)));
        assert!(found(detect_arb(&snap, &test_config())).is_some());
    }

    #[test]
    fn rejects_when_cheap_leg_below_min_notional() {
        // up=0.30, down=0.66, budget=$3
        // units = floor(3 / combined_after_fees) = floor(3/0.9728) = 3
        // min_shares_up = ceil(1/0.30) = 4
        // size_min = 4, units=3 < 4 → skip
        let snap = snapshot(Some(dec!(0.30)), Some(dec!(0.66)));
        assert!(is_rejected(detect_arb(&snap, &test_config())));
    }

    #[test]
    fn accepts_when_both_legs_clear_min_notional() {
        // up=0.47, down=0.49, budget=$3
        // units = floor(3/0.9786) = 3 (approx, after fees)
        // min_shares_up = ceil(1/0.47) = 3
        // min_shares_down = ceil(1/0.49) = 3
        // size_min = 3, units=3 >= 3 → accept
        let snap = snapshot(Some(dec!(0.47)), Some(dec!(0.49)));
        assert!(found(detect_arb(&snap, &test_config())).is_some());
    }

    #[test]
    fn rejects_when_depth_is_zero() {
        // No depth on UP side — size_cap = 0 < size_min → skip
        let snap = snapshot_with_depth(
            dec!(0.47),
            dec!(0.49),
            vec![],                        // UP: no depth
            vec![(dec!(0.49), dec!(100))], // DOWN: liquid
        );
        assert!(is_rejected(detect_arb(&snap, &test_config())));
    }

    #[test]
    fn rejects_when_depth_below_size_min() {
        // up=0.47, down=0.49, budget=$3 → size_min=3
        // UP depth = 2 shares → size_cap = min(2, 100) = 2 < 3 → skip
        let snap = snapshot_with_depth(
            dec!(0.47),
            dec!(0.49),
            vec![(dec!(0.47), dec!(2))],   // UP: only 2 shares
            vec![(dec!(0.49), dec!(100))], // DOWN: liquid
        );
        assert!(is_rejected(detect_arb(&snap, &test_config())));
    }

    #[test]
    fn caps_units_to_available_depth() {
        // up=0.45, down=0.45, budget=$3 → budget_units=3, size_min=3
        // UP depth = 2 shares → size_cap = 2... wait, 2 < size_min=3 → would skip.
        // Use depth=5 on one side to test capping: budget_units=3, size_cap=5 → units=3.
        // Or: budget=$10 → budget_units=10, depth=4 → units capped to 4.
        let config = StrategyConfig {
            max_order_size_usdc: dec!(10),
            ..test_config()
        };
        // up=0.45, down=0.45: size_min = ceil(1/0.45)=3, budget_units=floor(10/0.918)=10
        // UP depth=4, DOWN depth=6 → size_cap=4 → units capped to 4
        let snap = snapshot_with_depth(
            dec!(0.45),
            dec!(0.45),
            vec![(dec!(0.45), dec!(4))],  // UP: 4 shares
            vec![(dec!(0.45), dec!(6))],  // DOWN: 6 shares
        );
        let opp = found(detect_arb(&snap, &config));
        assert!(opp.is_some());
        let opp = opp.unwrap();
        // suggested_size = 4 * combined_after_fees ≈ 4 * 0.918 = 3.672
        assert!(opp.total_cost_usdc < dec!(4));
        assert!(opp.total_cost_usdc > dec!(3));
    }

    #[test]
    fn rejects_invalid_prices() {
        let snap = snapshot(Some(dec!(0.00)), Some(dec!(0.45)));
        assert!(found(detect_arb(&snap, &test_config())).is_none());

        let snap = snapshot(Some(dec!(1.00)), Some(dec!(0.45)));
        assert!(found(detect_arb(&snap, &test_config())).is_none());
    }

    #[test]
    fn rejection_carries_market_id_and_prices() {
        // Verify the rejection record has the right data for logging.
        let snap = snapshot(Some(dec!(0.05)), Some(dec!(0.92)));
        match detect_arb(&snap, &test_config()) {
            ArbCheck::Rejected(r) => {
                assert_eq!(r.market_id, "test-market-1");
                assert_eq!(r.up_ask, dec!(0.05));
                assert_eq!(r.down_ask, dec!(0.92));
                assert_eq!(r.reason, "leg_price_exceeds_max");
            }
            _ => panic!("expected ArbCheck::Rejected"),
        }
    }
}
