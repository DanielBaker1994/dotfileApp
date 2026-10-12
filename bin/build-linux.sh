#!/usr/bin/env bash
# bin/build-linux.sh — build a self-contained Linux dist of kitchen-sink.
#
#   bin/build-linux.sh            # build natively (run on a Linux host)
#   bin/build-linux.sh --docker   # build in the pinned image (rust/docker/linux.Dockerfile)
#
# Output: dist/kitchen-sink/{kitchen-sink, pylib/, vim/, …, run.sh} plus a
# generated commands.default.toml. run.sh sets WS_RS_ASSET_DIR to the dist dir
# and execs the binary, so `dist/kitchen-sink/run.sh headless` binds the socket
# with no checkout around it. Docs: AGENTS.md "Linux (experimental)".
set -euo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
ROOT="$(cd "$DIR/.." && pwd -P)"

IMAGE="kitchen-sink-linux-check"
DOCKERFILE="$ROOT/rust/docker/linux.Dockerfile"
# Native on Apple Silicon (fast); x86_64 hosts override via the env var.
PLATFORM="${LINUX_CHECK_PLATFORM:-linux/arm64}"
TARGET_DIR="$ROOT/.build/linux-target"
DIST="$ROOT/dist/kitchen-sink"

DOCKER=0
case "${1:-}" in
    --docker) DOCKER=1 ;;
    "") ;;
    *) echo "usage: bin/build-linux.sh [--docker]" >&2; exit 2 ;;
esac

# --- build ----------------------------------------------------------------
if [ "$DOCKER" = 1 ]; then
    command -v docker >/dev/null 2>&1 || { echo "build-linux: docker not found" >&2; exit 1; }
    docker info >/dev/null 2>&1 || { echo "build-linux: Docker is not running (open -a Docker)" >&2; exit 1; }
    echo "== docker build ($IMAGE) =="
    docker build --platform "$PLATFORM" -t "$IMAGE" -f "$DOCKERFILE" "$ROOT/rust/docker"
    echo "== cargo build --release -p ws-rs (container) =="
    docker run --rm --platform "$PLATFORM" \
        -v "$ROOT":/repo -w /repo/rust \
        -e CARGO_TARGET_DIR="/repo/.build/linux-target" \
        "$IMAGE" cargo build --release -p ws-rs
else
    case "$(uname -s)" in
        Linux) ;;
        *) echo "build-linux: not Linux; use --docker to build in the pinned image" >&2; exit 1 ;;
    esac
    command -v cargo >/dev/null 2>&1 || { echo "build-linux: cargo not found" >&2; exit 1; }
    echo "== cargo build --release -p ws-rs =="
    ( cd "$ROOT/rust" && CARGO_TARGET_DIR="$TARGET_DIR" cargo build --release -p ws-rs )
fi

BIN="$TARGET_DIR/release/kitchen-sink"
[ -x "$BIN" ] || { echo "build-linux: no binary at $BIN" >&2; exit 1; }

# --- lay out dist/kitchen-sink --------------------------------------------
echo "== dist -> $DIST =="
rm -rf "$DIST"
mkdir -p "$DIST"
cp -p "$BIN" "$DIST/kitchen-sink"

# Runtime assets the app resolves through Paths::asset_dir(): pylib/ is the
# Python helper, vim/ the notes pane, jira//confluence//notify//settings_hub/
# the Python CLIs, rules/ + config/ the seeded data.
for d in pylib vim jira confluence notify settings_hub rules config; do
    [ -d "$ROOT/$d" ] && cp -R "$ROOT/$d" "$DIST/$d"
done
for f in install.conf symlinks.sh UNINSTALL.sh app_icon.png jira_icon.png \
         notes_icon.png confluence_icon.png; do
    [ -f "$ROOT/$f" ] && cp -p "$ROOT/$f" "$DIST/$f"
done

# commands.default.toml: commands.toml with the owner-specific values reset,
# the same transform bin/build-app.sh applies to its --dist Resources payload.
awk '
    /^\[/ { sec = $0 }
    sec == "[notes]" && /^paths[ \t]*=/ {
        print "paths = \"~/notes, ~/.config/kitchen-sink/commands.toml\""; next }
    sec == "[notes]" && /^pdf-css[ \t]*=/ { print "pdf-css = \"\""; next }   # the dotfiles style
    /^config[ \t]*=.*fake\.json/ { next }   # bin/fake-confluence.sh start
    /^vim-bin[ \t]*=/ { print "vim-bin = \"nvim\""; next }
    sec == "[jira]" && /^enabled[ \t]*=/ { print "enabled = false"; next }
    sec == "[screenshot]" && /^contrast-opacity[ \t]*=/ { print "contrast-opacity = 190"; next }
    sec == "[screenshot]" && /^draw-color[ \t]*=/ { print "draw-color = \"#ff0000\""; next }
    sec == "[screenshot]" && /^save-path[ \t]*=/ { print "save-path = \"~/Desktop\""; next }
    sec == "[screenshot]" && /^start-mode[ \t]*=/ { print "start-mode = \"screenshot\""; next }
    { print }' "$ROOT/commands.toml" > "$DIST/commands.default.toml"

# A __pycache__ / .pyc inside the dist is junk (and would break a signed bundle).
find "$DIST" -name '__pycache__' -type d -prune -exec rm -rf {} + 2>/dev/null || true
find "$DIST" -name '*.pyc' -delete 2>/dev/null || true

# Wrapper: point the app at this dist dir and run it.
cat > "$DIST/run.sh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
export WS_RS_ASSET_DIR="$DIR"
exec "$DIR/kitchen-sink" "$@"
EOF
chmod 755 "$DIST/run.sh"

echo "build-linux: OK -> $DIST"
