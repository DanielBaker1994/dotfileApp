#!/usr/bin/env bash
# make-dmg.sh — build the distributable disk image.
#
#   bin/make-dmg.sh            .build/dist/<DMG_NAME>-<APP_VERSION>.dmg
#   bin/make-dmg.sh --no-notarize   sign only (Developer ID set, skip the
#                                   notary round-trip — a quick local check)
#
# Signing follows install.conf:
#   DEVELOPER_ID + NOTARY_PROFILE set   hardened runtime, notarized, stapled:
#                                       opens with a double-click on any Mac
#   both empty                          the self-signed cert / ad-hoc: fine on
#                                       YOUR Macs; everyone else must allow it
#                                       in System Settings ▸ Privacy &
#                                       Security ▸ Open Anyway, and macOS asks
#                                       for the privacy permissions again
#                                       after every update
#
# The bundle is a separate build (bin/build-app.sh --dist, in .build/dist):
# the dev bundle, the running daemon and TCC are never touched.
set -uo pipefail
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
ROOT="$(cd "$DIR/.." && pwd -P)"
# shellcheck source=../install.conf
. "$ROOT/install.conf"

NOTARIZE=1
[ "${1:-}" = "--no-notarize" ] && NOTARIZE=0

OUT="$ROOT/.build/dist"
APP="$OUT/$APP_NAME.app"
DMG="$OUT/$DMG_NAME-$APP_VERSION.dmg"
STAGE="$OUT/dmg-stage"
say() { printf '\033[1;36m== %s\033[0m\n' "$*"; }
die() { printf '\033[31mmake-dmg: %s\033[0m\n' "$*" >&2; exit 1; }

if [ -n "$DEVELOPER_ID" ]; then
    security find-identity -v -p codesigning 2>/dev/null | grep -qF "$DEVELOPER_ID" \
        || die "DEVELOPER_ID '$DEVELOPER_ID' is not in the keychain (security find-identity -v -p codesigning)"
fi

say "building the app bundle (bin/build-app.sh --dist)"
"$DIR/build-app.sh" --dist || die "build failed"
codesign --verify --deep --strict "$APP" 2>&1 || die "the bundle's signature does not verify"

say "disk image"
rm -rf "$STAGE" "$DMG"
mkdir -p "$STAGE"
cp -Rp "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
hdiutil create -quiet -volname "$APP_NAME $APP_VERSION" -srcfolder "$STAGE" \
    -fs APFS -format UDZO -ov "$DMG" || die "hdiutil failed"
rm -rf "$STAGE"

if [ -n "$DEVELOPER_ID" ]; then
    codesign --force --timestamp --sign "$DEVELOPER_ID" "$DMG" || die "signing the disk image failed"
    if [ "$NOTARIZE" = 1 ]; then
        [ -n "$NOTARY_PROFILE" ] || die "NOTARY_PROFILE is empty (xcrun notarytool store-credentials NAME …, then set it in install.conf)"
        say "notarizing (Apple's notary service — a few minutes)"
        xcrun notarytool submit "$DMG" --keychain-profile "$NOTARY_PROFILE" --wait \
            || die "notarization failed (xcrun notarytool log <id> --keychain-profile $NOTARY_PROFILE)"
        xcrun stapler staple "$DMG" || die "stapling failed"
        spctl -a -t open --context context:primary-signature -v "$DMG" 2>&1 || die "Gatekeeper rejects the disk image"
    fi
    printf '\033[32m  ✔ %s (Developer ID%s)\033[0m\n' "$DMG" "$([ "$NOTARIZE" = 1 ] && echo ', notarized')"
else
    SIG="$(codesign -dvv "$APP" 2>&1)"
    case "$SIG" in
        *"Authority=$SIGN_ID"*) HOW="self-signed ($SIGN_ID)" ;;
        *) HOW="ad-hoc" ;;
    esac
    printf '\033[32m  ✔ %s\033[0m\n' "$DMG"
    printf '\033[33m  ! %s, not notarized: on another Mac the first open is blocked —\n' "$HOW"
    printf '    System Settings ▸ Privacy & Security ▸ Open Anyway. Set DEVELOPER_ID +\n'
    printf '    NOTARY_PROFILE in install.conf for a build that opens with a double-click.\033[0m\n'
fi
