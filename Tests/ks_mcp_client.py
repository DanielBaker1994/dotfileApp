#!/usr/bin/env python3
"""Stdio MCP test client for bin/ks-mcp (hammer-test harness).

    C="python3 -I Tests/ks_mcp_client.py --socket SOCK"
    $C tools | call NAME [JSON] | raw FILE|-
    $C --read-only call ks_do '{"action":"hide"}'
    $C --env KS_MCP_READ_ONLY=yes tools
    $C --no-handshake call ks_state
    $C call ks_state | python3 -I Tests/ks_mcp_client.py --extract-state

Prints one JSON object per response line, then `# exit=<code> stderr=<300 chars>`.
`call` also prints `# state.view=… state.visible=…` when the result is state JSON.
`raw` sends the file's bytes verbatim (no handshake) and prints every reply line.
"""
from __future__ import annotations

import argparse
import json
import os
import queue
import subprocess
import sys
import threading
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SERVER = os.path.join(ROOT, "bin", "ks-mcp")


def extract_state() -> int:
    """Read client output on stdin; print the inner state JSON of the first tool result."""
    for line in sys.stdin:
        if line.startswith("#"):
            continue
        try:
            msg = json.loads(line)
            text = msg["result"]["content"][0]["text"]
            json.loads(text)
        except Exception:
            continue
        print(text)
        return 0
    print("no state found", file=sys.stderr)
    return 1


class Server:
    def __init__(self, args):
        cmd = [SERVER, "--socket", args.socket]
        if args.read_only:
            cmd.append("--read-only")
        env = dict(os.environ)
        for kv in args.env:
            k, _, v = kv.partition("=")
            env[k] = v
        self.p = subprocess.Popen(
            cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, env=env,
        )
        self.q: queue.Queue = queue.Queue()
        self.timeout = args.timeout
        threading.Thread(target=self._pump, daemon=True).start()
        self.err = b""
        threading.Thread(target=self._pump_err, daemon=True).start()

    def _pump(self):
        for raw in self.p.stdout:
            self.q.put(raw)
        self.q.put(None)

    def _pump_err(self):
        for raw in self.p.stderr:
            if len(self.err) < 4000:
                self.err += raw

    def send(self, obj_or_bytes):
        data = obj_or_bytes
        if not isinstance(data, bytes):
            data = (json.dumps(data) + "\n").encode()
        try:
            self.p.stdin.write(data)
            self.p.stdin.flush()
        except (BrokenPipeError, OSError):
            pass

    def recv(self, timeout=None):
        """Next stdout line (bytes), b'' on EOF, None on timeout."""
        try:
            item = self.q.get(timeout=timeout if timeout is not None else self.timeout)
        except queue.Empty:
            return None
        return b"" if item is None else item

    def handshake(self):
        self.send({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {
            "protocolVersion": "2025-11-25", "capabilities": {},
            "clientInfo": {"name": "ks-mcp-client", "version": "1"}}})
        r = self.recv()
        self.send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        return r

    def close(self):
        try:
            self.p.stdin.close()
        except OSError:
            pass
        try:
            code = self.p.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.p.kill()
            code = self.p.wait()
        time.sleep(0.05)
        err = self.err.decode("utf-8", "replace")[:300].replace("\n", " | ")
        print(f"# exit={code} stderr={err}")


def show(line: bytes):
    text = line.decode("utf-8", "replace").rstrip("\r\n")
    print(text)
    try:
        msg = json.loads(text)
        inner = json.loads(msg["result"]["content"][0]["text"])
        print(f"# state.view={inner.get('view')} state.visible={inner.get('visible')}")
    except Exception:
        pass


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--socket", default=os.environ.get("KS_SOCKET", ""))
    ap.add_argument("--read-only", action="store_true")
    ap.add_argument("--env", action="append", default=[])
    ap.add_argument("--no-handshake", action="store_true")
    ap.add_argument("--timeout", type=float, default=15.0)
    ap.add_argument("--extract-state", action="store_true")
    ap.add_argument("cmd", nargs="?", choices=["tools", "call", "raw"])
    ap.add_argument("rest", nargs="*")
    args = ap.parse_args()
    if args.extract_state:
        return extract_state()
    if not args.cmd:
        ap.error("need a subcommand")
    if not args.socket:
        ap.error("--socket required (or KS_SOCKET)")

    srv = Server(args)
    if args.cmd == "raw":
        data = sys.stdin.buffer.read() if args.rest[0] == "-" else open(args.rest[0], "rb").read()
        srv.send(data)
        # stream until idle for 1 s or the overall timeout passes
        deadline = time.time() + args.timeout
        while time.time() < deadline:
            line = srv.recv(timeout=min(1.0, max(0.05, deadline - time.time())))
            if line is None:
                break
            if line == b"":
                break
            show(line)
        srv.close()
        return 0

    if not args.no_handshake:
        if srv.handshake() is None:
            print("TIMEOUT waiting for initialize")
            srv.close()
            return 1
    if args.cmd == "tools":
        req = {"jsonrpc": "2.0", "id": 1, "method": "tools/list"}
    else:
        name = args.rest[0]
        arguments = json.loads(args.rest[1]) if len(args.rest) > 1 else {}
        req = {"jsonrpc": "2.0", "id": 1, "method": "tools/call",
               "params": {"name": name, "arguments": arguments}}
    srv.send(req)
    line = srv.recv()
    if line is None:
        print("TIMEOUT")
    elif line:
        show(line)
    srv.close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
