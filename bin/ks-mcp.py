#!/usr/bin/env python3
"""ks-mcp — a minimal MCP server (legacy stdio JSON-RPC) over the kitchen-sink
daemon socket.

Thin v1 surface (deliberately small; the Python cold path stays out):

    ks_state   read the daemon's full state JSON
    ks_do      run a `do:<action>` verb (mutating; hidden in read-only mode)
    ks_raw     allow-listed raw verbs:
               screenshot | screenshot-permission | compare | pane-shot
    ks_help    offline guide (do: actions, raw verbs, state fields); no daemon

Point it at the live app (read-only recommended) or at an isolated headless
sandbox started with `bin/ks-mcp-sandbox.sh start` (then `--socket "$(bin/ks-mcp-sandbox.sh sock)"`
or `KS_SOCKET=$(bin/ks-mcp-sandbox.sh sock)`).

    bin/ks-mcp --list-tools
    bin/ks-mcp --socket "$TMPDIR/ws-notes.sock"

Transport: newline-delimited JSON-RPC 2.0 on stdio (legacy MCP handshake:
initialize → notifications/initialized → tools/list → tools/call). stdout is
protocol-only; diagnostics go to stderr. Single-threaded: one slow call
(`compare --wait`, an interactive screenshot) blocks the others.

Socket resolution: --socket > $KS_SOCKET > $TMPDIR/ws-notes.sock.
Read-only mode: --read-only or KS_MCP_READ_ONLY=1 hides `ks_do` and rejects it
with -32602; `ks_raw` then only allows `screenshot-permission`.
"""
from __future__ import annotations

import argparse
import base64
import json
import os
import socket
import sys

SERVER_NAME = "kitchen-sink"
SERVER_VERSION = "1.0.0"

# Legacy handshake revisions we accept; anything else gets our latest.
SUPPORTED_PROTOCOLS = ("2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05")
LATEST_PROTOCOL = "2025-11-25"

RAW_VERBS = ("screenshot", "screenshot-permission", "compare", "pane-shot")
READ_ONLY_RAW_VERBS = ("screenshot-permission",)

# The `do:` action grammar. It lives in one constant because clients that render
# only the first description line never see the list:
#   - the ks_do description carries it after the first newline, and
#   - the ks_help tool returns it in full.
DO_ACTIONS = (
    "Valid actions — pass one of these: `open:<view>` or `show[:<view>]` "
    "(`notes`, `files`, `jira`, `detail`, `releases`, `config`, `output`, "
    "`confluence`, `ai`, `compare`, `compareText`; bare `show` re-presents the "
    "last view, or `jira` if none), `rebuild-card:<view>`, `cycle`, "
    "`cycle-back`, `hide`, `back` (returns to the previous view), `home`, "
    "`toggle`, `toggle-terminal`, `toggle-browser`, `term`, `switcher`, "
    "`switcher-hide`, `notes-find`, `notes-grep`, `jira-jump`, "
    "`jira-poll-on`, `jira-poll-off`, `jira-setup`, `reset-size`, "
    "`pane:h|j|k|l`, `pane:focus:<id>`, `key:<spec>`, "
    "`paths:show|hide|return|select:<n>`, "
    "`header-style:<quiet|flat|edge|stripe|tinted|glow|aurora>`, "
    "`esc-hides:<view>:on|off`, `hide-on-focus-loss:on|off`, "
    "`tool:<paths|prettyprint|filefast|health-checks|health|text-table>`, "
    "`tool-close:<name>`, `compare:<sub>` (e.g. `next`, `prev`, `swap`, "
    "`reload`, `select:<n>`, `filter:<all|diffs|same|context>`, `folder-open`), "
    "`screenshot:<sub>`. `board:*` is not implemented in the Rust port yet. "
    "Window-only verbs (`paths:show`, `tool:<name>`, `switcher`, "
    "`toggle-terminal`, `toggle-browser`, `term`, `notes-find`, `notes-grep`, "
    "`jira-jump`, `reset-size`) are refused in headless mode. Pass one of "
    "these; a typo is not always distinguishable from success."
)

