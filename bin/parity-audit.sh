#!/usr/bin/env bash
# One-shot Swift→Rust parity gap report (no UI, ~2-4 min, mostly cargo).
#
#   bin/parity-audit.sh [OUT.md]      default: .build/parity-audit.md
#
# Sections:
#   1. Dead code — Rust items written but never called (the crate has
#      `#![allow(dead_code)]`, so this is hidden in normal builds). Ported
#      models nobody wires up (VimKeys, PaneFocusRing, NoteFindModel…) show here.
#   2. Stub markers — "not modelled / not ported" logs and comments.
#   3. Socket `do:` verbs Swift handled (608d07d) vs Rust host.rs.
#   4. Verbs the nvim config sends to the app (vim/*.lua ask_app) vs Rust.
#   5. `[shortcuts]` rows per view (what Hyper+/ and Cmd+/ promise).
# A gap in 1-4 is a feature that looks done but is not.
set -euo pipefail
ROOT=$(git -C "$(dirname "$0")/.." rev-parse --show-toplevel)
cd "$ROOT"
SWIFT_REF=${SWIFT_REF:-608d07d}
OUT=${1:-.build/parity-audit.md}
mkdir -p "$(dirname "$OUT")" .build
SW=$(mktemp -d)
trap 'rm -rf "$SW"' EXIT
git archive "$SWIFT_REF" $(git ls-tree -r --name-only "$SWIFT_REF" | grep '\.swift$' | grep -v '^Tests/\|^rust/\|^notify/') | tar -x -C "$SW"
HOST=rust/ws-rs/src/app/host.rs

{
echo "# Parity audit — $(date '+%Y-%m-%d %H:%M') — Swift ref $SWIFT_REF"
echo

echo "## 1. Dead code (written, never called) by file"
echo
echo '```'
(cd rust && RUSTFLAGS="--force-warn dead_code" CARGO_TARGET_DIR="$ROOT/.build/deadcode-target" \
    cargo check -q -p ws-rs 2>&1 >/dev/null || true) > .build/deadcode.txt
grep -E '^\s+--> ' .build/deadcode.txt | sed -E 's#.*src/##; s#:.*##' | sort | uniq -c | sort -rn
echo '```'
echo
echo "<details><summary>All $(grep -c '^warning: .*never' .build/deadcode.txt || true) items</summary>"
echo
echo '```'
paste -d' ' <(grep -E '^warning: .*never' .build/deadcode.txt | sed 's/^warning: //') \
            <(grep -A1 -E '^warning: .*never' .build/deadcode.txt | grep -E '^\s+--> ' | sed -E 's#.*src/##')
echo '```'
echo "</details>"
echo

echo "## 2. Stub markers"
echo
echo '```'
rg -n -i 'not modelled|not ported|not yet ported|unimplemented!|todo!\(' rust/ws-rs/src | sed 's#rust/ws-rs/src/##' || true
echo '```'
echo

echo "## 3. Socket do: verbs — Swift vs Rust"
echo
start=$(grep -n 'if q.hasPrefix("do:")' "$SW/kitchen_sink.swift" | head -1 | cut -d: -f1)
swift_verbs=$(sed -n "${start},$((start+400))p" "$SW/kitchen_sink.swift" | grep -oE 'case "[a-z0-9:-]+"' | sed 's/case //; s/"//g' | sort -u)
stubbed=$(grep -E '=> \{$' -A6 "$HOST" | grep -B6 'not modelled' | grep -oE '"[a-z0-9:-]+"' | tr -d '"' | sort -u || true)
echo "| verb | Rust |"
echo "|---|---|"
for v in $swift_verbs; do
  if grep -qx "$v" <<<"$stubbed"; then s="❌ stub (logs only)"
  elif grep -q "\"$v\"" "$HOST" rust/ws-rs/src/app/registry.rs rust/ws-rs/src/views/*.rs; then s="✅ handled"
  else s="❌ missing"; fi
  echo "| \`$v\` | $s |"
done
echo

echo "## 4. nvim → app verbs (vim/*.lua)"
echo
echo "| verb | Rust |"
echo "|---|---|"
for v in $(grep -rhoE "ask_app\('[^']+'\)" vim | sed -E "s/ask_app\('//; s/'\)//" | sort -u); do
  if grep -qx "$v" <<<"$stubbed"; then s="❌ stub (logs only)"
  elif grep -q "\"$v\"" "$HOST"; then s="✅ handled"; else s="❌ missing"; fi
  echo "| \`$v\` | $s |"
done
echo

echo "## 5. [shortcuts] rows per view"
echo
echo '```'
sed -n '/^\[shortcuts\]/,/^\[/p' commands.toml | grep -oE '^"[a-z-]+:' | tr -d '":' | sort | uniq -c | sort -rn
echo '```'
echo
echo "Per-row behavior status lives in PARITY-shortcuts.md; a ✅ there must mean"
echo "verified in the live app (screenshot), not just routed."
} > "$OUT"

echo "wrote $OUT"
