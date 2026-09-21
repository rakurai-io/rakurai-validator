#!/usr/bin/env bash
# Check TIN gRPC surface on a host: Auth, Block Engine (bundles), P2C (Relayer).
#
# Usage:
#   ./scripts/check_tin_endpoint.sh <HOST:PORT> [BASE64_32BYTE_PUBKEY]
#
# Example:
#   ./scripts/check_tin_endpoint.sh ny.node1.me:30012 \
#     'ymp24tmfR4LFWNAjarZaPTTvvCwgIiBF+1dWQS60SBE='
#
# Requires: grpcurl, python3 (for base64 sanity check). Optional: PROTO_DIR.
# If grpcurl is missing, installs it (Go toolchain, brew, or GitHub release binary).
set -euo pipefail

TARGET="${1:-}"
PUBKEY_B64="${2:-ymp24tmfR4LFWNAjarZaPTTvvCwgIiBF+1dWQS60SBE=}"

if [[ -z "$TARGET" ]]; then
  echo "Usage: $0 <HOST:PORT> [BASE64_32BYTE_PUBKEY]" >&2
  exit 2
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# Prefer sibling tin_sample_servers/protos, else p2c-protos in monorepo, else PROTO_DIR.
if [[ -n "${PROTO_DIR:-}" ]]; then
  :
elif [[ -d "$SCRIPT_DIR/../protos" ]]; then
  PROTO_DIR="$SCRIPT_DIR/../protos"
elif [[ -d "$SCRIPT_DIR/../../rakurai_jito_private/p2c-protos/protos" ]]; then
  PROTO_DIR="$SCRIPT_DIR/../../rakurai_jito_private/p2c-protos/protos"
elif [[ -d "$SCRIPT_DIR/../p2c-protos/protos" ]]; then
  PROTO_DIR="$SCRIPT_DIR/../p2c-protos/protos"
else
  echo "Set PROTO_DIR to the directory that contains auth.proto / block_engine.proto" >&2
  exit 2
fi

PROTO_DIR="$(cd "$PROTO_DIR" && pwd)"
for f in auth.proto block_engine.proto packet.proto shared.proto; do
  if [[ ! -f "$PROTO_DIR/$f" ]]; then
    echo "Missing $PROTO_DIR/$f" >&2
    exit 2
  fi
done

# Resolve grpcurl; install into ~/.local/bin (or GOPATH/bin) when missing.
ensure_grpcurl() {
  if command -v grpcurl >/dev/null 2>&1; then
    return 0
  fi

  echo "grpcurl not found; installing..." >&2
  local install_dir="${GRPCURL_INSTALL_DIR:-$HOME/.local/bin}"
  mkdir -p "$install_dir"

  # 1) Go install (if toolchain present)
  if command -v go >/dev/null 2>&1; then
    echo "  → go install github.com/fullstorydev/grpcurl/cmd/grpcurl@latest" >&2
    if GOBIN="$install_dir" go install github.com/fullstorydev/grpcurl/cmd/grpcurl@latest; then
      export PATH="$install_dir:$PATH"
      if command -v grpcurl >/dev/null 2>&1; then
        echo "  ✓ installed to $install_dir/grpcurl" >&2
        return 0
      fi
    fi
  fi

  # 2) Homebrew
  if command -v brew >/dev/null 2>&1; then
    echo "  → brew install grpcurl" >&2
    if brew install grpcurl; then
      if command -v grpcurl >/dev/null 2>&1; then
        echo "  ✓ installed via brew" >&2
        return 0
      fi
    fi
  fi

  # 3) GitHub release binary (linux/darwin, amd64/arm64)
  if command -v curl >/dev/null 2>&1 || command -v wget >/dev/null 2>&1; then
    local os arch asset version url tmp
    os="$(uname -s | tr '[:upper:]' '[:lower:]')"
    arch="$(uname -m)"
    case "$arch" in
      x86_64|amd64) arch="x86_64" ;;
      aarch64|arm64) arch="arm64" ;;
      *)
        echo "  unsupported arch: $arch" >&2
        arch=""
        ;;
    esac
    case "$os" in
      linux|darwin) ;;
      *)
        echo "  unsupported OS: $os" >&2
        os=""
        ;;
    esac

    if [[ -n "$os" && -n "$arch" ]]; then
      # Pin to a known release; override with GRPCURL_VERSION=v1.9.3 etc.
      version="${GRPCURL_VERSION:-v1.9.3}"
      asset="grpcurl_${version#v}_${os}_${arch}.tar.gz"
      url="https://github.com/fullstorydev/grpcurl/releases/download/${version}/${asset}"
      tmp="$(mktemp -d)"
      echo "  → download $url" >&2
      if command -v curl >/dev/null 2>&1; then
        curl -fsSL "$url" -o "$tmp/$asset"
      else
        wget -qO "$tmp/$asset" "$url"
      fi
      if tar -xzf "$tmp/$asset" -C "$tmp" grpcurl 2>/dev/null \
        || tar -xzf "$tmp/$asset" -C "$tmp"; then
        if [[ -f "$tmp/grpcurl" ]]; then
          install -m 755 "$tmp/grpcurl" "$install_dir/grpcurl"
          export PATH="$install_dir:$PATH"
          rm -rf "$tmp"
          if command -v grpcurl >/dev/null 2>&1; then
            echo "  ✓ installed to $install_dir/grpcurl" >&2
            return 0
          fi
        fi
      fi
      rm -rf "$tmp"
    fi
  fi

  echo "Failed to install grpcurl automatically." >&2
  echo "Install manually, then re-run:" >&2
  echo "  # Go:" >&2
  echo "  go install github.com/fullstorydev/grpcurl/cmd/grpcurl@latest" >&2
  echo "  # or Homebrew: brew install grpcurl" >&2
  echo "  # or see https://github.com/fullstorydev/grpcurl/releases" >&2
  echo "Ensure the install dir is on PATH (default: \$HOME/.local/bin)." >&2
  exit 2
}

