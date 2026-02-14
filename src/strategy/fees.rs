use rust_decimal::Decimal;
use rust_decimal_macros::dec;

/// Polymarket dynamic taker fee model.
///
/// Taker fees on Polymarket are calculated as a percentage of the *lesser*
/// of the price and (1 - price), scaled by a fee rate. The fee rate is 2%
/// (200 bps) as of the current fee schedule.
///
/// Formula: fee = price * fee_rate if price <= 0.5
///          fee = (1 - price) * fee_rate if price > 0.5
///
/// This means fees are highest at 0.5 (1 cent on a $0.50 contract at 2%)
/// and lowest near 0 or 1.
const FEE_RATE: Decimal = dec!(0.02);

/// Calculate the taker fee for buying at a given ask price.
pub fn taker_fee(price: Decimal) -> Decimal {
    let complement = Decimal::ONE - price;
    let base = if price <= complement {
        price
    } else {
        complement
    };
    (base * FEE_RATE).round_dp(6)
}

/// Calculate the total cost of buying one unit at the given ask price (price + fee).
#[allow(dead_code)]
pub fn total_cost(price: Decimal) -> Decimal {
    price + taker_fee(price)
}

/// Calculate the effective combined cost of buying both YES and NO at their ask prices.
/// Returns (combined_cost_after_fees, yes_fee, no_fee).
pub fn combined_cost_after_fees(yes_ask: Decimal, no_ask: Decimal) -> (Decimal, Decimal, Decimal) {
    let yes_fee = taker_fee(yes_ask);
    let no_fee = taker_fee(no_ask);
    let total = yes_ask + no_ask + yes_fee + no_fee;
    (total, yes_fee, no_fee)
}

/// Check if there is an arb opportunity: buying YES + NO for less than 1.0 after fees.
/// Returns the profit per unit if profitable, or None.
#[allow(dead_code)]
pub fn arb_profit_after_fees(yes_ask: Decimal, no_ask: Decimal) -> Option<Decimal> {
    let (combined, _, _) = combined_cost_after_fees(yes_ask, no_ask);
    let profit = Decimal::ONE - combined;
    if profit > Decimal::ZERO {
        Some(profit)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn fee_at_50_cents() {
        // At 0.50: fee = 0.50 * 0.02 = 0.01
        assert_eq!(taker_fee(dec!(0.50)), dec!(0.010000));
    }

    #[test]
    fn fee_at_low_price() {
        // At 0.10: fee = 0.10 * 0.02 = 0.002
        assert_eq!(taker_fee(dec!(0.10)), dec!(0.002000));
    }

    #[test]
    fn fee_at_high_price() {
        // At 0.90: complement = 0.10, fee = 0.10 * 0.02 = 0.002
        assert_eq!(taker_fee(dec!(0.90)), dec!(0.002000));
    }

    #[test]
    fn fee_at_boundary() {
        // At 0.01: fee = 0.01 * 0.02 = 0.0002
        assert_eq!(taker_fee(dec!(0.01)), dec!(0.000200));
    }

    #[test]
    fn fee_symmetry() {
        // Fee at price p should equal fee at (1-p)
        assert_eq!(taker_fee(dec!(0.30)), taker_fee(dec!(0.70)));
        assert_eq!(taker_fee(dec!(0.20)), taker_fee(dec!(0.80)));
        assert_eq!(taker_fee(dec!(0.10)), taker_fee(dec!(0.90)));
    }

    #[test]
    fn arb_exists_when_combined_less_than_one() {
        // YES at 0.45, NO at 0.45 = 0.90 raw
        // YES fee = 0.45 * 0.02 = 0.009, NO fee = 0.009
        // Combined = 0.90 + 0.018 = 0.918
        // Profit = 1.0 - 0.918 = 0.082
        let profit = arb_profit_after_fees(dec!(0.45), dec!(0.45));
        assert!(profit.is_some());
        assert_eq!(profit.unwrap(), dec!(0.082000));
    }

    #[test]
    fn no_arb_when_combined_exceeds_one() {
        // YES at 0.55, NO at 0.55 = 1.10 raw — no arb possible
        let profit = arb_profit_after_fees(dec!(0.55), dec!(0.55));
        assert!(profit.is_none());
    }

    #[test]
    fn no_arb_at_fair_price() {
        // YES at 0.50, NO at 0.50 = 1.00 raw + fees = >1.00
        let profit = arb_profit_after_fees(dec!(0.50), dec!(0.50));
        assert!(profit.is_none());
    }

    #[test]
    fn arb_marginal_case() {
        // YES at 0.48, NO at 0.48 = 0.96 raw
        // Fees: 0.48 * 0.02 = 0.0096 each, total fees = 0.0192
        // Combined = 0.96 + 0.0192 = 0.9792
        // Profit = 0.0208
        let profit = arb_profit_after_fees(dec!(0.48), dec!(0.48));
        assert!(profit.is_some());
        let p = profit.unwrap();
        assert!(p > dec!(0.02) && p < dec!(0.03), "profit was {p}");
    }

    #[test]
    fn combined_cost_calculation() {
        let (combined, yes_fee, no_fee) = combined_cost_after_fees(dec!(0.40), dec!(0.55));
        // YES fee: 0.40 * 0.02 = 0.008
        assert_eq!(yes_fee, dec!(0.008000));
        // NO fee: (1 - 0.55) = 0.45 * 0.02 = 0.009
        assert_eq!(no_fee, dec!(0.009000));
        // Combined: 0.40 + 0.55 + 0.008 + 0.009 = 0.967
        assert_eq!(combined, dec!(0.967000));
    }
}
