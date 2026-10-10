#!/usr/bin/env bash
set -uo pipefail
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$DIR/.." && pwd)"
. "$ROOT/install.conf"

DIST=0
[ "${1:-}" = "--dist" ] && DIST=1
APP="$ROOT/$APP_NAME.app"
[ "$DIST" = 1 ] && APP="$ROOT/.build/dist/$APP_NAME.app"
BIN="$APP/Contents/MacOS/$APP_NAME"
"$DIR/check-macos.sh" || exit 1
TERM_SRC="$ROOT/$SWIFTTERM_DIR"
TERM_MOD_DIR="$ROOT/.build/SwiftTerm"
TERM_LIB="$TERM_MOD_DIR/libSwiftTerm.a"
TERM_SENTINEL="$ROOT/.build/.termbuilt"
TERM_TARGET="$(uname -m)-apple-macosx$MACOS_MIN"
TMP="${TMPDIR:-/tmp}"

shopt -s nullglob
SOURCES=("$ROOT"/$SWIFT_SOURCES_GLOB)
shopt -u nullglob
[ "${#SOURCES[@]}" -gt 0 ] || { echo "build-app: no Swift sources in $ROOT" >&2; exit 1; }

OBJ_DIR="$ROOT/.build/obj"
MODULE_NAME="KitchenSink"

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
    local f
    for f in "${SOURCES[@]}" "$ROOT/Info.plist" "$ROOT/install.conf"; do
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
    local res="$APP/Contents/Resources" d f src
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
    mkdir -p "$res/helpers-bin"
    for src in $HELPER_SOURCES; do
        swiftc -O -target "$(uname -m)-apple-macosx$MACOS_MIN" "$ROOT/$src" \
            -o "$res/helpers-bin/$(basename "$src" .swift)" >>"$LOG" 2>&1 || {
                cat "$LOG" >&2; echo "build-app: helper $src failed" >&2; return 1; }
    done
    find "$res" -name '__pycache__' -type d -prune -exec rm -rf {} +
    find "$res" -name '.DS_Store' -delete
}
compile_incremental() {
    local opt="$1" src base
    mkdir -p "$OBJ_DIR"
    : > "$LOG"

    local stamp="$OBJ_DIR/.stamp" want
    want="$(uname -m)-apple-macosx$MACOS_MIN $opt"
    if [ "$(cat "$stamp" 2>/dev/null)" != "$want" ]; then
        rm -f "$OBJ_DIR"/*.o "$OBJ_DIR"/*.swiftdeps "$OBJ_DIR"/master.* \
              "$OBJ_DIR/output-file-map.json"
        printf '%s\n' "$want" > "$stamp"
    fi

    local map="$OBJ_DIR/output-file-map.json"
    {
        printf '{\n'
        printf '  "": { "swift-dependencies": "%s/master.swiftdeps" },\n' "$OBJ_DIR"
        local i=0 n=${#SOURCES[@]}
        for src in "${SOURCES[@]}"; do
            base="$(basename "$src" .swift)"
            (( i++ )) || true
            printf '  "%s": { "object": "%s/%s.o", "swift-dependencies": "%s/%s.swiftdeps" }' \
                "$src" "$OBJ_DIR" "$base" "$OBJ_DIR" "$base"
            [ "$i" -lt "$n" ] && printf ','
            printf '\n'
        done
        printf '}\n'
    } > "$map"

    local stale=0
    for src in "${SOURCES[@]}"; do
        base="$(basename "$src" .swift)"
        if [ ! -f "$OBJ_DIR/$base.o" ] || [ "$src" -nt "$OBJ_DIR/$base.o" ]; then
            stale=1
            break
        fi
    done
    if [ "$stale" = 1 ]; then
        swiftc -c -incremental "$opt" -swift-version 5 -module-name "$MODULE_NAME" \
            -target "$(uname -m)-apple-macosx$MACOS_MIN" \
            -output-file-map "$map" \
            -I "$TERM_MOD_DIR" \
            "${SOURCES[@]}" >>"$LOG" 2>&1 || return 1
    fi

    local -a objs=()
    for src in "${SOURCES[@]}"; do
        objs+=("$OBJ_DIR/$(basename "$src" .swift).o")
    done
    swiftc "$opt" -swift-version 5 -module-name "$MODULE_NAME" \
        -target "$(uname -m)-apple-macosx$MACOS_MIN" \
        -Xlinker -sectcreate -Xlinker __TEXT -Xlinker __info_plist -Xlinker "$PLIST" \
        -I "$TERM_MOD_DIR" -Xlinker "$TERM_LIB" \
        "${objs[@]}" -o "$BUILD_TMP" >>"$LOG" 2>&1
}
if ! compile_incremental -O && ! compile_incremental -Onone; then
    grep -E 'error:' -A3 "$LOG" >&2 || cat "$LOG" >&2
    echo "build-app: compile failed (full log: $LOG)" >&2
    rm -f "$BUILD_TMP" "$PLIST"
    exit 1
fi

[ "$DIST" = 1 ] || pkill -f "$APP_NAME.app/Contents/MacOS" 2>/dev/null || true
mv "$BUILD_TMP" "$BIN"
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

SIGINFO="$(codesign -dvv "$APP" 2>&1)"
[ "$DIST" = 1 ] && exit 0

"$DIR/grant-permissions.sh" "$BUNDLE_ID" "$APP" >/dev/null 2>&1 || true
exit 0