ensure_grpcurl

# Validate pubkey is 32 bytes when decoded.
PUBKEY_LEN="$(python3 -c "import base64,sys; d=base64.b64decode(sys.argv[1]); print(len(d))" "$PUBKEY_B64" 2>/dev/null || echo 0)"
if [[ "$PUBKEY_LEN" != "32" ]]; then
  echo "WARN: pubkey base64 does not decode to 32 bytes (got $PUBKEY_LEN). Auth challenge may fail." >&2
fi

PLAINTEXT=()
TLS_MODE="TLS"
# Use TLS if port looks like 443 or GRPC_TLS=1; otherwise plaintext (sample servers default).
if [[ "${GRPC_TLS:-0}" == "1" ]] || [[ "$TARGET" == *:443 ]]; then
  :
else
  PLAINTEXT=(-plaintext)
  TLS_MODE="plaintext"
fi

TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

pass=0
fail=0
skip=0
warn=0

results=() # lines: CATEGORY|RPC|STATUS|DETAIL

record() {
  local cat="$1" rpc="$2" status="$3" detail="$4"
  results+=("${cat}|${rpc}|${status}|${detail}")
  case "$status" in
    PASS) pass=$((pass + 1)) ;;
    FAIL) fail=$((fail + 1)) ;;
    SKIP) skip=$((skip + 1)) ;;
    WARN) warn=$((warn + 1)) ;;
  esac
}

