#!/bin/bash
# Clean up background processes on exit
trap "kill 0" EXIT

# Build
echo "Building..."
cargo build --quiet

# Start 4 Nodes (0, 1, 2, 3)
echo "Starting Node 0..."
RUST_LOG=info cargo run --quiet -- 0 > node0.log 2>&1 &

sleep 2

echo "Starting Node 1..."
RUST_LOG=info cargo run --quiet -- 1 > node1.log 2>&1 &

echo "Starting Node 2..."
RUST_LOG=info cargo run --quiet -- 2 > node2.log 2>&1 &

echo "Starting Node 3..."
RUST_LOG=info cargo run --quiet -- 3 > node3.log 2>&1 &

echo "Nodes started. Waiting for 60 seconds for mDNS discovery and consensus and finalization..."
sleep 60

echo "--- LOG SUMMARY ---"
echo "Node 0 (Head):"
head -n 5 node0.log
echo "..."
echo "Node 0 (Tail):"
tail -n 5 node0.log

echo "--- CONSENSUS CHECK ---"
QC_COUNT=$(grep "QC Formed" node*.log | wc -l)
BLOCK_COUNT=$(grep "Received Block" node*.log | wc -l)

echo "Total QCs Formed: $QC_COUNT"
echo "Total Blocks Received: $BLOCK_COUNT"

if [ $QC_COUNT -gt 0 ] || [ $BLOCK_COUNT -gt 0 ]; then
    echo "SUCCESS: Activity detected."
else
    echo "FAILURE: No consensus activity."
fi
