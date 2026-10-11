#!/usr/bin/env python3
"""rs-parity — compare the Swift and Rust daemons' socket `state` JSON.

The Rust port must keep the socket contract byte-compatible (PLAN-rust-port.md).
This drives a scripted sequence of `do:ACTION` verbs against BOTH daemons and
reports (a) `state` keys the Rust side is missing and (b) common scalar keys
whose values disagree. It is a report tool, not a hard gate, unless --strict.

Usage:
  bin/rs-parity.py [--swift-sock PATH] [--rust-sock PATH]
                   [--actions A,B,C] [--strict] [--json]

Sockets default to $TMPDIR/ws-notes.sock for Swift and require --rust-sock
(or $WS_RUST_SOCK) for the Rust daemon.
"""
import argparse
import json
import os
import socket
import sys
import time

# Keys whose values must match when both sides are up. UI-only or
# implementation-specific keys are excluded (the Rust port is partial).
SCALAR_KEYS = [
    "view",
    "visible",
    "hideOnFocusLoss",
    "headerStyle",
    "escHides",
    "paletteCommands",
]

# Dotted state paths compared once both daemons have them. These are the
# contract keys the UI suites assert. `views.notes.frame` / `wid` are excluded
# (placement differs), and so is `views.notes.browser` (the Swift daemon has
# no `do:toggle-browser` action; the Rust port implements it).
NESTED_KEYS = [
    "palette",
    "viewSwitcher.shown",
    "terminalPanel.shown",
    "paths.shown",
    "views.notes.terminal",
    "views.notes.shown",
    "views.files.shown",
    "views.jira.shown",
]


def get_path(doc, path):
    cur = doc
    for part in path.split("."):
        if not isinstance(cur, dict) or part not in cur:
            return None
        cur = cur[part]
    return cur


def query(sock_path, msg, timeout=3.0):
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout(timeout)
    try:
        s.connect(sock_path)
        s.sendall((msg + "\n").encode())
        buf = b""
        while not buf.endswith(b"\n"):
            chunk = s.recv(65536)
            if not chunk:
                break
            buf += chunk
        return json.loads(buf.decode() or "null")
    except FileNotFoundError:
        return None
    except (ConnectionRefusedError, socket.timeout, OSError):
        return None
    finally:
        s.close()


def compare(swift, rust):
    report = {"missing_keys": [], "extra_keys": [], "scalar_mismatch": [],
              "nested_mismatch": []}
    if not isinstance(swift, dict) or not isinstance(rust, dict):
        return report
    for k in sorted(swift):
        if k not in rust:
            report["missing_keys"].append(k)
    for k in sorted(rust):
        if k not in swift:
            report["extra_keys"].append(k)
    for k in SCALAR_KEYS:
        if k in swift and k in rust and swift[k] != rust[k]:
            report["scalar_mismatch"].append({"key": k, "swift": swift[k], "rust": rust[k]})
    for k in NESTED_KEYS:
        sv, rv = get_path(swift, k), get_path(rust, k)
        if sv is not None and rv is not None and sv != rv:
            report["nested_mismatch"].append({"key": k, "swift": sv, "rust": rv})
    return report


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--swift-sock", default=os.path.join(os.environ.get("TMPDIR", "/tmp"), "ws-notes.sock"))
    ap.add_argument("--rust-sock", default=os.environ.get("WS_RUST_SOCK"))
    ap.add_argument("--actions", default="state,do:open:notes,do:open:files,do:hide,do:cycle")
    ap.add_argument("--settle", type=float, default=0.0,
                    help="seconds to sleep after each action before reading state "
                         "(the Swift shared window presents asynchronously)")
    ap.add_argument("--strict", action="store_true")
    ap.add_argument("--json", action="store_true")
    args = ap.parse_args()

    if not args.rust_sock:
        print("rs-parity: --rust-sock (or WS_RUST_SOCK) is required", file=sys.stderr)
        return 2

    actions = [a.strip() for a in args.actions.split(",") if a.strip()]
    results = []
    swift_up = query(args.swift_sock, "ping") is not None or query(args.swift_sock, "state") is not None
    rust_up = query(args.rust_sock, "state") is not None
    if not rust_up:
        print(f"rs-parity: Rust daemon not answering on {args.rust_sock}", file=sys.stderr)
        return 2
    if not swift_up:
        print(f"rs-parity: WARNING Swift daemon not answering on {args.swift_sock}; "
              "reporting Rust state shape only", file=sys.stderr)

    total_missing = 0
    total_nested = 0
    for action in actions:
        swift_state = query(args.swift_sock, action) if swift_up else {}
        rust_state = query(args.rust_sock, action)
        if args.settle and action != "state":
            time.sleep(args.settle)
            swift_state = query(args.swift_sock, "state") if swift_up else {}
            rust_state = query(args.rust_sock, "state")
        rep = compare(swift_state, rust_state)
        rep["action"] = action
        total_missing += len(rep["missing_keys"])
        total_nested += len(rep.get("nested_mismatch", []))
        results.append(rep)
        if not args.json:
            miss = ", ".join(rep["missing_keys"]) or "-"
            mism = "; ".join(f"{m['key']}: swift={m['swift']!r} rust={m['rust']!r}"
                             for m in rep["scalar_mismatch"]) or "-"
            print(f"[{action}] missing_keys: {miss}")
            print(f"           scalar_mismatch: {mism}")
            for m in rep.get("nested_mismatch", []):
                print(f"           nested_mismatch: {m['key']}: swift={m['swift']!r} rust={m['rust']!r}")

    if args.json:
        print(json.dumps({"swift_up": swift_up, "rust_up": rust_up, "results": results}, indent=2))
    else:
        print(f"\nrs-parity: {len(actions)} actions, {total_missing} missing keys / "
              f"{total_nested} nested mismatches on the Rust side (Swift up={swift_up})")
    if args.strict and (total_missing or total_nested):
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
