#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" &>/dev/null && pwd)"
MAP="$SCRIPT_DIR/sketchybar-app-font/dist/icon_map.json"

usage() {
    cat <<'EOF'
usage: manage_icons.sh <cmd> [args]
  add <App-Name> <glyph-name>   map an app to an EXISTING font glyph
  lookup <App-Name>             show what an app currently maps to
  list                          show all mappings
  count                         number of mappings
EOF
    exit 1
}

[[ $# -ge 1 ]] || usage
cmd="$1"; shift

case "$cmd" in
    add)
        [[ $# -eq 2 ]] || usage
        app="$1"; glyph="$2"
        [[ "$glyph" == :* ]] || glyph=":$glyph:"
        python3 - "$MAP" "$app" "$glyph" <<'PY'
import json, sys
path, app, glyph = sys.argv[1], sys.argv[2], sys.argv[3]
with open(path) as f:
    data = json.load(f)
for entry in data:
    if glyph == entry["iconName"] and app in entry["appNames"]:
        print(f"already mapped: {app} -> {glyph}")
        sys.exit(0)
    if app in entry["appNames"]:
        entry["appNames"].remove(app)
entry = next((e for e in data if e["iconName"] == glyph), None)
if entry is None:
    print(f"error: glyph {glyph} not in font map (must rebuild font first)")
    sys.exit(1)
entry["appNames"].append(app)
with open(path, "w") as f:
    json.dump(data, f, indent=4)
print(f"mapped: {app} -> {glyph}")
PY
        brew services restart sketchybar >/dev/null 2>&1
        echo "sketchybar restarted"
        ;;
    lookup)
        [[ $# -eq 1 ]] || usage
        python3 - "$MAP" "$1" <<'PY'
import json, sys
path, app = sys.argv[1], sys.argv[2]
with open(path) as f:
    data = json.load(f)
hits = [e["iconName"] for e in data if app in e["appNames"]]
print(hits[0] if hits else "no mapping")
PY
        ;;
    list)
        python3 - "$MAP" <<'PY'
import json, sys
with open(sys.argv[1]) as f:
    data = json.load(f)
for e in data:
    for a in e["appNames"]:
        print(f"{a} -> {e['iconName']}")
PY
        ;;
    count)
        python3 - "$MAP" <<'PY'
import json, sys
with open(sys.argv[1]) as f:
    data = json.load(f)
print(len(data))
PY
        ;;
    *)
        usage
        ;;
esac