#!/usr/bin/env bash
# Quick dry-run test: run the bot for N minutes, then summarize the log.
# Usage: ./scripts/dry-run-test.sh [minutes]  (default: 3)
#
# Always forces mode = "dry_run" regardless of what config.toml says,
# then restores the original config when done (even on Ctrl+C).

set -euo pipefail

MINUTES="${1:-3}"
SECONDS_TOTAL=$((MINUTES * 60))
LOG_FILE="logs/trades.jsonl"
CONFIG_FILE="config.toml"
CONFIG_BACKUP="${CONFIG_FILE}.bak"

echo "=== MTB Dry-Run Test ==="
echo "Duration: ${MINUTES} minute(s)"
echo "Log file: ${LOG_FILE}"
echo ""

# --- Force dry_run mode and restore on exit ---
cp "$CONFIG_FILE" "$CONFIG_BACKUP"
restore_config() {
    mv "$CONFIG_BACKUP" "$CONFIG_FILE"
    echo ""
    echo "(config.toml restored)"
}
trap restore_config EXIT

# Swap active mode line to dry_run (leave comments untouched)
sed -i.tmp 's/^mode = "live"/mode = "dry_run"/' "$CONFIG_FILE"
rm -f "${CONFIG_FILE}.tmp"

echo "Mode forced to dry_run for this test."
echo ""

# Clear previous log for a clean test
mkdir -p logs
> "$LOG_FILE"

echo "Starting bot in dry-run mode..."
echo "Press Ctrl+C to stop early."
echo ""

# Run the bot in background, kill after timeout
cargo run 2>&1 &
BOT_PID=$!

sleep "${SECONDS_TOTAL}"
kill "$BOT_PID" 2>/dev/null || true
wait "$BOT_PID" 2>/dev/null || true

echo ""
echo "=== Dry-Run Summary ==="
echo ""

if [ ! -s "$LOG_FILE" ]; then
    echo "No arbs detected during this run."
    echo ""
    echo "This is normal — arbs are rare. The bot is working if you saw"
    echo "exchange price ticks and market discovery in the output above."
    exit 0
fi

TOTAL=$(wc -l < "$LOG_FILE" | tr -d ' ')
echo "Total arb detections: ${TOTAL}"
echo ""

# Count by pipeline stage
echo "By pipeline stage:"
jq -r '.stage' "$LOG_FILE" 2>/dev/null | sort | uniq -c | sort -rn || echo "  (jq not available, raw log below)"
echo ""

# Count by asset
echo "By asset:"
jq -r '.asset' "$LOG_FILE" 2>/dev/null | sort | uniq -c | sort -rn || true
echo ""

# Show rejection reasons
REJECTED=$(jq -r 'select(.reason != null) | .reason' "$LOG_FILE" 2>/dev/null | wc -l | tr -d ' ')
if [ "$REJECTED" -gt 0 ]; then
    echo "Rejection reasons:"
    jq -r 'select(.reason != null) | .reason' "$LOG_FILE" 2>/dev/null | sort | uniq -c | sort -rn
    echo ""
fi

# Show profit range for executed arbs
EXECUTED=$(jq -r 'select(.stage == "executed") | .expected_profit_usdc' "$LOG_FILE" 2>/dev/null | wc -l | tr -d ' ')
if [ "$EXECUTED" -gt 0 ]; then
    echo "Executed arbs — expected profit (USDC):"
    jq -r 'select(.stage == "executed") | .expected_profit_usdc' "$LOG_FILE" 2>/dev/null | sort -n | head -5
    echo "  ..."
    jq -r 'select(.stage == "executed") | .expected_profit_usdc' "$LOG_FILE" 2>/dev/null | sort -n | tail -5
    echo ""
fi

# Show execution_failed breakdown (FOK kills vs real failures)
EXEC_FAILED=$(jq -r 'select(.stage == "execution_failed") | .reason' "$LOG_FILE" 2>/dev/null | wc -l | tr -d ' ')
if [ "$EXEC_FAILED" -gt 0 ]; then
    echo "Execution failures (${EXEC_FAILED} total) — top reasons:"
    jq -r 'select(.stage == "execution_failed") | .reason' "$LOG_FILE" 2>/dev/null | sort | uniq -c | sort -rn | head -5
    echo ""
fi

# Show exchange snapshot from latest record
echo "Latest exchange snapshot:"
jq -r '.exchange_snapshot | "  \(.asset): \(.exchanges | map("\(.exchange)=$\(.price) (\(.age_ms)ms)") | join(", ")) | fresh=\(.fresh_count) div=\(.divergence_pct // "n/a")% chainlink=$\(.chainlink_price // "n/a")"' "$LOG_FILE" 2>/dev/null | tail -3
echo ""

echo "Full log: ${LOG_FILE}"
echo "View with: jq . ${LOG_FILE}"