# The ks_help body for `topic=raw`: the raw verbs and why they are dangerous.
RAW_HELP = (
    "Raw socket verbs (`ks_raw` → `verb`, optional `args` list of strings, joined "
    "with tabs):\n"
    "- `screenshot` — capture the real screen; args `-g`/`--print-geometry` for "
    "geometry, `-r`/`--raw` for a raw PNG; interactive forms come up in the app.\n"
    "- `pane-shot` — capture a live window/pane to a PNG on disk (args per "
    "`kitchen-sink pane-shot --help`).\n"
    "- `compare` — the compare tool; args `--wait`, `--title1 T`, `--title2 T`, "
    "then 1-2 paths.\n"
    "- `screenshot-permission` — reports `granted`/`denied`.\n"
    "WARNING: `screenshot` and `pane-shot` capture the real screen. A headless "
    "sandbox refuses them (`error: <verb> unavailable in headless mode`) — never "
    "point them at the owner's machine. In read-only mode only "
    "`screenshot-permission` is allowed."
)

# The ks_help body for `topic=state`: the main `state` fields and the headless
# caveat (window-only verbs are refused; see `needs_window` in host.rs).
STATE_HELP = (
    "Main `state` fields (`ks_state` returns the whole JSON document):\n"
    "- `view` — the active SlotView name (\"\" when none is presented).\n"
    "- `visible` — true while the shared window is up.\n"
    "- `views.<name>.shown` — per-view visibility; also `key` (has key focus), "
    "`wid`, `frame [x,y,w,h]`; notes/files/jira add `terminal`, `drawerInset`, "
    "`query`, `rowCount`, `tabs`, `board`, `header`, ….\n"
    "- `windows` — the live window count.\n"
    "- `tools` — open tool panels by name (`paths`, `prettyprint`, `filefast`, …).\n"
    "- `paths` — the paths shelf (`shown`, `rows`).\n"
    "- `palette` — command palette visibility (`paletteCommands` lists its rows).\n"
    "Other keys: `active`, `keyWindow`, `escHides`, `hideOnFocusLoss`, `pane`, "
    "`screenshot`, `compare`, `paneShot`, `terminalPanel`, `viewSwitcher`, "
    "`activations`, `frontmostPid`, `pid`."
)

# The headless caveat, shared by ks_help's state section and its own note.
HEADLESS_HELP = (
    "Headless mode runs no AppKit: window-only verbs are refused (see "
    "`needs_window` in host.rs): `paths:show`, `switcher`, `toggle-terminal`, "
    "`toggle-browser`, `term`, `notes-find`, `notes-grep`, `jira-jump`, "
    "`reset-size`, `tool:<name>`. `screenshot`/`pane-shot` are refused too. "
    "`state` still reports `shown: true` for panels that cannot exist without a "
    "window."
)

SOCKET_TIMEOUT = 5.0
SCREENSHOT_RAW_TIMEOUT = 60.0
COMPARE_WAIT_TIMEOUT = 300.0
PANE_SHOT_TIMEOUT = 60.0

JSONRPC_ERRORS = {
    "parse": -32700,
    "invalid_request": -32600,
    "method_not_found": -32601,
    "invalid_params": -32602,
    "internal": -32603,
}


class DaemonDown(Exception):
    """The socket could not be reached (or died mid-call)."""


class RpcError(Exception):
    """A JSON-RPC level error (bad params, read-only refusal, …)."""

    def __init__(self, code: int, message: str):
        super().__init__(message)
        self.code = code
        self.message = message


class ToolError(Exception):
    """A tool failure: reported as a normal result with isError=true."""


def default_socket() -> str:
    tmp = os.environ.get("TMPDIR") or "/tmp"
    return os.path.join(tmp, "ws-notes.sock")


def read_only_env() -> bool:
    value = os.environ.get("KS_MCP_READ_ONLY", "")
    return value.strip().lower() in ("1", "true", "yes", "on")


# Mirrors READ_MAX in rust/ws-rs/src/app/socket.rs.
DAEMON_READ_MAX = 2048


def daemon_down_message(path: str) -> str:
    return (
        f"kitchen-sink daemon not answering at {path} "
        "(start the app or use bin/ks-mcp-sandbox.sh)"
    )


