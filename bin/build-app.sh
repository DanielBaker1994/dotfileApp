#!/usr/bin/env bash
# build-app.sh — build the kitchen-sink .app from the Rust workspace (rust/).
#
#   build-app.sh            build if stale, else exit 0
#   build-app.sh --force    always rebuild
#   build-app.sh --stale    exit 0 when a rebuild is needed (no build)
#   build-app.sh --dist     self-contained bundle in .build/dist (resources,
#                           commands.default.toml, AX helpers)
#
# The app is the `kitchen-sink` binary of rust/ws-rs; the one Swift piece is the
# SwiftTerm shim crate (rust/swiftterm-shim), which links the pinned, prebuilt
# .build/SwiftTerm/libSwiftTerm.a (fetched by ensure-swiftterm.sh, compiled
# here only when the SwiftTerm checkout changes).
set -uo pipefail
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$DIR/.." && pwd)"
. "$ROOT/install.conf"

# NOTE: do NOT export MACOSX_DEPLOYMENT_TARGET=$MACOS_MIN here — the Homebrew
# rustc produces a corrupt proc-macro dylib for that target. The SwiftTerm
# min-OS is handled at link time (rust/.cargo/config.toml).

DIST=0
[ "${1:-}" = "--dist" ] && DIST=1
APP="$ROOT/$APP_NAME.app"
[ "$DIST" = 1 ] && APP="$ROOT/.build/dist/$APP_NAME.app"
BIN="$APP/Contents/MacOS/$APP_NAME"
WORKSPACE="$ROOT/rust"
CARGO_TARGET_DIR="$ROOT/.build/rust-target"
export CARGO_TARGET_DIR
"$DIR/check-macos.sh" || exit 1
TERM_SRC="$ROOT/$SWIFTTERM_DIR"
TERM_MOD_DIR="$ROOT/.build/SwiftTerm"
TERM_LIB="$TERM_MOD_DIR/libSwiftTerm.a"
TERM_SENTINEL="$ROOT/.build/.termbuilt"
TERM_TARGET="$(uname -m)-apple-macosx$MACOS_MIN"
TMP="${TMPDIR:-/tmp}"

command -v cargo >/dev/null 2>&1 || {
    echo "build-app: cargo not found — install Rust (rustup or 'brew install rust')" >&2; exit 1; }

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
    [ -f "$TERM_SRC/.ws-pinned" ] || return 0
    [ "$(cat "$TERM_SRC/.ws-pinned" 2>/dev/null)" = "$SWIFTTERM_PIN" ] || return 0
    [ -n "$(find "$WORKSPACE" \( -path "$WORKSPACE/target" -prune \) -o \
        \( -name '*.rs' -o -name '*.swift' -o -name 'Cargo.toml' -o -name 'Cargo.lock' -o -name '*.toml' \) \
        -newer "$BIN" -print 2>/dev/null | head -1)" ] && return 0
    local f
    for f in "$ROOT/Info.plist" "$ROOT/install.conf"; do
        [ "$f" -nt "$BIN" ] && return 0
    done
    return 1
}

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
PLIST="$BUILD_TMP.plist"
trap 'rm -f "$BUILD_TMP" "$PLIST" "$LOG"' EXIT

if ! (cd "$WORKSPACE" && cargo build --release -p ws-rs -p ws-helpers) >"$LOG" 2>&1; then
    grep -E 'error(\[|:)' -A4 "$LOG" >&2 || cat "$LOG" >&2
    echo "build-app: cargo build failed (log: $LOG)" >&2
    trap - EXIT
    rm -f "$BUILD_TMP" "$PLIST"
    exit 1
fi
cp "$CARGO_TARGET_DIR/release/$APP_NAME" "$BUILD_TMP" || {
    echo "build-app: no binary at $CARGO_TARGET_DIR/release/$APP_NAME" >&2; exit 1; }

cp "$ROOT/Info.plist" "$PLIST" || { echo "build-app: cannot read $ROOT/Info.plist" >&2; exit 1; }
pl_set() {
    /usr/libexec/PlistBuddy -c "Add :$1 $2 $3" "$PLIST" >/dev/null 2>&1 \
        || /usr/libexec/PlistBuddy -c "Set :$1 $3" "$PLIST" >/dev/null 2>&1
}
pl_set CFBundleShortVersionString string "$APP_VERSION"
pl_set CFBundleVersion string "$APP_VERSION"
pl_set LSMinimumSystemVersion string "$MACOS_MIN"
pl_set CFBundleIconFile string AppIcon

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

