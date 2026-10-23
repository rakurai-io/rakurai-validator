#!/usr/bin/env bash
#
# Static checks asserting the scheduler library cannot reach outside the
# validator process: no file I/O, no network I/O, no access to signing material.
#
# Fail-closed throughout: a missing binary or an unavailable tool exits non-zero
# rather than reporting a pass.
#
# Tests 1 and 4 block. Tests 2 and 3 are advisory until their allowlists have
# been calibrated against a released build, promote them by swapping warn() for
# fail() at their call sites.
#
# Usage: static_scheduler_tests.sh [path-to-scheduler.so]

set -euo pipefail

FAILURES=()

fail() {
    echo "  FAILED: $1"
    FAILURES+=("$1")
}

pass() {
    echo "  passed: $1"
}

# Reported but does not fail the run.
warn() {
    echo "  ADVISORY: $1"
}

# --- Resolve the binary ----------------------------------------------------

resolve_binary() {
    if [[ $# -ge 1 && -n "$1" ]]; then
        echo "$1"
        return
    fi

    # Accept either name: the deployment step renames librakurai_scheduler.so to
    # the versioned librak_scheduler_<version>.so that core/build.rs links.
    local candidates=()
    local candidate
    for candidate in target/release/librak_scheduler_*.so \
                     target/release/librakurai_scheduler.so; do
        [[ -f "$candidate" ]] && candidates+=("$candidate")
    done

    if [[ ${#candidates[@]} -eq 0 ]]; then
        echo "No scheduler library found under target/release." >&2
        echo "Looked for librak_scheduler_*.so and librakurai_scheduler.so." >&2
        exit 1
    fi

    if [[ ${#candidates[@]} -gt 1 ]]; then
        echo "Multiple scheduler libraries found; pass the one to test explicitly:" >&2
        printf '  %s\n' "${candidates[@]}" >&2
        exit 1
    fi

    echo "${candidates[0]}"
}

BINARY="$(resolve_binary "${1:-}")"

if [[ ! -f "$BINARY" ]]; then
    echo "Scheduler library not found: $BINARY" >&2
    exit 1
fi

if ! command -v objdump >/dev/null 2>&1; then
    echo "objdump not found on PATH; cannot run static checks." >&2
    exit 1
fi

echo "Testing $BINARY"
echo

# Dumped once. Disassembly and headers are required, so a failure here aborts:
# an unreadable binary must never be reported as clean.
DISASM="$(objdump -d "$BINARY")"
DYNSYMS="$(objdump -T "$BINARY" 2>/dev/null || true)"
ALLSYMS="$(objdump -t "$BINARY" 2>/dev/null || true)"
HEADERS="$(objdump -p "$BINARY")"

# Test 1: direct file or network I/O syscalls

# x86-64 numbers for the file and socket syscalls: read/write/open/close, stat
# family, pread/pwrite, access, the socket calls, rename, unlink, openat.
SYSCALL_NUMS='0|1|2|3|4|5|6|11|12|15|29|2a|2b|2c|2d|2e|2f|31|32|33|34|36|37|52|57|101'
SYSCALL_WINDOW=4

echo "Test 1: direct file or network I/O syscalls"

# Require the number load *and* a nearby syscall instruction. Matching the
# immediate alone flags ordinary constants, as mov $0x1,%rax is everywhere.
SYSCALL_HITS="$(
    printf '%s\n' "$DISASM" | awk -v nums="$SYSCALL_NUMS" -v window="$SYSCALL_WINDOW" '
        # Count only instruction lines, so section headers and blank lines
        # cannot push a load out of range.
        /^[[:space:]]*[0-9a-f]+:/ {
            n += 1
            recent[n] = $0

            if ($0 ~ /[[:space:]]syscall([[:space:]]|$)/) {
                for (i = n - 1; i >= 1 && i >= n - window; i--) {
                    if (recent[i] ~ "mov[[:space:]]+\\$0x(" nums ")," "%(r|e)ax") {
                        print recent[i]
                        print $0
                        break
                    }
                }
            }

            delete recent[n - window - 1]
        }
    '
)"

if [[ -z "$SYSCALL_HITS" ]]; then
    pass "no direct file or network I/O syscalls"
else
    fail "direct file or network I/O syscalls found"
    printf '%s\n' "$SYSCALL_HITS" | sed 's/^/    /'
fi

# Test 2: imported file or network I/O functions (advisory)

# A Rust cdylib reaches the kernel through libc rather than inline syscalls, so
# Test 1 cannot see that I/O. Undefined (*UND*) dynamic symbols are the imports.
IO_IMPORTS='^(open|open64|openat|creat|read|pread|pread64|write|pwrite|pwrite64|fopen|fopen64|freopen|fread|fwrite|opendir|readdir|mkdir|rename|unlink|remove|truncate|ftruncate|socket|socketpair|connect|bind|listen|accept|accept4|send|sendto|sendmsg|recv|recvfrom|recvmsg|getaddrinfo|gethostbyname|setsockopt|getsockopt|shutdown|dlopen|execve|execvp|fork|posix_spawn|system|popen)$'

echo "Test 2: imported file or network I/O functions (advisory)"

IMPORT_HITS="$(
    printf '%s\n' "$DYNSYMS" \
        | awk '$0 ~ /\*UND\*/ { print $NF }' \
        | sed 's/@@.*$//; s/@.*$//' \
        | grep -E "$IO_IMPORTS" \
        | sort -u \
        || true
)"

if [[ -z "$IMPORT_HITS" ]]; then
    pass "no file or network I/O functions imported"
else
    warn "file or network I/O functions imported"
    printf '%s\n' "$IMPORT_HITS" | sed 's/^/    /'
fi

# Test 3: shared library dependencies (advisory)

# DT_NEEDED names every library loaded alongside this one. Anything past the
# base runtime, such as a TLS stack or an HTTP client, is capability it should
# not have.
ALLOWED_NEEDED='^(libc|libm|libdl|libpthread|librt|libgcc_s|ld-linux-x86-64|ld-linux|libstdc\+\+)\.so'

echo "Test 3: shared library dependencies (advisory)"

NEEDED_HITS="$(
    printf '%s\n' "$HEADERS" \
        | awk '$1 == "NEEDED" { print $2 }' \
        | grep -Ev "$ALLOWED_NEEDED" \
        | sort -u \
        || true
)"

if [[ -z "$NEEDED_HITS" ]]; then
    pass "no unexpected shared library dependencies"
else
    warn "unexpected shared library dependencies"
    printf '%s\n' "$NEEDED_HITS" | sed 's/^/    /'
fi

# Test 4: access to validator signing material

# The scheduler is handed ContactInfo, never the identity keypair. These symbols
# would mean it was compiled against the types that hold or read signing keys.
KEY_SYMBOLS='ClusterInfo|Keypair|keypair|read_keypair|signer_from_path|sign_message'

echo "Test 4: access to validator signing material"

KEY_HITS="$(
    printf '%s\n%s\n' "$DYNSYMS" "$ALLSYMS" \
        | grep -Eo "$KEY_SYMBOLS" \
        | sort -u \
        || true
)"

if [[ -z "$KEY_HITS" ]]; then
    pass "no validator signing material access"
else
    fail "validator signing material access"
    printf '%s\n' "$KEY_HITS" | sed 's/^/    /'
fi

#Summary
echo
if [[ ${#FAILURES[@]} -eq 0 ]]; then
    echo "All blocking static scheduler checks passed."
    exit 0
fi

echo "${#FAILURES[@]} blocking static scheduler check(s) failed:"
printf '  - %s\n' "${FAILURES[@]}"
exit 1
