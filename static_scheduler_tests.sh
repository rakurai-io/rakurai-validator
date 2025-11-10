#!/bin/bash
set -e

BINARY="target/release/librakurai_scheduler_1_0.so"

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