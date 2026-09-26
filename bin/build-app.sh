#!/usr/bin/env bash
# build-app.sh — THE one build of workspace-switcher.app. INSTALL.sh and
# bin/workspace_switcher.sh (build.sh) both call this, so the compiled file
# list can never drift between them (install once missed the Jira files).
#
#   bin/build-app.sh            build if stale
#   bin/build-app.sh --force    always build
#   bin/build-app.sh --stale    exit 0 if a build is needed, 1 if up to date
#
# Quiet on success; compiler errors go to stderr and the exit code is non-zero.
set -uo pipefail
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$DIR/.." && pwd)"
# shellcheck source=../install.conf
. "$ROOT/install.conf"

APP="$ROOT/$APP_NAME.app"
BIN="$APP/Contents/MacOS/$APP_NAME"
TERM_MOD_DIR="$ROOT/.build/SwiftTerm"
TERM_LIB="$TERM_MOD_DIR/libSwiftTerm.a"
TERM_SENTINEL="$ROOT/.build/.termbuilt"
TMP="${TMPDIR:-/tmp}"

shopt -s nullglob
SOURCES=("$ROOT"/$SWIFT_SOURCES_GLOB)
shopt -u nullglob
[ "${#SOURCES[@]}" -gt 0 ] || { echo "build-app: no Swift sources in $ROOT" >&2; exit 1; }

stale() {
    [ -x "$BIN" ] && [ -f "$TERM_LIB" ] || return 0
    local f
    for f in "${SOURCES[@]}" "$ROOT/Info.plist"; do
        [ "$f" -nt "$BIN" ] && return 0
    done
    return 1
}

# SwiftTerm (~230 files) is precompiled ONCE into a static lib + module;
# rebuilt only when one of its sources changes.
build_term_lib() {
    [ -f "$TERM_LIB" ] && [ -f "$TERM_SENTINEL" ] && \
        [ -z "$(find "$ROOT"/Vendor/SwiftTerm/Sources "$ROOT"/Vendor/SwiftTerm/Generated \
            -name '*.swift' -newer "$TERM_SENTINEL" 2>/dev/null | head -1)" ] && return 0
    mkdir -p "$TERM_MOD_DIR"
    swiftc -O -swift-version 5 -parse-as-library -emit-library -static -module-name SwiftTerm \
        "$ROOT"/Vendor/SwiftTerm/Sources/SwiftTerm/*.swift \
        "$ROOT"/Vendor/SwiftTerm/Sources/SwiftTerm/Apple/*.swift \
        "$ROOT"/Vendor/SwiftTerm/Sources/SwiftTerm/Apple/Metal/*.swift \
        "$ROOT"/Vendor/SwiftTerm/Sources/SwiftTerm/Mac/*.swift \
        "$ROOT"/Vendor/SwiftTerm/Sources/SwiftTerm/Portable/*.swift \
        "$ROOT"/Vendor/SwiftTerm/Generated/*.swift \
        -emit-module -emit-module-path "$TERM_MOD_DIR/SwiftTerm.swiftmodule" \
        -o "$TERM_LIB" >"$TERM_MOD_DIR/build.log" 2>&1 || {
            cat "$TERM_MOD_DIR/build.log" >&2
            echo "build-app: SwiftTerm failed to compile" >&2; return 1; }
    touch "$TERM_SENTINEL"
}

case "${1:-}" in
    --stale) stale; exit $? ;;
    --force) ;;
    "") stale || exit 0 ;;
    *) echo "usage: build-app.sh [--force|--stale]" >&2; exit 2 ;;
esac

build_term_lib || exit 1

mkdir -p "$(dirname "$BIN")"
BUILD_TMP="$(mktemp "$TMP/ws-build.XXXXXX")" || exit 1
LOG="$BUILD_TMP.log"
compile() {
    swiftc "$@" -swift-version 5 \
        -Xlinker -sectcreate -Xlinker __TEXT -Xlinker __info_plist -Xlinker "$ROOT/Info.plist" \
        -I "$TERM_MOD_DIR" -Xlinker "$TERM_LIB" \
        "${SOURCES[@]}" -o "$BUILD_TMP" >"$LOG" 2>&1
}
# unoptimized retry: works around an occasional -O compiler crash
if ! compile -O && ! compile -Onone; then
    grep -E 'error:' -A3 "$LOG" >&2 || cat "$LOG" >&2
    echo "build-app: compile failed (full log: $LOG)" >&2
    rm -f "$BUILD_TMP"
    exit 1
fi
rm -f "$LOG"

# a new binary means any RUNNING daemon is the old one
pkill -f "$APP_NAME.app/Contents/MacOS" 2>/dev/null || true
mv "$BUILD_TMP" "$BIN"
# the bundle's on-disk Info.plist is what LaunchServices reads for Finder
# services (NSServices) — keep it in sync with the embedded one
cp "$ROOT/Info.plist" "$APP/Contents/Info.plist"

# stable self-signed cert → TCC grants survive rebuilds (an ad-hoc cdhash
# changes every build). perl alarm = portable timeout: an unapproved key ACL
# pops a keychain dialog and would hang the build forever.
# A codesign killed mid-run (the alarm) leaves "<binary>.cstemp" in
# Contents/MacOS, and every later codesign of the bundle then FAILS on it
# ("invalid or unsupported format … .cstemp") — silently leaving the
# linker's throwaway ad-hoc signature, so macOS re-asked for every
# permission after each build. Clear it before (and between) attempts.
clear_cstemp() { rm -f "$APP/Contents/MacOS/"*.cstemp; }
clear_cstemp
signed=0
if security find-certificate -c "$SIGN_ID" >/dev/null 2>&1; then
    perl -e 'alarm 30; exec @ARGV' codesign --force --sign "$SIGN_ID" --identifier "$BUNDLE_ID" "$APP" >/dev/null 2>&1 \
        && signed=1
    clear_cstemp
fi
if [ "$signed" = 0 ]; then
    codesign --force --sign - --identifier "$BUNDLE_ID" "$APP" >/dev/null 2>&1
    clear_cstemp
fi
# say so when the stable signature didn't take: permissions then reset on
# every rebuild (macOS keys them to the signature)
# (captured first: under pipefail, `codesign | grep -q` reports a failure
# when grep stops reading early and codesign gets SIGPIPE)
SIGINFO="$(codesign -dvv "$APP" 2>&1)"
if [[ "$SIGINFO" != *"Authority=$SIGN_ID"* ]]; then
    echo "build-app: WARNING not signed with '$SIGN_ID' (ad-hoc) — privacy grants won't survive rebuilds" >&2
fi

# fresh signature — (re)grant every privacy permission the app uses (mic,
# speech, Downloads / Desktop / Documents) so no prompt interrupts you
"$DIR/grant-permissions.sh" "$BUNDLE_ID" "$APP" >/dev/null 2>&1 || true
exit 0
