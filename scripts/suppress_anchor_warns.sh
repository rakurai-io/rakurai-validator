#!/usr/bin/env bash
# Suppress compiler warnings for anchor submodule crates on stable Cargo.
set -euo pipefail

real_rustc=$1
shift

is_anchor_crate=0
prev_arg=""
for arg in "$@"; do
    if [[ $prev_arg == "--crate-name" && $arg == anchor_* ]]; then
        is_anchor_crate=1
        break
    fi
    prev_arg=$arg
done

if [[ $is_anchor_crate -eq 1 ]]; then
    exec "$real_rustc" -Awarnings "$@"
fi

exec "$real_rustc" "$@"
