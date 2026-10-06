#!/usr/bin/env bash
# fix-permissions.sh — the one command when macOS permissions (Screen
# Recording, …) are missing or flaky. Run it in a normal Terminal (it may ask
# for your login password):
#   bin/fix-permissions.sh
# 1. makes sure a valid signing identity exists AND codesign can use its key
#    (a stable signature is what keeps grants valid across rebuilds),
# 2. clears the stale Screen Recording grant, 3. rebuilds + relaunches,
# 4. checks the app is really signed with it. Last step is yours: press Hyper+X
#    and click Allow ONCE (macOS can't pre-grant Screen Recording).
set -u
WS_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
. "$WS_ROOT/install.conf"   # BUNDLE_ID, APP_NAME, SIGN_ID
APP="$WS_ROOT/$APP_NAME.app"
KC="$HOME/Library/Keychains/login.keychain-db"
die() { echo "✘ $*" >&2; exit 1; }

valid() { security find-identity -v -p codesigning 2>/dev/null | grep -qF "\"$SIGN_ID\""; }
can_sign() {   # a real signing test: the only check that catches key-access problems
    local f rc; f="$(mktemp)"; cp /bin/echo "$f"
    perl -e 'alarm 20; exec @ARGV' codesign --force --sign "$SIGN_ID" "$f" >/dev/null 2>&1; rc=$?
    rm -f "$f"; return $rc
}

echo "▸ signing identity"
if ! valid; then
    T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
    printf '[req]\ndistinguished_name=dn\nx509_extensions=ext\nprompt=no\n[dn]\nCN=%s\n[ext]\nbasicConstraints=critical,CA:false\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=critical,codeSigning\n' "$SIGN_ID" > "$T/c.cnf"
    # /usr/bin/openssl (LibreSSL): its .p12 imports cleanly, brew's v3 needs -legacy
    /usr/bin/openssl req -x509 -newkey rsa:2048 -nodes -days 3650 -config "$T/c.cnf" -keyout "$T/k.pem" -out "$T/c.pem" 2>/dev/null \
      && /usr/bin/openssl pkcs12 -export -inkey "$T/k.pem" -in "$T/c.pem" -out "$T/c.p12" -passout pass:ws \
      && security import "$T/c.p12" -k "$KC" -P ws -T /usr/bin/codesign >/dev/null \
      && security add-trusted-cert -r trustRoot -p codeSign -k "$KC" "$T/c.pem" \
      || die "could not create/trust '$SIGN_ID'"
    valid || die "'$SIGN_ID' still not valid — Keychain Access ▸ it ▸ Trust ▸ Code Signing: Always Trust"
    echo "  created + trusted '$SIGN_ID'"
fi
if ! can_sign; then
    [ -t 0 ] || die "codesign can't use the key and needs your login password — run in a normal Terminal"
    read -rs -p "  login password (to authorize the key for codesign): " PW; echo
    { security unlock-keychain -p "$PW" "$KC" \
      && security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$PW" "$KC"; } >/dev/null 2>&1
    rc=$?; unset PW
    [ $rc = 0 ] && can_sign || die "codesign still can't use '$SIGN_ID' (wrong password?) — delete it in Keychain Access and re-run"
fi
echo "  ✔ '$SIGN_ID' usable"

echo "▸ clearing stale Screen Recording grant"
tccutil reset ScreenCapture "$BUNDLE_ID" >/dev/null 2>&1

echo "▸ rebuilding"
"$WS_ROOT/build.sh" --force --build-only || die "build failed"
codesign -dvv "$APP" 2>&1 | grep -qF "Authority=$SIGN_ID" && ! codesign -d -r- "$APP" 2>&1 | grep -q cdhash \
    || die "app is ad-hoc signed — grants would reset on every rebuild"
echo "  ✔ signed with '$SIGN_ID'"

pkill -f "$APP_NAME.app/Contents/MacOS" 2>/dev/null; sleep 1
"$WS_ROOT/bin/kitchen_sink.sh" window >/dev/null 2>&1 &
open "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
echo; echo "✔ done — press Hyper+X and click Allow once (quit + reopen the app if it still complains)."
