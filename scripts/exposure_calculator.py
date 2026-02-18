#!/usr/bin/env python3
"""
Arb sizing exposure calculator.

Given max_order_size_usdc and max_leg_price, shows the full exposure table
across all cheap-leg prices so you can calibrate config before going live.

Expensive leg is fixed at max_leg_price (worst-case partial fill scenario).

Usage:
    python3 scripts/exposure_calculator.py [max_order_size_usdc] [max_leg_price]

Examples:
    python3 scripts/exposure_calculator.py 3.0 0.50
    python3 scripts/exposure_calculator.py 5.0 0.60
"""

import sys
import math

FEE_RATE       = 0.02   # Polymarket taker fee rate
MIN_ORDER_USDC = 1.00   # CLOB minimum notional per individual order
MIN_PROFIT_PCT = 0.005  # 0.5% minimum arb profit after fees (matches config)


def taker_fee(price: float) -> float:
    """Polymarket taker fee: 2% of min(price, 1 - price)."""
    return round(min(price, 1.0 - price) * FEE_RATE, 6)


def main():
    max_budget = float(sys.argv[1]) if len(sys.argv) > 1 else 3.0
    max_leg    = float(sys.argv[2]) if len(sys.argv) > 2 else 0.50

    print()
    print("=" * 88)
    print(f"  Arb Sizing Exposure Calculator")
    print(f"  max_order_size_usdc = ${max_budget:.2f}   max_leg_price = {max_leg:.2f}")
    print(f"  Expensive leg fixed at {max_leg:.2f} — worst-case directional exposure on partial fill")
    print("=" * 88)
    print()
    print(f"  {'Cheap':>6} {'Exp':>5} | "
          f"{'MinUnits':>8} {'BudgetUnits':>11} | "
          f"{'CheapCost':>9} {'ExpCost':>8} {'Total':>7} | "
          f"{'Profit':>7} | "
          f"{'MinExposure':>11} {'MaxExposure':>11} | Status")
    print("  " + "-" * 85)

    cheap = 0.05
    any_row = False

    while cheap <= max_leg + 1e-9:
        cheap = round(cheap, 2)
        exp   = round(max_leg, 2)

        fee_c    = taker_fee(cheap)
        fee_e    = taker_fee(exp)
        combined = cheap + exp + fee_c + fee_e
        profit   = 1.0 - combined

        # Must be a real arb above the min profit threshold
        if profit <= MIN_PROFIT_PCT:
            cheap += 0.01
            continue

        # 8a: minimum units so each leg individually costs >= MIN_ORDER_USDC
        min_units    = max(math.ceil(MIN_ORDER_USDC / cheap),
                          math.ceil(MIN_ORDER_USDC / exp))

        # Maximum units from budget cap
        budget_units = math.floor(max_budget / combined)

        passed = budget_units >= min_units

        # Min scenario: trade at size_min (cheapest valid trade)
        min_cheap_cost = round(min_units * cheap, 2)
        min_exp_cost   = round(min_units * exp, 2)
        min_total      = round(min_units * combined, 2)

        # Max scenario: trade at budget_units (most the budget allows)
        max_exp_cost = round(budget_units * exp, 2) if passed else None

        profit_pct = profit * 100
        status     = "OK" if passed else "<< FILTERED (8a)"

        max_exp_str = f"${max_exp_cost:>9.2f}" if max_exp_cost is not None else f"{'n/a':>10}"

        print(f"  ${cheap:5.2f} ${exp:4.2f} | "
              f"{min_units:8d} {budget_units:11d} | "
              f"${min_cheap_cost:8.2f} ${min_exp_cost:7.2f} ${min_total:6.2f} | "
              f"{profit_pct:6.2f}% | "
              f"${min_exp_cost:>10.2f} {max_exp_str} | {status}")

        any_row = True
        cheap += 0.01

    if not any_row:
        print("  No valid arbs found — combined price after fees exceeds 1.0 for all combinations.")

    print()
    print("  Columns:")
    print("    MinUnits    = min shares so both legs individually cost >= $1.00 (CLOB minimum)")
    print("    BudgetUnits = max shares budget allows: floor(budget / combined_after_fees)")
    print("    CheapCost   = MinUnits × cheap_leg_price")
    print("    ExpCost     = MinUnits × expensive_leg_price")
    print("    Total       = MinUnits × combined_after_fees (both legs, at minimum trade size)")
    print("    Profit      = (1 - combined_after_fees) × 100 — profit % per unit pair")
    print("    MinExposure = ExpCost at MinUnits — partial fill loss if you can't avoid it")
    print("    MaxExposure = ExpCost at BudgetUnits — worst case if book is deep")
    print("    FILTERED    = budget_units < min_units → bot rejects via 8a, never fires")
    print()


if __name__ == "__main__":
    main()
