#!/bin/bash
set -e

BINARY="target/release/librakurai_scheduler_1_0.so"

echo "Running objdump verification..."

OUTPUT1=$(objdump -d "$BINARY" | grep -B2 -E 'mov\s+\$0x(00|01|02|03|04|05|06|11|12|15|29|2a|2b|2c|2d|2e|2f|31|32|33|34|36|37|52|57|101),%rax' || true)
OUTPUT2=$(objdump -x "$BINARY" | grep -i ClusterInfo || true)

OUTPUT="${OUTPUT1}${OUTPUT2}"

if [[ -z "$OUTPUT" ]]; then
    echo "objdump verification passed!"
    exit 0
else
    echo "objdump verification failed!"
    exit 1
fi