bundle_resources() {
    local res="$APP/Contents/Resources" d f h
    for d in $RESOURCE_LINK_DIRS $RESOURCE_SEED_DIRS; do
        [ "$d" = bin ] && continue
        [ -d "$ROOT/$d" ] || continue
        mkdir -p "$res/$d"
        (cd "$ROOT" && git ls-files -co --exclude-standard -- "$d") | while IFS= read -r f; do
            mkdir -p "$res/$(dirname "$f")"
            cp -p "$ROOT/$f" "$res/$f"
        done
    done
    mkdir -p "$res/bin"
    for f in $RESOURCE_BIN; do cp -p "$ROOT/bin/$f" "$res/bin/$f" || return 1; done
    for f in $RESOURCE_FILES; do cp -p "$ROOT/$f" "$res/$f" || return 1; done
    awk '
        /^\[/ { sec = $0 }
        sec == "[notes]" && /^paths[ \t]*=/ {
            print "paths = \"~/notes, ~/.config/kitchen-sink/commands.toml\""; next }
        sec == "[notes]" && /^pdf-css[ \t]*=/ { print "pdf-css = \"\""; next }   # the dotfiles style
        /^config[ \t]*=.*fake\.json/ { next }   # bin/fake-confluence.sh start
        /^vim-bin[ \t]*=/ { print "vim-bin = \"nvim\""; next }
        # Jira needs a site + token first: off until "Enable Jira" in the menu
        sec == "[jira]" && /^enabled[ \t]*=/ { print "enabled = false"; next }
        # /screenshot: the Flameshot defaults, not the owner flameshot.ini
        # (no apostrophes in here: this awk program is single-quoted)
        sec == "[screenshot]" && /^contrast-opacity[ \t]*=/ { print "contrast-opacity = 190"; next }
        sec == "[screenshot]" && /^draw-color[ \t]*=/ { print "draw-color = \"#ff0000\""; next }
        sec == "[screenshot]" && /^save-path[ \t]*=/ { print "save-path = \"~/Desktop\""; next }
        { print }' "$ROOT/commands.toml" > "$res/commands.default.toml"
    printf '%s\n' "$APP_VERSION" > "$res/VERSION"
    # The notify AX helpers (rust/ws-helpers), found by notify_poll.py.
    mkdir -p "$res/helpers-bin"
    for h in $HELPER_BINS; do
        cp -p "$CARGO_TARGET_DIR/release/$h" "$res/helpers-bin/$h" || {
            echo "build-app: helper $h missing from the cargo build" >&2; return 1; }
    done
    find "$res" -name '__pycache__' -type d -prune -exec rm -rf {} +
    find "$res" -name '.DS_Store' -delete
}

[ "$DIST" = 1 ] || pkill -f "$APP_NAME.app/Contents/MacOS" 2>/dev/null || true
mv "$BUILD_TMP" "$BIN"
chmod 755 "$BIN"
cp "$PLIST" "$APP/Contents/Info.plist"
rm -f "$PLIST"
build_icon
if [ "$DIST" = 1 ]; then
    bundle_resources || exit 1
fi
rm -f "$LOG"

clear_cstemp() { rm -f "$APP/Contents/MacOS/"*.cstemp; }
clear_cstemp
if [ "$DIST" = 1 ] && [ -n "${DEVELOPER_ID:-}" ]; then
    for f in "$APP/Contents/Resources/helpers-bin/"*; do
        codesign --force --options runtime --timestamp --sign "$DEVELOPER_ID" "$f" || exit 1
    done
    codesign --force --options runtime --timestamp --sign "$DEVELOPER_ID" \
        --identifier "$BUNDLE_ID" --entitlements "$ROOT/entitlements.plist" "$APP" || {
            echo "build-app: Developer ID signing failed ($DEVELOPER_ID)" >&2; exit 1; }
    exit 0
fi
perl -e 'alarm 30; exec @ARGV' codesign --force --sign "$SIGN_ID" --identifier "$BUNDLE_ID" "$APP" >/dev/null 2>&1 \
    || { echo "build-app: codesign with '$SIGN_ID' failed — run 'ws permissions fix'" >&2; exit 1; }
clear_cstemp

[ "$DIST" = 1 ] && exit 0

"$DIR/grant-permissions.sh" "$BUNDLE_ID" "$APP" >/dev/null 2>&1 || true
exit 0
