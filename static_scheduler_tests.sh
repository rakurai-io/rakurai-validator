#!/bin/bash
set -e

BINARY="target/release/librakurai_scheduler_1_0.so"

echo "Test 1: Checking binary for file and network I/O through objdump..."

OUTPUT1=$(objdump -d "$BINARY" | grep -B2 -E 'mov\s+\$0x(00|01|02|03|04|05|06|11|12|15|29|2a|2b|2c|2d|2e|2f|31|32|33|34|36|37|52|57|101),%rax' || true)
if [[ -z "$OUTPUT1" ]]; then
    echo "Test 1 passed! No file or network I/O found."
    IO_TEST_PASSED=true
else
    echo "Test 1 failed! File or network I/O found."
    IO_TEST_PASSED=false
fi

echo "Test 2: Checking binary for validator private key access through objdump..."

OUTPUT2=$(objdump -x "$BINARY" | grep -i ClusterInfo || true)
if [[ -z "$OUTPUT2" ]]; then
    echo "Test 2 passed! No validator private key access found."
    PRIV_KEY_TEST_PASSED=true
else
    echo "Test 2 failed! Validator private key access found."
    PRIV_KEY_TEST_PASSED=false
fi

if [[ "$IO_TEST_PASSED" == true && "$PRIV_KEY_TEST_PASSED" == true ]]; then
    exit 0
else
    exit 1
fi