def send_and_read(path: str, payload: str, timeout: float, mode: str):
    """One request line, one connection. Returns (bytes, timed_out).

    mode: "line" stops at the first newline, "eof" reads to close, "none"
    writes and returns immediately (verbs that answer later or never).
    """
    body = payload.encode("utf-8") + b"\n"
    if len(body) > DAEMON_READ_MAX:  # the daemon refuses (and closes early) past this
        raise ToolError(f"request too long ({len(body)} bytes; the daemon accepts {DAEMON_READ_MAX})")
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.settimeout(timeout)
    try:
        try:
            sock.connect(path)
            sock.sendall(body)
        except OSError as exc:  # FileNotFoundError / ConnectionRefused / EPIPE
            raise DaemonDown(path) from exc
        if mode == "none":
            return b"", False
        out = bytearray()
        while True:
            try:
                chunk = sock.recv(65536)
            except socket.timeout:
                return bytes(out), True
            if not chunk:
                return bytes(out), False
            out += chunk
            if mode == "line" and b"\n" in chunk:
                return bytes(out), False
    finally:
        sock.close()


def json_reply(path: str, payload: str, timeout: float):
    """A one-line JSON reply (state / do:). Returns (parsed_or_None, text)."""
    data, timed_out = send_and_read(path, payload, timeout, "line")
    if not data:
        why = "no reply" + (" within %.0f s" % timeout if timed_out else " before close")
        raise ToolError(f"{payload.splitlines()[0]}: {why}")
    text = data.decode("utf-8", "replace").strip()
    try:
        return json.loads(text), text
    except ValueError:
        return None, text


def text_result(text: str):
    return {"content": [{"type": "text", "text": text}]}


def error_result(text: str):
    return {"content": [{"type": "text", "text": text}], "isError": True}


