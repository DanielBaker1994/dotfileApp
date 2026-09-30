#!/usr/bin/env bash
# build-app.sh — THE one build of workspace-switcher.app. INSTALL.sh and
# bin/workspace_switcher.sh (build.sh) both call this, so the compiled file
# list can never drift between them (install once missed the Jira files).
#
#   bin/build-app.sh            build if stale
#   bin/build-app.sh --force    always build
#   bin/build-app.sh --stale    exit 0 if a build is needed, 1 if up to date
#   bin/build-app.sh --dist     the self-contained bundle for the DMG, in
#                               .build/dist (code + default configs inside
#                               Contents/Resources). Never touches the dev
#                               bundle, the running daemon or TCC.
#
# Quiet on success; compiler errors go to stderr and the exit code is non-zero.
set -uo pipefail
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$DIR/.." && pwd)"
# shellcheck source=../install.conf
. "$ROOT/install.conf"

DIST=0
[ "${1:-}" = "--dist" ] && DIST=1
APP="$ROOT/$APP_NAME.app"
[ "$DIST" = 1 ] && APP="$ROOT/.build/dist/$APP_NAME.app"
BIN="$APP/Contents/MacOS/$APP_NAME"
# Refuse to build a target this Mac cannot run BEFORE compiling anything (a
# newer minos makes LaunchServices refuse the app, error -10825). Shared with
# INSTALL.sh so both doors give the same readable reason.
"$DIR/check-macos.sh" || exit 1
# SwiftTerm is fetched OUTSIDE the repo (SWIFTTERM_DIR, default ../SwiftTerm);
# this is the only place the checkout path is spelled.
TERM_SRC="$ROOT/$SWIFTTERM_DIR"
TERM_MOD_DIR="$ROOT/.build/SwiftTerm"
TERM_LIB="$TERM_MOD_DIR/libSwiftTerm.a"
TERM_SENTINEL="$ROOT/.build/.termbuilt"
# arch + min macOS the lib is built for; recorded in the sentinel so a
# MACOS_MIN change rebuilds it (a stale lib at a newer minos makes swiftc
# refuse the app build: "SwiftTerm has a minimum deployed target").
TERM_TARGET="$(uname -m)-apple-macosx$MACOS_MIN"
TMP="${TMPDIR:-/tmp}"

shopt -s nullglob
SOURCES=("$ROOT"/$SWIFT_SOURCES_GLOB)
shopt -u nullglob
[ "${#SOURCES[@]}" -gt 0 ] || { echo "build-app: no Swift sources in $ROOT" >&2; exit 1; }

# The archive's ACTUAL build target: LC_BUILD_VERSION's minos + its arch. The
# sentinel only records what a previous run *intended*; this reads the lib
# itself, so a hand-built / older / foreign-arch SwiftTerm is never reused.
lib_target_ok() {
    [ -f "$TERM_LIB" ] || return 1
    local minos
    minos="$(otool -l "$TERM_LIB" 2>/dev/null | awk '
        /LC_BUILD_VERSION/     { b = 1; next }
        b && $1 == "minos"     { print $2; exit }
        /LC_VERSION_MIN_MACOSX/ { v = 1; next }
        v && $1 == "version"   { print $2; exit }')"
    [ "$minos" = "$MACOS_MIN" ] || return 1
    lipo -info "$TERM_LIB" 2>/dev/null | grep -q "$(uname -m)"
}

stale() {
    [ -x "$BIN" ] && [ -f "$TERM_LIB" ] || return 0
    [ "$(cat "$TERM_SENTINEL" 2>/dev/null)" = "$TERM_TARGET" ] || return 0
    lib_target_ok || return 0
    # SwiftTerm is fetched at build time (bin/ensure-swiftterm.sh) — missing or
    # a different pin means this build needs to run.
    [ -f "$TERM_SRC/.ws-pinned" ] || return 0
    [ "$(cat "$TERM_SRC/.ws-pinned" 2>/dev/null)" = "$SWIFTTERM_PIN" ] || return 0
    local f
    for f in "${SOURCES[@]}" "$ROOT/Info.plist" "$ROOT/install.conf"; do
        [ "$f" -nt "$BIN" ] && return 0
    done
    return 1
}

