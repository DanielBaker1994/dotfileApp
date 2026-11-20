#!/bin/bash
# Wrapper: builds winwatch from winwatch.swift on first use (or when the source
# is newer), then runs it. Needs Xcode command line tools (swiftc).
d="$(cd "$(dirname "$0")" && pwd)"
if [ ! -x "$d/winwatch" ] || [ "$d/winwatch.swift" -nt "$d/winwatch" ]; then
  swiftc -O "$d/winwatch.swift" -o "$d/winwatch" 2>&1 | tail -3 >&2 || exit 1
fi
exec "$d/winwatch" "$@"