# Run grpcurl; capture stdout+stderr; set global $rc.
# Usage: run_grpc <outfile> [grpcurl flags...] <METHOD>
# HOST:PORT ($TARGET) is inserted immediately before METHOD (grpcurl requires address before method).
run_grpc() {
  local out_file="$1"; shift
  if [[ $# -lt 1 ]]; then
    echo "run_grpc: missing METHOD" >&2
    rc=2
    return
  fi
  local method="${*: -1}"
  local -a args=()
  if [[ $# -gt 1 ]]; then
    args=("${@:1:$#-1}")
  fi
  set +e
  grpcurl "${PLAINTEXT[@]}" -import-path "$PROTO_DIR" \
    "${args[@]}" "$TARGET" "$method" >"$out_file" 2>&1
  rc=$?
  set -e
}

classify_error() {
  local body="$1"
  if grep -qiE 'Unimplemented|unknown service|Method not found' <<<"$body"; then
    echo "UNIMPLEMENTED"
  elif grep -qiE 'Unauthenticated|PermissionDenied|authorization|Bearer|denied' <<<"$body"; then
    echo "UNAUTH"
  elif grep -qiE 'Failed to dial|Unavailable|connection refused|dial tcp|deadline|timeout|reset by peer|transport: error|certificate|handshake|connection error' <<<"$body"; then
    echo "UNREACHABLE"
  else
    echo "ERROR"
  fi
}

short_detail() {
  # One-line trim
  tr '\n' ' ' <<<"$1" | sed 's/  */ /g' | cut -c1-120
}

echo "=== TIN endpoint check ==="
echo "Target:    $TARGET"
echo "Proto dir: $PROTO_DIR"
echo "Transport: $TLS_MODE  (set GRPC_TLS=1 for TLS, unset for -plaintext)"
echo "Pubkey:    ${PUBKEY_B64:0:20}… (${PUBKEY_LEN} bytes decoded)"
echo

# --- Auth ---
auth_out="$TMPDIR/auth_relayer.json"
run_grpc "$auth_out" -proto auth.proto \
  -d "{\"role\":\"RELAYER\",\"pubkey\":\"$PUBKEY_B64\"}" \
  auth.AuthService/GenerateAuthChallenge
if [[ $rc -eq 0 ]] && grep -q challenge "$auth_out"; then
  record "Auth" "AuthService/GenerateAuthChallenge (RELAYER)" "PASS" "$(short_detail "$(cat "$auth_out")")"
else
  kind="$(classify_error "$(cat "$auth_out")")"
  if [[ "$kind" == "UNIMPLEMENTED" ]]; then
    record "Auth" "AuthService/GenerateAuthChallenge (RELAYER)" "FAIL" "AuthService missing — both Bundles and P2C need it"
  else
    record "Auth" "AuthService/GenerateAuthChallenge (RELAYER)" "FAIL" "$kind: $(short_detail "$(cat "$auth_out")")"
  fi
fi

auth_out_v="$TMPDIR/auth_validator.json"
run_grpc "$auth_out_v" -proto auth.proto \
  -d "{\"role\":\"VALIDATOR\",\"pubkey\":\"$PUBKEY_B64\"}" \
  auth.AuthService/GenerateAuthChallenge
if [[ $rc -eq 0 ]] && grep -q challenge "$auth_out_v"; then
  record "Auth" "AuthService/GenerateAuthChallenge (VALIDATOR)" "PASS" "$(short_detail "$(cat "$auth_out_v")")"
else
  kind="$(classify_error "$(cat "$auth_out_v")")"
  record "Auth" "AuthService/GenerateAuthChallenge (VALIDATOR)" "FAIL" "$kind: $(short_detail "$(cat "$auth_out_v")")"
fi

record "Auth" "AuthService/GenerateAuthTokens" "SKIP" "Needs signed challenge with identity keypair (not checked here)"

# --- Block Engine (bundles / discovery) ---
be_out="$TMPDIR/endpoints.json"
run_grpc "$be_out" -proto block_engine.proto \
  block_engine.BlockEngineValidator/GetBlockEngineEndpoints
if [[ $rc -eq 0 ]]; then
  record "BlockEngine" "BlockEngineValidator/GetBlockEngineEndpoints" "PASS" "$(short_detail "$(cat "$be_out")")"
else
  kind="$(classify_error "$(cat "$be_out")")"
  record "BlockEngine" "BlockEngineValidator/GetBlockEngineEndpoints" "FAIL" "$kind: $(short_detail "$(cat "$be_out")")"
fi

# Subscribe streams need Bearer VALIDATOR token + hang open — probe for presence only.
# Without token, UNAUTH usually means method exists; UNIMPLEMENTED means missing.
for rpc in SubscribePackets SubscribeBundles; do
  sub_out="$TMPDIR/${rpc}.txt"
  # Unary-style attempt; bi-di will fail, but error text still classifies.
  run_grpc "$sub_out" -proto block_engine.proto -d '{}' \
    "block_engine.BlockEngineValidator/${rpc}" || true
  body="$(cat "$sub_out")"
  kind="$(classify_error "$body")"
  if [[ "$kind" == "UNREACHABLE" ]]; then
    record "BlockEngine" "BlockEngineValidator/${rpc}" "FAIL" "UNREACHABLE: $(short_detail "$body")"
  elif [[ "$kind" == "UNIMPLEMENTED" ]]; then
    record "BlockEngine" "BlockEngineValidator/${rpc}" "WARN" "UNIMPLEMENTED — OK on P2C-only host; required for bundles"
  elif [[ "$kind" == "UNAUTH" ]]; then
    record "BlockEngine" "BlockEngineValidator/${rpc}" "PASS" "RPC present (auth required to stream — expected without Bearer)"
  elif [[ $rc -eq 0 ]]; then
    record "BlockEngine" "BlockEngineValidator/${rpc}" "PASS" "Accepted (unexpected without auth, but method exists)"
  else
    record "BlockEngine" "BlockEngineValidator/${rpc}" "WARN" "$kind: $(short_detail "$body")"
  fi
done

# --- P2C Relayer ---
for rpc in StartExpiringPacketStream StartExpiringMevPacketStream StartExpiringTpuPacketStream StartP2cUpdateCountStream; do
  r_out="$TMPDIR/${rpc}.txt"
  # Bi-di: send one empty-ish message then close. Classify by error.
  if [[ "$rpc" == "StartP2cUpdateCountStream" ]]; then
    payload='{}'
  else
    payload='{"heartbeat":{"count":1}}'
  fi
  run_grpc "$r_out" \
    -proto block_engine.proto -proto packet.proto -proto shared.proto \
    -d "$payload" \
    "block_engine.BlockEngineRelayer/${rpc}" || true
  body="$(cat "$r_out")"
  kind="$(classify_error "$body")"

  required="optional"
  [[ "$rpc" == "StartExpiringPacketStream" || "$rpc" == "StartExpiringMevPacketStream" ]] && required="required"

  if [[ "$kind" == "UNREACHABLE" ]]; then
    record "P2C" "BlockEngineRelayer/${rpc}" "FAIL" "UNREACHABLE: $(short_detail "$body")"
  elif [[ "$kind" == "UNIMPLEMENTED" ]]; then
    if [[ "$required" == "required" ]]; then
      if [[ "$rpc" == "StartExpiringPacketStream" ]]; then
        record "P2C" "BlockEngineRelayer/${rpc}" "FAIL" "UNIMPLEMENTED — required for ReSell / PSA endpoints"
      else
        record "P2C" "BlockEngineRelayer/${rpc}" "FAIL" "UNIMPLEMENTED — required for Mev / MCA endpoints"
      fi
    else
      record "P2C" "BlockEngineRelayer/${rpc}" "WARN" "UNIMPLEMENTED — optional (OK if you skip TPU/counts)"
    fi
  elif [[ "$kind" == "UNAUTH" ]]; then
    record "P2C" "BlockEngineRelayer/${rpc}" "PASS" "RPC present (needs RELAYER Bearer to stream)"
  elif [[ $rc -eq 0 ]]; then
    record "P2C" "BlockEngineRelayer/${rpc}" "PASS" "Stream accepted"
  else
    record "P2C" "BlockEngineRelayer/${rpc}" "FAIL" "$kind: $(short_detail "$body")"
  fi
done

# --- Print summary ---
print_cat() {
  local want="$1"
  echo
  echo "── $want ──"
  printf '%-55s %-6s %s\n' "RPC" "STATUS" "DETAIL"
  printf '%-55s %-6s %s\n' "---" "------" "------"
  local line cat rpc status detail
  for line in "${results[@]}"; do
    IFS='|' read -r cat rpc status detail <<<"$line"
    [[ "$cat" == "$want" ]] || continue
    printf '%-55s %-6s %s\n' "$rpc" "$status" "$detail"
  done
}

print_cat "Auth"
print_cat "BlockEngine"
print_cat "P2C"

echo
echo "=== Summary ==="
echo "PASS=$pass  FAIL=$fail  WARN=$warn  SKIP=$skip"
echo
echo "Interpretation:"
echo "  • UNREACHABLE = TCP/TLS dial failed — fix host/port/firewall/TLS before auth or streams"
echo "  • Auth RELAYER + VALIDATOR challenges → host can auth both paths (signer only needed for tokens)"
echo "  • BlockEngine GetBlockEngineEndpoints → discovery OK"
echo "  • Subscribe* PASS/UNAUTH → bundles surface present; UNIMPLEMENTED → P2C-only OK"
echo "  • StartExpiringPacketStream required for ReSell/PSA; StartExpiringMevPacketStream required for Mev/MCA; TPU (Mev flag) / count optional"
echo "  • Full stream I/O needs Bearer after GenerateAuthTokens (sign challenge with identity key)"
echo
if [[ "$fail" -gt 0 ]] && printf '%s\n' "${results[@]}" | grep -q 'UNREACHABLE'; then
  if [[ "$TLS_MODE" == "TLS" ]]; then
    echo "Hint: you used TLS. Sample servers are usually plaintext — retry without GRPC_TLS=1:"
    echo "  ./scripts/check_tin_endpoint.sh $TARGET '<pubkey>'"
    echo
  else
    echo "Hint: dial failed on plaintext — if the host expects TLS, retry:"
    echo "  GRPC_TLS=1 ./scripts/check_tin_endpoint.sh $TARGET '<pubkey>'"
    echo
  fi
fi

# Exit non-zero if any hard FAIL
if [[ "$fail" -gt 0 ]]; then
  exit 1
fi
exit 0
