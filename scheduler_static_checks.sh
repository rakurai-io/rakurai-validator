#!/bin/bash
set -e

if [[ $# -lt 1 || -z "${1:-}" ]]; then
    echo "Usage: $0 <path-to-scheduler.so>" >&2
    exit 1
fi

BINARY="$1"

if [[ ! -f "$BINARY" ]]; then
    echo "Binary not found: $BINARY" >&2
    exit 1
fi

echo "Running static tests on: $BINARY"

echo "Test 1: Looking for calls to exec syscall, which loads binaries into running process..."

OUTPUT1=$(nm -D -C "$BINARY" | grep -E "exec" || true)
if [[ -z "$OUTPUT1" ]]; then
    echo "Test 1 passed! No calls to exec syscall found."
    EXEC_TEST_PASSED=true
else
    echo "Test 1 failed! Calls to exec syscall found."
    EXEC_TEST_PASSED=false
fi

echo "Test 2: Looking for any logic initiating processes..."

OUTPUT2=$(readelf -Ws -C "$BINARY" | grep "process::Command" || true)
if [[ -z "$OUTPUT2" ]]; then
    echo "Test 2 passed! No logic initiating processes found."
    PROCESS_TEST_PASSED=true
else
    echo "Test 2 failed! Logic initiating processes found."
    PROCESS_TEST_PASSED=false
fi

if [[ "$EXEC_TEST_PASSED" == true && "$PROCESS_TEST_PASSED" == true ]]; then
    exit 0
else
    exit 1
fi