class Server:
    def __init__(self, sock_path: str, read_only: bool):
        self.sock = sock_path
        self.read_only = read_only

    # -- tools ---------------------------------------------------------------

    def tool_ks_state(self, _args):
        parsed, text = json_reply(self.sock, "state", SOCKET_TIMEOUT)
        if parsed is None:
            return text_result(text)
        return text_result(json.dumps(parsed, separators=(",", ":")))

    def tool_ks_help(self, args):
        """Offline guide: never touches the socket (works with the daemon down)."""
        topic = args.get("topic")
        if topic is None:
            topic = "all"
        if not isinstance(topic, str) or topic not in ("actions", "state", "raw", "all"):
            raise ToolError(
                "ks_help 'topic' must be one of: actions, state, raw (default: all)"
            )
        sections = []
        if topic in ("all", "actions"):
            sections.append(
                "do: actions (pass one as `ks_do`'s `action`, without the `do:` "
                "prefix):\n" + DO_ACTIONS
            )
        if topic in ("all", "raw"):
            sections.append(RAW_HELP)
        if topic in ("all", "state"):
            sections.append(STATE_HELP + "\n" + HEADLESS_HELP)
        return text_result("\n\n".join(sections))

    def tool_ks_do(self, args):
        action = args.get("action")
        if not isinstance(action, str) or not action:
            raise RpcError(JSONRPC_ERRORS["invalid_params"], "ks_do needs a non-empty 'action' string")
        if "\n" in action:
            raise RpcError(JSONRPC_ERRORS["invalid_params"], "ks_do 'action' must be one line")
        parsed, text = json_reply(self.sock, "do:" + action, 10.0)
        if isinstance(parsed, dict) and "error" in parsed:
            return error_result(text)
        return text_result(text)

    def tool_ks_raw(self, args):
        verb = args.get("verb")
        if verb not in RAW_VERBS:
            raise RpcError(
                JSONRPC_ERRORS["invalid_params"],
                "ks_raw 'verb' must be one of: " + ", ".join(RAW_VERBS),
            )
        raw_args = args.get("args", [])
        if isinstance(raw_args, str):
            raw_args = [raw_args]
        if not isinstance(raw_args, list) or not all(isinstance(a, str) for a in raw_args):
            raise RpcError(JSONRPC_ERRORS["invalid_params"], "ks_raw 'args' must be a list of strings")
        for a in raw_args:
            if "\t" in a or "\n" in a:
                raise RpcError(JSONRPC_ERRORS["invalid_params"], "ks_raw 'args' must not contain tabs or newlines")

        if self.read_only and verb not in READ_ONLY_RAW_VERBS:
            raise RpcError(
                JSONRPC_ERRORS["invalid_params"],
                f"read-only mode: ks_raw '{verb}' is disabled (set KS_MCP_READ_ONLY=0 to allow)",
            )

        payload = verb if not raw_args else verb + "\t" + "\t".join(raw_args)
        if verb == "screenshot":
            return self.raw_screenshot(raw_args, payload)
        if verb == "screenshot-permission":
            data, _ = send_and_read(self.sock, payload, SOCKET_TIMEOUT, "line")
            text = data.decode("utf-8", "replace").strip()
            if not text:
                return error_result("screenshot-permission: no answer")
            return text_result(text)
        if verb == "compare":
            wait = "--wait" in raw_args
            timeout = COMPARE_WAIT_TIMEOUT if wait else SOCKET_TIMEOUT
            data, timed_out = send_and_read(self.sock, payload, timeout, "line")
            text = data.decode("utf-8", "replace").strip()
            if wait and not text:
                return error_result(f"compare --wait: no reply within {timeout:.0f} s")
            return self.raw_text_result(text or "ok")
        # pane-shot
        data, timed_out = send_and_read(self.sock, payload, PANE_SHOT_TIMEOUT, "line")
        text = data.decode("utf-8", "replace").strip()
        if not text:
            return error_result("pane-shot: no answer")
        if text.startswith("error: "):
            return error_result(text[7:])
        return text_result(text)

    def raw_screenshot(self, raw_args: list, payload: str):
        if "-r" in raw_args or "--raw" in raw_args:
            data, timed_out = send_and_read(
                self.sock, payload, SCREENSHOT_RAW_TIMEOUT, "eof"
            )
            if not data:
                why = "timed out" if timed_out else "closed with no data"
                return error_result(
                    f"screenshot --raw: no image ({why}; interactive captures only "
                    "deliver once accepted — try the live app)"
                )
            if data.startswith(b"\x89PNG"):
                return {
                    "content": [
                        {
                            "type": "image",
                            "data": base64.b64encode(data).decode("ascii"),
                            "mimeType": "image/png",
                        }
                    ]
                }
            return self.raw_text_result(data.decode("utf-8", "replace").strip())
        if "-g" in raw_args or "--print-geometry" in raw_args:
            data, _ = send_and_read(self.sock, payload, SOCKET_TIMEOUT, "line")
            text = data.decode("utf-8", "replace").strip()
            if not text:
                return error_result("screenshot --print-geometry: no answer")
            return self.raw_text_result(text)
        # Interactive forms may defer the reply until the overlay closes; a
        # clean close / no reply means the overlay is up (or the verb is quiet).
        data, _ = send_and_read(self.sock, payload, SOCKET_TIMEOUT, "line")
        text = data.decode("utf-8", "replace").strip()
        return self.raw_text_result(text or "ok")

    @staticmethod
    def raw_text_result(text: str):
        """Daemon `error: …` replies are failures, not successes."""
        if text.startswith("error: "):
            return error_result(text[7:])
        return text_result(text)

    # -- MCP plumbing --------------------------------------------------------

    def tool_list(self):
        tools = []
        if not self.read_only:
            tools.append(
                {
                    "name": "ks_do",
                    "description": (
                        "Run a do:<action> verb; returns state JSON. Call ks_help for the valid actions.\n"
                        "Mutating (toggle views, terminal, panes, screenshot verbs, …). "
                        + DO_ACTIONS
                    ),
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "action": {
                                "type": "string",
                                "description": "Action without the `do:` prefix, e.g. `open:notes`, `toggle-terminal`, `cycle`.",
                            }
                        },
                        "required": ["action"],
                        "additionalProperties": False,
                    },
                    "annotations": {
                        "readOnlyHint": False,
                        "destructiveHint": True,
                        "idempotentHint": False,
                    },
                }
            )
        tools.insert(
            0,
            {
                "name": "ks_state",
                "description": (
                    "Read the kitchen-sink daemon's full state JSON. Call ks_help for the field guide.\n"
                    "(view, visible, views.<view>.shown/key/terminal, panes, palette, …)."
                ),
                "inputSchema": {"type": "object", "properties": {}, "additionalProperties": False},
                "annotations": {"readOnlyHint": True, "idempotentHint": True},
            },
        )
        raw_desc = (
            "Raw socket verb (screenshot, pane-shot, compare...). WARNING: captures the real screen.\n"
            "Send an allow-listed raw socket verb: `screenshot` (args like `-g` "
            "for geometry or `-r` for a raw PNG; interactive forms come up in the "
            "app), `screenshot-permission` (granted/denied), `compare` (args "
            "`--wait`, `--title1 T`, `--title2 T`, then 1-2 paths), `pane-shot` "
            "(args per `kitchen-sink pane-shot --help`). WARNING: `screenshot` "
            "and `pane-shot` capture the real screen; they are refused in "
            "headless sandbox mode — never point them at one."
        )
        if self.read_only:
            raw_desc += " Read-only mode: only `screenshot-permission` is allowed."
        tools.append(
            {
                "name": "ks_raw",
                "description": raw_desc,
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "verb": {"type": "string", "enum": list(RAW_VERBS)},
                        "args": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "Verb arguments, one per element (joined with tabs).",
                        },
                    },
                    "required": ["verb"],
                    "additionalProperties": False,
                },
                "annotations": {
                    "readOnlyHint": False,
                    "destructiveHint": True,
                    "idempotentHint": False,
                },
            }
        )
        tools.append(
            {
                "name": "ks_help",
                "description": (
                    "Offline guide to the ks_* tools (no daemon contact). Covers do: actions, raw verbs, state fields.\n"
                    "Optional `topic`: `actions` (the full `do:` action list and "
                    "argument grammar), `state` (the main `state` fields and the "
                    "headless window-only caveat), `raw` (the raw verbs and their "
                    "screen-capture warnings); default all. Works in read-only "
                    "mode and with the daemon down."
                ),
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "topic": {
                            "type": "string",
                            "enum": ["actions", "state", "raw"],
                            "description": "Which section to return; default all.",
                        }
                    },
                    "additionalProperties": False,
                },
                "annotations": {"readOnlyHint": True, "idempotentHint": True},
            }
        )
        return {"tools": tools}

    def call_tool(self, name: str, args):
        if name == "ks_state":
            return self.tool_ks_state(args)
        if name == "ks_help":
            return self.tool_ks_help(args)
        if name == "ks_do":
            if self.read_only:
                raise RpcError(
                    JSONRPC_ERRORS["invalid_params"],
                    "read-only mode: ks_do is disabled (set KS_MCP_READ_ONLY=0 to allow)",
                )
            return self.tool_ks_do(args)
        if name == "ks_raw":
            return self.tool_ks_raw(args)
        raise RpcError(JSONRPC_ERRORS["invalid_params"], f"unknown tool: {name}")

    def handle_request(self, req: dict):
        method = req.get("method")
        params = req.get("params")
        if not params:  # absent / null / {} / [] keep the lenient default
            params = {}
        elif not isinstance(params, dict):
            raise RpcError(JSONRPC_ERRORS["invalid_params"], "'params' must be an object")
        if method == "initialize":
            requested = params.get("protocolVersion")
            protocol = requested if requested in SUPPORTED_PROTOCOLS else LATEST_PROTOCOL
            mode = "read-only" if self.read_only else "read-write"
            return {
                "protocolVersion": protocol,
                "capabilities": {"tools": {"listChanged": False}},
                "serverInfo": {"name": SERVER_NAME, "version": SERVER_VERSION},
                "instructions": (
                    f"kitchen-sink socket bridge at {self.sock} ({mode}). "
                    "ks_state reads state; ks_do runs `do:` verbs; ks_raw sends "
                    "allow-listed raw verbs; ks_help is an offline guide (do: "
                    "actions, raw verbs, state fields) that needs no daemon. Point "
                    "the socket at bin/ks-mcp-sandbox.sh for a disposable headless "
                    "daemon."
                ),
            }
        if method == "ping":
            return {}
        if method == "tools/list":
            return self.tool_list()
        if method == "tools/call":
            name = params.get("name")
            args = params.get("arguments") or {}
            if not isinstance(name, str) or not name:
                raise RpcError(JSONRPC_ERRORS["invalid_params"], "tools/call needs a 'name'")
            if not isinstance(args, dict):
                raise RpcError(JSONRPC_ERRORS["invalid_params"], "'arguments' must be an object")
            try:
                return self.call_tool(name, args)
            except ToolError as exc:
                return error_result(str(exc))
            except DaemonDown:
                return error_result(daemon_down_message(self.sock))
        raise RpcError(JSONRPC_ERRORS["method_not_found"], f"method not found: {method}")


