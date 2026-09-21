#!/usr/bin/env bash
set -euo pipefail

#############################################
# Process tracking / cleanup
#############################################

FAUCET_PID=""
BOOTSTRAP_PID=""
CLEANING_UP=0

cleanup() {
    local exit_code=${1:-$?}
    if [[ $CLEANING_UP -eq 1 ]]; then
        return
    fi
    CLEANING_UP=1

    # Disable further ERR traps so cleanup itself cannot recurse.
    trap - INT TERM ERR EXIT

    echo
    echo "🛑 Cleaning up started processes (exit=$exit_code)..."

    if [[ -n "${BOOTSTRAP_PID}" ]] && kill -0 "$BOOTSTRAP_PID" 2>/dev/null; then
        kill "$BOOTSTRAP_PID" 2>/dev/null || true
        # Also try a graceful validator exit if the binary exists.
        if [[ -x ./target/release/agave-validator ]]; then
            ./target/release/agave-validator \
                -l ./config/bootstrap-validator/ exit -f 2>/dev/null || true
        fi
        wait "$BOOTSTRAP_PID" 2>/dev/null || true
    fi

    if [[ -n "${FAUCET_PID}" ]] && kill -0 "$FAUCET_PID" 2>/dev/null; then
        kill "$FAUCET_PID" 2>/dev/null || true
        wait "$FAUCET_PID" 2>/dev/null || true
    fi

    # Kill any remaining children in this process group.
    kill 0 2>/dev/null || true

    exit "$exit_code"
}

on_signal() {
    cleanup 130
}

on_error() {
    local exit_code=$?
    echo "❌ Command failed (exit=$exit_code) at line ${BASH_LINENO[0]}: ${BASH_COMMAND}"
    cleanup "$exit_code"
}

trap on_signal INT TERM
trap on_error ERR
trap 'cleanup $?' EXIT

# Wait until host:port is open, or fail if a watched PID dies.
wait_for_port() {
    local host=$1
    local port=$2
    local watched_pid=${3:-}
    local label=${4:-service}

    echo "Waiting for ${label} on ${host}:${port}..."
    while true; do
        if nc -z "$host" "$port" 2>/dev/null; then
            echo "${label} is ready ✅"
            return 0
        fi
        if [[ -n "$watched_pid" ]]; then
            if ! kill -0 "$watched_pid" 2>/dev/null; then
                wait "$watched_pid" 2>/dev/null || true
                echo "❌ ${label} process (pid=${watched_pid}) exited before ${host}:${port} became ready"
                return 1
            fi
        fi
        sleep 1
    done
}

PROFILE="release"
export LD_LIBRARY_PATH="./target/release${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"

echo "=== Cleaning previous localnet state ==="
rm -rf ./config/ ./multinode-demo/tx_io.log ./tx_io_check_local_validator.log

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

wait_for_port localhost 9900 "$FAUCET_PID" "faucet"

#############################################
# Start bootstrap validator
#############################################

echo "=== Starting bootstrap validator ==="

CARGO_BUILD_PROFILE=$PROFILE ./multinode-demo/bootstrap-validator.sh \
    --log tx_io_check_local_validator.log \
    --no-restart \
    --enable-rpc-transaction-history &
BOOTSTRAP_PID=$!

echo "Bootstrap PID: $BOOTSTRAP_PID"

wait_for_port localhost 8899 "$BOOTSTRAP_PID" "validator RPC"

#############################################
# Run bench-tps
#############################################

echo "=== Running bench-tps ==="
CARGO_BUILD_PROFILE=$PROFILE ./multinode-demo/txs-bench.sh \
    --target-tps 500 \
    --duration 10

echo "bench-tps completed ✅"

#############################################
# Stop bootstrap validator
#############################################

echo "Sleeping 5 seconds..."
sleep 5

echo "Stopping bootstrap validator..."

./target/release/agave-validator \
    -l ./config/bootstrap-validator/ exit -f || true

if [[ -n "${BOOTSTRAP_PID}" ]]; then
    wait "$BOOTSTRAP_PID" 2>/dev/null || true
    BOOTSTRAP_PID=""
fi

#############################################
# Cleanup faucet
#############################################

echo "Cleaning up faucet..."

if [[ -n "${FAUCET_PID}" ]]; then
    kill "$FAUCET_PID" 2>/dev/null || true
    wait "$FAUCET_PID" 2>/dev/null || true
    FAUCET_PID=""
fi

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

trap - EXIT

if [[ "$CHECK_OUTPUT" == *"Counts are equal"* ]] && \
   [[ "$CHECK_OUTPUT" == *"No excessive unmatched tx_in detected at the end."* ]]; then
    echo "✅ TEST PASSED"
    exit 0
else
    echo "❌ TEST FAILED"
    exit 1
fi
