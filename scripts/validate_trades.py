#!/usr/bin/env python3
"""
Validate trades.jsonl against config.toml strategy limits.

For each record checks that the pipeline stage is consistent with the
configured thresholds (max_leg_price, max_order_size_usdc). Flags any
record where the filter decision appears to contradict the config.

Usage:
    python3 scripts/validate_trades.py [trades.jsonl] [config.toml]

Defaults:
    trades.jsonl  = logs/trades.jsonl
    config.toml   = config.toml
"""

import json
import math
import sys
from decimal import Decimal, ROUND_CEILING, ROUND_FLOOR
from pathlib import Path

# --------------------------------------------------------------------------- #
# Config loading (stdlib only — no tomllib dependency)
# --------------------------------------------------------------------------- #

def load_config(path: str) -> dict:
    """Extract strategy limits from config.toml using simple line parsing."""
    cfg = {
        "max_leg_price": None,
        "max_order_size_usdc": None,
        "min_order_usdc": Decimal("1.0"),   # CLOB minimum — not in config yet
        "min_arb_profit": Decimal("0.001"),
    }
    in_strategy = False
    with open(path) as f:
        for line in f:
            line = line.strip()
            if line.startswith("["):
                in_strategy = (line == "[strategy]")
                continue
            if not in_strategy or "=" not in line or line.startswith("#"):
                continue
            key, _, val = line.partition("=")
            key = key.strip()
            val = val.split("#")[0].strip().strip('"')
            if key == "max_leg_price":
                cfg["max_leg_price"] = Decimal(val)
            elif key == "max_order_size_usdc":
                cfg["max_order_size_usdc"] = Decimal(val)
            elif key == "min_order_usdc":
                cfg["min_order_usdc"] = Decimal(val)
            elif key == "min_arb_profit":
                cfg["min_arb_profit"] = Decimal(val)
    if cfg["max_leg_price"] is None or cfg["max_order_size_usdc"] is None:
        raise ValueError("Could not find max_leg_price / max_order_size_usdc in config.toml")
    return cfg


# --------------------------------------------------------------------------- #
# Fee calculation — must match src/strategy/fees.rs
# --------------------------------------------------------------------------- #

FEE_RATE = Decimal("0.02")

def taker_fee(price: Decimal) -> Decimal:
    return min(price, Decimal("1") - price) * FEE_RATE

def combined_after_fees(up: Decimal, down: Decimal) -> Decimal:
    return up + taker_fee(up) + down + taker_fee(down)


# --------------------------------------------------------------------------- #
# Per-record validation
# --------------------------------------------------------------------------- #

def check_arb_rejected(rec: dict, cfg: dict) -> list[str]:
    """Validate an arb_rejected record. Returns list of anomaly strings."""
    anomalies = []
    up  = Decimal(str(rec["up_ask"]))
    dn  = Decimal(str(rec["down_ask"]))
    reason = rec.get("reason", "")
    mlp = cfg["max_leg_price"]

    if reason == "leg_price_exceeds_max":
        # At least one leg must be > max_leg_price
        if up <= mlp and dn <= mlp:
            anomalies.append(
                f"leg_price_exceeds_max but up={up} down={dn} both <= max_leg_price={mlp}"
            )

    elif reason == "min_notional":
        # Both legs must be within leg price limit (passed that filter)
        if up > mlp or dn > mlp:
            anomalies.append(
                f"min_notional rejection but up={up} or down={dn} > max_leg_price={mlp} "
                f"(should have been caught earlier)"
            )
        else:
            # Verify budget_units < size_min
            caf = combined_after_fees(up, dn)
            budget = (cfg["max_order_size_usdc"] / caf).to_integral_value(rounding=ROUND_FLOOR)
            min_up = (cfg["min_order_usdc"] / up).to_integral_value(rounding=ROUND_CEILING)
            min_dn = (cfg["min_order_usdc"] / dn).to_integral_value(rounding=ROUND_CEILING)
            size_min = max(min_up, min_dn)
            if budget >= size_min:
                anomalies.append(
                    f"min_notional rejection but budget_units={budget} >= size_min={size_min} "
                    f"(up={up} down={dn}) — should have passed"
                )

    elif reason == "depth_too_thin":
        # Both legs within leg price limit, and min_notional should have passed
        if up > mlp or dn > mlp:
            anomalies.append(
                f"depth_too_thin rejection but up={up} or down={dn} > max_leg_price={mlp}"
            )
        else:
            caf = combined_after_fees(up, dn)
            budget = (cfg["max_order_size_usdc"] / caf).to_integral_value(rounding=ROUND_FLOOR)
            min_up = (cfg["min_order_usdc"] / up).to_integral_value(rounding=ROUND_CEILING)
            min_dn = (cfg["min_order_usdc"] / dn).to_integral_value(rounding=ROUND_CEILING)
            size_min = max(min_up, min_dn)
            if budget < size_min:
                anomalies.append(
                    f"depth_too_thin rejection but budget_units={budget} < size_min={size_min} "
                    f"(up={up} down={dn}) — min_notional should have fired first"
                )
        # Depth itself we can't verify from the log (not recorded). That's fine.

    return anomalies