def write_message(message: dict):
    sys.stdout.write(json.dumps(message, separators=(",", ":")) + "\n")
    sys.stdout.flush()


def serve(server: Server):
    # Decode stdin as UTF-8 with errors="replace": invalid bytes on the wire
    # must not take the server down. They surface as a JSON-RPC parse error
    # (-32700) and the loop keeps serving.
    for raw in sys.stdin.buffer:
        line = raw.decode("utf-8", "replace").strip()
        if not line:
            continue
        try:
            req = json.loads(line)
        except (ValueError, RecursionError):  # deeply nested input raises RecursionError
            write_message(
                {
                    "jsonrpc": "2.0",
                    "id": None,
                    "error": {"code": JSONRPC_ERRORS["parse"], "message": "parse error"},
                }
            )
            continue
        if not isinstance(req, dict) or "method" not in req:
            rid = req.get("id") if isinstance(req, dict) else None
            write_message(
                {
                    "jsonrpc": "2.0",
                    "id": rid,
                    "error": {
                        "code": JSONRPC_ERRORS["invalid_request"],
                        "message": "invalid request",
                    },
                }
            )
            continue
        if "id" not in req or req.get("id") is None:
            continue  # notification (notifications/initialized, …): no reply
        rid = req["id"]
        if isinstance(rid, float) and (rid != rid or rid in (float("inf"), float("-inf"))):
            rid = None  # 1e400 parses to inf, which json.dumps would emit as invalid `Infinity`
        try:
            result = server.handle_request(req)
        except RpcError as exc:
            write_message(
                {
                    "jsonrpc": "2.0",
                    "id": rid,
                    "error": {"code": exc.code, "message": exc.message},
                }
            )
            continue
        except Exception as exc:  # never take the server down on a tool bug
            write_message(
                {
                    "jsonrpc": "2.0",
                    "id": rid,
                    "error": {"code": JSONRPC_ERRORS["internal"], "message": f"internal error: {exc}"},
                }
            )
            continue
        write_message({"jsonrpc": "2.0", "id": rid, "result": result})


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(
        prog="ks-mcp",
        description="Minimal MCP (legacy stdio JSON-RPC) bridge to the kitchen-sink daemon socket.",
    )
    parser.add_argument(
        "--socket",
        default=os.environ.get("KS_SOCKET") or default_socket(),
        help="daemon socket path (default: $KS_SOCKET, else $TMPDIR/ws-notes.sock)",
    )
    parser.add_argument(
        "--read-only",
        action="store_true",
        default=read_only_env(),
        help="hide/reject mutating tools (also: KS_MCP_READ_ONLY=1)",
    )
    parser.add_argument(
        "--list-tools",
        action="store_true",
        help="print the tool names for this mode and exit",
    )
    args = parser.parse_args(argv)

    server = Server(args.socket, args.read_only)
    if args.list_tools:
        for tool in server.tool_list()["tools"]:
            mode = "RO" if tool["annotations"].get("readOnlyHint") else "MUT"
            print(f"{tool['name']}\t{mode}")
        return 0
    try:
        serve(server)
    except (BrokenPipeError, KeyboardInterrupt):
        # The client went away (or was Ctrl-C'd); no traceback on a dead pipe.
        try:
            os.dup2(os.open(os.devnull, os.O_WRONLY), sys.stdout.fileno())
        except OSError:
            pass
    return 0


if __name__ == "__main__":
    sys.exit(main())
