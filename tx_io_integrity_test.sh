#!/usr/bin/env bash
set -euo pipefail

#############################################
# Ctrl+C / termination handling
#############################################

cleanup() {
    echo
    echo "🛑 Interrupt received — terminating all processes..."
    kill 0 2>/dev/null || true
    exit 130
}

trap cleanup INT TERM

PROFILE="release"
export LD_LIBRARY_PATH="./target/release${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"

echo "=== Running setup ==="

# Run setup and WAIT for it to exit
CARGO_BUILD_PROFILE=$PROFILE ./multinode-demo/setup.sh

echo "Setup completed ✅"

#############################################
# Start faucet AFTER setup
#############################################

echo "=== Starting faucet ==="

CARGO_BUILD_PROFILE=$PROFILE ./multinode-demo/faucet.sh &
FAUCET_PID=$!

echo "Faucet PID: $FAUCET_PID"

#############################################
# Wait for faucet to be ready
#############################################

echo "Waiting for faucet to start..."

until nc -z localhost 9900 2>/dev/null; do
    sleep 1
done

echo "Faucet is ready ✅"

#############################################
# Start bootstrap validator
#############################################

echo "=== Starting bootstrap validator ==="

CARGO_BUILD_PROFILE=$PROFILE ./multinode-demo/bootstrap-validator.sh \
    --log logs \
    --no-restart \
    --enable-rpc-transaction-history &
BOOTSTRAP_PID=$!

echo "Bootstrap PID: $BOOTSTRAP_PID"

#############################################
# Wait for validator RPC
#############################################

echo "Waiting for validator RPC..."

until nc -z localhost 8899 2>/dev/null; do
    sleep 1
done

echo "Validator is ready ✅"

#############################################
# Run bench-tps
#############################################

echo "=== Running bench-tps ==="

CARGO_BUILD_PROFILE=$PROFILE ./multinode-demo/bench-tps.sh \
    --tx-count 1000 \
    --duration 15

echo "bench-tps completed ✅"

#############################################
# Stop bootstrap validator
#############################################

echo "Sleeping 5 seconds..."
sleep 5

echo "Stopping bootstrap validator..."

./target/release/agave-validator \
    -l ./config/bootstrap-validator/ exit -f || true

#############################################
# Cleanup faucet
#############################################

echo "Cleaning up faucet..."

kill $FAUCET_PID 2>/dev/null || true

echo "=== Test completed ==="

#############################################
# Run TX I/O checker
#############################################

echo "=== Running tx_io_checker ==="

CHECK_OUTPUT=$(python3 tx_io_checker.py ./multinode-demo/tx_io.log || true)

echo "$CHECK_OUTPUT"

#############################################
# Validate output
#############################################

if [[ "$CHECK_OUTPUT" == *"Counts are equal"* ]] && \
   [[ "$CHECK_OUTPUT" == *"No excessive unmatched tx_in detected at the end."* ]]; then
    echo "✅ TEST PASSED"
    exit 0
else
    echo "❌ TEST FAILED"
    exit 1
fi
