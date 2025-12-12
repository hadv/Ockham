#!/bin/bash
set -e

# Cleanup function
cleanup() {
    echo "Stopping all nodes..."
    pkill -f "cargo run --quiet --" || true
}
trap cleanup EXIT

# Clean old logs
rm -f node*.log

echo "Building..."
cargo build --quiet

# Start 5 Nodes (0, 1, 2, 3, 4)
echo "Starting 5 Nodes..."
RUST_LOG=info cargo run --quiet -- 0 > node0.log 2>&1 &
PID0=$!
sleep 2

RUST_LOG=info cargo run --quiet -- 1 > node1.log 2>&1 &
PID1=$!
RUST_LOG=info cargo run --quiet -- 2 > node2.log 2>&1 &
PID2=$!
RUST_LOG=info cargo run --quiet -- 3 > node3.log 2>&1 &
PID3=$!
RUST_LOG=info cargo run --quiet -- 4 > node4.log 2>&1 &
PID4=$!

echo "Nodes started. Waiting for View 1 and View 2 (approx 30s)..."
# View 1 and 2 should complete normally.

# Wait until we see activity for View 2?
# Let's just wait a fixed buffer. View 1 takes <5s (happy path). View 2 takes <5s.
sleep 15

# KILL NODE 3 (Leader for View 3)
echo "!!! KILLING NODE 3 (Leader View 3) !!!"
kill $PID3
echo "Node 3 killed. Waiting for timeout and recovery (View 3 -> Timeout -> View 4)..."

# View 3 is 30s timeout. So we wait >30s.
sleep 45

echo "--- FINALIZED BLOCKS (Last 5) ---"
grep "EXPLICITLY FINALIZED VIEW" node*.log | tail -n 5

echo ""
echo "--- CONSENSUS HEALTH CHECK ---"
# If we reached View > 4, it means we handled the timeout.
# We can also check if we see "QC Formed for View 3" which was the timeout view.
echo "Checking for View 3 QC..."
if grep -q "QC Formed for View 3" node*.log; then
    echo "SUCCESS: Dummy QC for View 3 formed."
    grep "QC Formed for View 3" node*.log | head -n 1
else
    echo "WARNING: Did not find explicit log for View 3 QC, but checking max view..."
fi
# Verify we reached View 4 or 5
# Logs show "View Advanced to 3. Resetting Timer."
# grep -o "View Advanced to [0-9]*" gives "View Advanced to 3"
# awk '{print $NF}' gives "3"
MAX_VIEW=$(grep -o "View Advanced to [0-9]*" node0.log | awk '{print $NF}' | sort -n | tail -1)
echo "Max View Reached: $MAX_VIEW"

if [ "$MAX_VIEW" -ge 4 ]; then
    echo "SUCCESS: Network recovered and advanced to View 4+."
else
    echo "FAILURE: Network stalled at View $MAX_VIEW."
    exit 1
fi
