#!/usr/bin/env bash
# Timed run: run the bot for N minutes using config.toml as-is, then stop.
# Usage: ./scripts/timed-run.sh [minutes]  (default: 5)
#
# Unlike dry-run-test.sh, this does NOT override config.toml — whatever
# mode is set there (dry_run or live) is what runs.

set -euo pipefail

MINUTES="${1:-5}"
SECONDS_TOTAL=$((MINUTES * 60))
LOG_FILE="logs/trades.jsonl"
CONFIG_FILE="config.toml"

MODE=$(grep '^mode' "$CONFIG_FILE" | head -1 | tr -d ' "' | cut -d= -f2)

echo "=== MTB Timed Run ==="
echo "Duration : ${MINUTES} minute(s)"
echo "Mode     : ${MODE}  (from config.toml — not overridden)"
echo "Log file : ${LOG_FILE}"
echo ""

if [ "$MODE" = "live" ]; then
    echo "WARNING: running in LIVE mode — real orders will be placed."
    echo "Press Ctrl+C within 5 seconds to abort."
    echo ""
    sleep 5
fi

mkdir -p logs

echo "Starting bot..."
echo "Press Ctrl+C to stop early."
echo ""

cargo run 2>&1 &
BOT_PID=$!

sleep "${SECONDS_TOTAL}"
kill "$BOT_PID" 2>/dev/null || true
wait "$BOT_PID" 2>/dev/null || true

echo ""
echo "=== Run Summary ==="
echo ""

if [ ! -s "$LOG_FILE" ]; then
    echo "No arbs logged during this run."
    exit 0
fi

TOTAL=$(wc -l < "$LOG_FILE" | tr -d ' ')
echo "Total arb detections: ${TOTAL}"
echo ""

echo "By pipeline stage:"
jq -r '.stage' "$LOG_FILE" 2>/dev/null | sort | uniq -c | sort -rn || echo "  (jq not available)"
echo ""

echo "By asset:"
jq -r '.asset' "$LOG_FILE" 2>/dev/null | sort | uniq -c | sort -rn || true
echo ""

REJECTED=$(jq -r 'select(.reason != null) | .reason' "$LOG_FILE" 2>/dev/null | wc -l | tr -d ' ')
if [ "$REJECTED" -gt 0 ]; then
    echo "Rejection reasons:"
    jq -r 'select(.reason != null) | .reason' "$LOG_FILE" 2>/dev/null | sort | uniq -c | sort -rn
    echo ""
fi

EXECUTED=$(jq -r 'select(.stage == "executed") | .expected_profit_usdc' "$LOG_FILE" 2>/dev/null | wc -l | tr -d ' ')
if [ "$EXECUTED" -gt 0 ]; then
    echo "Executed arbs — expected profit (USDC):"
    jq -r 'select(.stage == "executed") | .expected_profit_usdc' "$LOG_FILE" 2>/dev/null | sort -n | head -5
    echo "  ..."
    jq -r 'select(.stage == "executed") | .expected_profit_usdc' "$LOG_FILE" 2>/dev/null | sort -n | tail -5
    echo ""
fi

EXEC_FAILED=$(jq -r 'select(.stage == "execution_failed") | .reason' "$LOG_FILE" 2>/dev/null | wc -l | tr -d ' ')
if [ "$EXEC_FAILED" -gt 0 ]; then
    echo "Execution failures (${EXEC_FAILED} total) — top reasons:"
    jq -r 'select(.stage == "execution_failed") | .reason' "$LOG_FILE" 2>/dev/null | sort | uniq -c | sort -rn | head -5
    echo ""
fi

echo "Latest exchange snapshot:"
jq -r '.exchange_snapshot | "  \(.asset): \(.exchanges | map("\(.exchange)=$\(.price) (\(.age_ms)ms)") | join(", ")) | fresh=\(.fresh_count) div=\(.divergence_pct // "n/a")% chainlink=$\(.chainlink_price // "n/a")"' "$LOG_FILE" 2>/dev/null | tail -3
echo ""

echo "Full log: ${LOG_FILE}"
echo "View with: jq . ${LOG_FILE}"