def check_full_record(rec: dict, cfg: dict, stage: str) -> list[str]:
    """Validate a record that has a full Opportunity (executed / freshness / risk)."""
    anomalies = []
    up  = Decimal(str(rec["up_ask"]))
    dn  = Decimal(str(rec["down_ask"]))
    mlp = cfg["max_leg_price"]

    # All these stages passed arb detection — both legs must be within limit
    if up > mlp or dn > mlp:
        anomalies.append(
            f"stage={stage} but up={up} or down={dn} > max_leg_price={mlp} "
            f"(should have been arb_rejected before reaching this stage)"
        )

    if stage in ("executed", "execution_failed"):
        total = Decimal(str(rec.get("total_cost_usdc", 0)))
        cap   = cfg["max_order_size_usdc"]
        # Allow a small rounding tolerance (floor() in strategy)
        if total > cap + Decimal("0.01"):
            anomalies.append(
                f"stage={stage} total_cost_usdc={total} > max_order_size_usdc={cap}"
            )

    return anomalies


# --------------------------------------------------------------------------- #
# Main
# --------------------------------------------------------------------------- #

def main():
    trades_path = sys.argv[1] if len(sys.argv) > 1 else "logs/trades.jsonl"
    config_path = sys.argv[2] if len(sys.argv) > 2 else "config.toml"

    if not Path(trades_path).exists():
        print(f"ERROR: {trades_path} not found")
        sys.exit(1)

    cfg = load_config(config_path)
    max_leg = cfg["max_leg_price"]
    max_size = cfg["max_order_size_usdc"]

    print()
    print("=" * 72)
    print("  Trade Log Validator")
    print(f"  config:  max_leg_price={max_leg}  max_order_size_usdc=${max_size}")
    print(f"  source:  {trades_path}")
    print("=" * 72)

    counts   = {}   # stage -> int
    anomalies = []  # (line_number, stage, market_id, description)

    with open(trades_path) as f:
        for lineno, line in enumerate(f, 1):
            line = line.strip()
            if not line:
                continue
            try:
                rec = json.loads(line)
            except json.JSONDecodeError as e:
                anomalies.append((lineno, "?", "?", f"JSON parse error: {e}"))
                continue

            stage = rec.get("stage", "unknown")
            market_id = rec.get("market_id", "?")
            counts[stage] = counts.get(stage, 0) + 1

            if stage == "arb_rejected":
                errors = check_arb_rejected(rec, cfg)
            elif stage in ("executed", "execution_failed", "freshness_rejected", "risk_rejected"):
                errors = check_full_record(rec, cfg, stage)
            else:
                errors = [f"unknown stage: {stage}"]

            for err in errors:
                anomalies.append((lineno, stage, market_id, err))

    # ------------------------------------------------------------------ #
    # Summary
    # ------------------------------------------------------------------ #
    total = sum(counts.values())
    print()
    print(f"  Records: {total:,}")
    for stage, n in sorted(counts.items()):
        print(f"    {stage:<22} {n:>6,}")

    # Breakdown of arb_rejected reasons
    reason_counts: dict[str, int] = {}
    with open(trades_path) as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            try:
                rec = json.loads(line)
            except Exception:
                continue
            if rec.get("stage") == "arb_rejected":
                r = rec.get("reason", "?")
                reason_counts[r] = reason_counts.get(r, 0) + 1

    if reason_counts:
        print()
        print("  arb_rejected breakdown:")
        for reason, n in sorted(reason_counts.items(), key=lambda x: -x[1]):
            pct = 100 * n / counts.get("arb_rejected", 1)
            print(f"    {reason:<30} {n:>6,}  ({pct:.1f}%)")

    # ------------------------------------------------------------------ #
    # Anomalies
    # ------------------------------------------------------------------ #
    print()
    if not anomalies:
        print("  RESULT: OK — all records are consistent with config limits")
    else:
        print(f"  RESULT: FAIL — {len(anomalies)} anomaly(s) found:")
        print()
        for lineno, stage, market_id, desc in anomalies[:50]:
            print(f"    line {lineno:>6}  [{stage}]  market={market_id}")
            print(f"              {desc}")
        if len(anomalies) > 50:
            print(f"    ... and {len(anomalies) - 50} more")

    print()


if __name__ == "__main__":
    main()