# SwiftTerm (~230 files) is fetched at the pinned upstream commit + patch by
# bin/ensure-swiftterm.sh, then precompiled ONCE into a static lib + module;
# rebuilt only when one of its sources changes OR the build target changes.
build_term_lib() {
    [ -f "$TERM_SENTINEL" ] && \
        [ "$(cat "$TERM_SENTINEL" 2>/dev/null)" = "$TERM_TARGET" ] && \
        lib_target_ok && \
        [ -z "$(find "$TERM_SRC"/Sources "$TERM_SRC"/Generated \
            -name '*.swift' -newer "$TERM_SENTINEL" 2>/dev/null | head -1)" ] && return 0
    mkdir -p "$TERM_MOD_DIR"
    swiftc -O -swift-version 5 -target "$TERM_TARGET" -parse-as-library -emit-library -static -module-name SwiftTerm \
        "$TERM_SRC"/Sources/SwiftTerm/*.swift \
        "$TERM_SRC"/Sources/SwiftTerm/Apple/*.swift \
        "$TERM_SRC"/Sources/SwiftTerm/Apple/Metal/*.swift \
        "$TERM_SRC"/Sources/SwiftTerm/Mac/*.swift \
        "$TERM_SRC"/Sources/SwiftTerm/Portable/*.swift \
        "$TERM_SRC"/Generated/*.swift \
        -emit-module -emit-module-path "$TERM_MOD_DIR/SwiftTerm.swiftmodule" \
        -o "$TERM_LIB" >"$TERM_MOD_DIR/build.log" 2>&1 || {
            cat "$TERM_MOD_DIR/build.log" >&2
            echo "build-app: SwiftTerm failed to compile" >&2; return 1; }
    printf '%s\n' "$TERM_TARGET" > "$TERM_SENTINEL"
}

case "${1:-}" in
    --stale) stale; exit $? ;;
    --force|--dist) ;;
    "") stale || exit 0 ;;
    *) echo "usage: build-app.sh [--force|--stale|--dist]" >&2; exit 2 ;;
esac

"$DIR/ensure-swiftterm.sh" || exit 1
build_term_lib || exit 1

[ "$DIST" = 1 ] && rm -rf "$APP"
mkdir -p "$(dirname "$BIN")" "$APP/Contents/Resources"
BUILD_TMP="$(mktemp "$TMP/ws-build.XXXXXX")" || exit 1
LOG="$BUILD_TMP.log"

# Info.plist as shipped = the repo's + the values that live in install.conf
# (version, minimum macOS) + the icon; the same file is embedded in the
# binary and written into the bundle.
PLIST="$BUILD_TMP.plist"
cp "$ROOT/Info.plist" "$PLIST"
pl_set() {   # key type value
    /usr/libexec/PlistBuddy -c "Add :$1 $2 $3" "$PLIST" >/dev/null 2>&1 \
        || /usr/libexec/PlistBuddy -c "Set :$1 $3" "$PLIST" >/dev/null 2>&1
}
pl_set CFBundleShortVersionString string "$APP_VERSION"
pl_set CFBundleVersion string "$APP_VERSION"
pl_set LSMinimumSystemVersion string "$MACOS_MIN"
pl_set CFBundleIconFile string AppIcon

# Finder / Dock icon from app_icon.png (rebuilt only when the png changes)
build_icon() {
    local src="$ROOT/app_icon.png" out="$APP/Contents/Resources/AppIcon.icns" set n
    [ -f "$src" ] || return 0
    [ -f "$out" ] && [ ! "$src" -nt "$out" ] && return 0
    set="$(mktemp -d "$TMP/ws-icon.XXXXXX")/AppIcon.iconset"
    mkdir -p "$set"
    for n in 16 32 128 256; do
        sips -z "$n" "$n" "$src" --out "$set/icon_${n}x${n}.png" >/dev/null 2>&1
        sips -z $((n * 2)) $((n * 2)) "$src" --out "$set/icon_${n}x${n}@2x.png" >/dev/null 2>&1
    done
    sips -z 512 512 "$src" --out "$set/icon_512x512.png" >/dev/null 2>&1
    iconutil -c icns "$set" -o "$out" >/dev/null 2>&1
    rm -rf "$(dirname "$set")"
}

# --dist: everything the app needs at runtime goes INSIDE the bundle (lists in
# install.conf). An installed app reads its code from Contents/Resources and
# the user's files from ~/.config/workspace-switcher (bin/setup-home.sh).
bundle_resources() {
    local res="$APP/Contents/Resources" d f src
    for d in $RESOURCE_LINK_DIRS $RESOURCE_SEED_DIRS; do
        [ "$d" = bin ] && continue
        [ -d "$ROOT/$d" ] || continue
        mkdir -p "$res/$d"
        # tracked files only: never a cache, a log or an untracked scratch file
        (cd "$ROOT" && git ls-files -co --exclude-standard -- "$d") | while IFS= read -r f; do
            mkdir -p "$res/$(dirname "$f")"
            cp -p "$ROOT/$f" "$res/$f"
        done
    done
    mkdir -p "$res/bin"
    for f in $RESOURCE_BIN; do cp -p "$ROOT/bin/$f" "$res/bin/$f" || return 1; done
    for f in $RESOURCE_FILES; do cp -p "$ROOT/$f" "$res/$f" || return 1; done
    # the default config: the repo's, minus what is personal to this machine
    # (note tabs, the fake Confluence site, an absolute nvim path)
    awk '
        /^\[/ { sec = $0 }
        sec == "[notes]" && /^paths[ \t]*=/ {
            print "paths = \"~/notes, ~/.config/workspace-switcher/commands.toml\""; next }
        /^config[ \t]*=.*fake\.json/ { next }   # bin/fake-confluence.sh start
        /^vim-bin[ \t]*=/ { print "vim-bin = \"nvim\""; next }
        # Jira needs a site + token first: off until "Enable Jira" in the menu
        sec == "[jira]" && /^enabled[ \t]*=/ { print "enabled = false"; next }
        { print }' "$ROOT/commands.toml" > "$res/commands.default.toml"
    printf '%s\n' "$APP_VERSION" > "$res/VERSION"
    # sketchybar helpers, precompiled: an end user has no swiftc
    mkdir -p "$res/helpers-bin"
    for src in $HELPER_SOURCES; do
        swiftc -O -target "$(uname -m)-apple-macosx$MACOS_MIN" "$ROOT/$src" \
            -o "$res/helpers-bin/$(basename "$src" .swift)" >>"$LOG" 2>&1 || {
                cat "$LOG" >&2; echo "build-app: helper $src failed" >&2; return 1; }
    done
    find "$res" -name '__pycache__' -type d -prune -exec rm -rf {} +
    find "$res" -name '.DS_Store' -delete
}
compile() {
    swiftc "$@" -swift-version 5 -target "$(uname -m)-apple-macosx$MACOS_MIN" \
        -Xlinker -sectcreate -Xlinker __TEXT -Xlinker __info_plist -Xlinker "$PLIST" \
        -I "$TERM_MOD_DIR" -Xlinker "$TERM_LIB" \
        "${SOURCES[@]}" -o "$BUILD_TMP" >"$LOG" 2>&1
}
# unoptimized retry: works around an occasional -O compiler crash
if ! compile -O && ! compile -Onone; then
    grep -E 'error:' -A3 "$LOG" >&2 || cat "$LOG" >&2
    echo "build-app: compile failed (full log: $LOG)" >&2
    rm -f "$BUILD_TMP" "$PLIST"
    exit 1
fi

# a new binary means any RUNNING daemon is the old one (the dist bundle is
# never the running one)
[ "$DIST" = 1 ] || pkill -f "$APP_NAME.app/Contents/MacOS" 2>/dev/null || true
mv "$BUILD_TMP" "$BIN"
# the bundle's on-disk Info.plist is what LaunchServices reads for Finder
# services (NSServices) — keep it in sync with the embedded one
cp "$PLIST" "$APP/Contents/Info.plist"
rm -f "$PLIST"
build_icon
if [ "$DIST" = 1 ]; then
    bundle_resources || exit 1
fi
rm -f "$LOG"

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
# --dist with a Developer ID: hardened runtime + secure timestamp (what the
# notary service requires); code inside Resources is signed first.
if [ "$DIST" = 1 ] && [ -n "${DEVELOPER_ID:-}" ]; then
    for f in "$APP/Contents/Resources/helpers-bin/"*; do
        codesign --force --options runtime --timestamp --sign "$DEVELOPER_ID" "$f" || exit 1
    done
    codesign --force --options runtime --timestamp --sign "$DEVELOPER_ID" \
        --identifier "$BUNDLE_ID" --entitlements "$ROOT/entitlements.plist" "$APP" || {
            echo "build-app: Developer ID signing failed ($DEVELOPER_ID)" >&2; exit 1; }
    exit 0
fi
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
# (a dist build says so in bin/make-dmg.sh, and never touches TCC: an
# installed app gets the normal macOS permission prompts)
[ "$DIST" = 1 ] && exit 0
if [[ "$SIGINFO" != *"Authority=$SIGN_ID"* ]]; then
    echo "build-app: WARNING not signed with '$SIGN_ID' (ad-hoc) — privacy grants won't survive rebuilds" >&2
fi

# fresh signature — (re)grant every privacy permission the app uses (mic,
# speech, Downloads / Desktop / Documents) so no prompt interrupts you
"$DIR/grant-permissions.sh" "$BUNDLE_ID" "$APP" >/dev/null 2>&1 || true
exit 0
