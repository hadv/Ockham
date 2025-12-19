#!/bin/bash
set -e

# Cleanup function
cleanup() {
    echo "Stopping all nodes..."
    pkill -9 -f ockham || true
    pkill -9 -f cargo || true
    sleep 3
}
trap cleanup EXIT

trap cleanup EXIT

# PRE-FLIGHT CLEANUP
echo "Ensuring no previous nodes are running..."
pkill -9 -f ockham || true
pkill -9 -f cargo || true
sleep 3

# Clean old logs and DB
rm -f node*.log
rm -rf ./db

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

echo "Node 3 killed. Waiting for timeout and recovery (View 4+)..."

# Polling Loop (Max 120s)
MAX_RETRIES=60
SUCCESS=0

for i in $(seq 1 $MAX_RETRIES); do
    MAX_VIEW=$(grep -o "View Advanced to [0-9]*" node*.log | awk '{print $NF}' | sort -n | tail -1)
    if [ -z "$MAX_VIEW" ]; then MAX_VIEW=0; fi
    
    echo "Wait $i/$MAX_RETRIES... Current View: $MAX_VIEW"
    
    if [ "$MAX_VIEW" -ge 4 ]; then
        echo "SUCCESS: Network recovered and advanced to View 4+ (View $MAX_VIEW)."
        SUCCESS=1
        break
    fi
    sleep 2
done

echo "--- FINALIZED BLOCKS (Last 5) ---"
grep "EXPLICITLY FINALIZED VIEW" node*.log | tail -n 5

if [ $SUCCESS -eq 1 ]; then
    echo "Test Passed!"
else
    echo "FAILURE: Network stalled at View $MAX_VIEW."
    exit 1
fi
