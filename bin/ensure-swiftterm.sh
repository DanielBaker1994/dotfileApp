#!/usr/bin/env bash
set -uo pipefail
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$DIR/.." && pwd)"
. "$ROOT/install.conf"

DEST="$ROOT/$SWIFTTERM_DIR"
PATCH="$ROOT/$SWIFTTERM_PATCH"
PIN_FILE="$DEST/.ws-pinned"
TARGET="$(uname -m)-apple-macosx$MACOS_MIN"

if [ -f "$PIN_FILE" ] && [ "$(cat "$PIN_FILE" 2>/dev/null)" = "$SWIFTTERM_PIN" ] \
    && [ -f "$DEST/Sources/SwiftTerm/CellStorage.swift" ] \
    && [ -f "$DEST/Generated/GenTerminfo.swift" ] \
    && [ -f "$DEST/Generated/GenBuildInfo.swift" ]; then
    exit 0
fi

command -v git >/dev/null 2>&1 || { echo "ensure-swiftterm: git not found" >&2; exit 1; }
[ -f "$PATCH" ] || { echo "ensure-swiftterm: missing patch $SWIFTTERM_PATCH" >&2; exit 1; }

echo "ensure-swiftterm: fetching SwiftTerm $SWIFTTERM_PIN …" >&2

TMP="$(dirname "$DEST")/.swiftterm.tmp.$$"
rm -rf "$TMP"
mkdir -p "$(dirname "$TMP")"

if ! git init -q "$TMP" \
    || ! git -C "$TMP" remote add origin "$SWIFTTERM_URL" \
    || ! git -C "$TMP" fetch -q --depth 1 origin "$SWIFTTERM_PIN" \
    || ! git -C "$TMP" checkout -q FETCH_HEAD; then
    echo "ensure-swiftterm: could not fetch $SWIFTTERM_URL at $SWIFTTERM_PIN" >&2
    rm -rf "$TMP"
    exit 1
fi

if ! git -C "$TMP" apply "$PATCH"; then
    echo "ensure-swiftterm: patch $SWIFTTERM_PATCH did not apply to $SWIFTTERM_PIN" >&2
    rm -rf "$TMP"
    exit 1
fi

mkdir -p "$TMP/.ws-gen"
if ! swiftc -O -swift-version 5 -target "$TARGET" -o "$TMP/.ws-gen/genbuildinfo" \
        "$TMP"/Sources/SwiftTermBuildInfoGenerator/*.swift \
        || ! env SWIFTTERM_BUILD_BRANCH=main SWIFTTERM_BUILD_COMMIT="$SWIFTTERM_PIN" \
              SWIFTTERM_BUILD_DIRTY=false \
              "$TMP/.ws-gen/genbuildinfo" "$TMP" \
              "$TMP/Generated/GenBuildInfo.swift" "$TMP/Generated/GenTerminfo.swift"; then
    echo "ensure-swiftterm: could not generate SwiftTerm/Generated" >&2
    rm -rf "$TMP"
    exit 1
fi

rm -rf "$TMP/.ws-gen" "$TMP/.git"
find "$TMP/Sources" -mindepth 1 -maxdepth 1 ! -name SwiftTerm -exec rm -rf {} + 2>/dev/null
find "$TMP" -mindepth 1 -maxdepth 1 ! -name Sources ! -name Generated ! -name LICENSE \
    -exec rm -rf {} + 2>/dev/null
printf '%s\n' "$SWIFTTERM_PIN" > "$TMP/.ws-pinned"

rm -rf "$DEST"
mv "$TMP" "$DEST"
echo "ensure-swiftterm: ready ($SWIFTTERM_PIN + patch)" >&2
exit 0
