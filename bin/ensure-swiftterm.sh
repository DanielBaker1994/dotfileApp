#!/usr/bin/env bash
# ensure-swiftterm.sh — materialize the embedded terminal library.
#
# SwiftTerm is NOT committed to this repo (it used to be 6 MB of vendored
# code). This script shallow-clones upstream at the pinned commit from
# install.conf, applies patches/swiftterm-cellstorage-cache.patch (the perf
# fix that makes up the copy this app was tested with) and runs upstream's
# own generator for Generated/{GenBuildInfo,GenTerminfo}.swift — the two
# files bin/build-app.sh compiles. The result lands in Vendor/SwiftTerm/
# (gitignored) with a .ws-pinned marker; a run that finds the marker matching
# SWIFTTERM_PIN is a no-op.
#
# Called by bin/build-app.sh (and jira-doctor.sh) before the SwiftTerm build.
# Needs network only when the pinned checkout is missing/wrong.
set -uo pipefail
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$DIR/.." && pwd)"
# shellcheck source=../install.conf
. "$ROOT/install.conf"

DEST="$ROOT/$SWIFTTERM_DIR"
PATCH="$ROOT/$SWIFTTERM_PATCH"
PIN_FILE="$DEST/.ws-pinned"
TARGET="$(uname -m)-apple-macosx$MACOS_MIN"

# already materialized at the right pin?
if [ -f "$PIN_FILE" ] && [ "$(cat "$PIN_FILE" 2>/dev/null)" = "$SWIFTTERM_PIN" ] \
    && [ -f "$DEST/Sources/SwiftTerm/CellStorage.swift" ] \
    && [ -f "$DEST/Generated/GenTerminfo.swift" ] \
    && [ -f "$DEST/Generated/GenBuildInfo.swift" ]; then
    exit 0
fi

command -v git >/dev/null 2>&1 || { echo "ensure-swiftterm: git not found" >&2; exit 1; }
[ -f "$PATCH" ] || { echo "ensure-swiftterm: missing patch $SWIFTTERM_PATCH" >&2; exit 1; }

echo "ensure-swiftterm: fetching SwiftTerm $SWIFTTERM_PIN …" >&2

TMP="$ROOT/Vendor/.swiftterm.tmp.$$"
rm -rf "$TMP"
mkdir -p "$ROOT/Vendor"

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

# upstream generates Generated/*.swift from its own git info + swifterm-terminfo
# at build time; we run the generator directly so the hand-rolled swiftc build
# gets those two files. Env vars make the output deterministic (no git needed).
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

# keep only what the swiftc build compiles (+ the MIT license): upstream also
# ships Tests/ (2.6M), TerminalApp/, Tools/ … that this app never touches
rm -rf "$TMP/.ws-gen" "$TMP/.git"
find "$TMP/Sources" -mindepth 1 -maxdepth 1 ! -name SwiftTerm -exec rm -rf {} + 2>/dev/null
find "$TMP" -mindepth 1 -maxdepth 1 ! -name Sources ! -name Generated ! -name LICENSE \
    -exec rm -rf {} + 2>/dev/null
printf '%s\n' "$SWIFTTERM_PIN" > "$TMP/.ws-pinned"

rm -rf "$DEST"
mv "$TMP" "$DEST"
echo "ensure-swiftterm: ready ($SWIFTTERM_PIN + patch)" >&2
exit 0
