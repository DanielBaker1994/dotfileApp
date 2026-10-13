#!/usr/bin/env python3
"""ks-mcp tests (bin/ks-mcp.py) against a FAKE kitchen-sink daemon.

    python3 -B Tests/test_ks_mcp.py
    ./ws test mcp

bin/ks-mcp.py is the stdio MCP (legacy JSON-RPC 2.0) bridge to the daemon
socket. These cases drive it exactly like an MCP client, but every socket is a
throwaway unix-socket daemon in a short /tmp path that reads one line per
connection and replies with canned JSON (one request per connection, like the
real server). No real app, no real socket, no GUI is ever touched.

The cases lock the protocol fixes from MCP-TEST-FINDINGS.md §4:
invalid UTF-8 → -32700 and the loop survives; non-object params → -32602;
unknown `do:` verbs surface as `isError`; the daemon-down path is a tool error
rather than a crash; read-only mode hides/rejects mutating tools.
"""
from __future__ import annotations

import json
import os
import queue
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
KS_MCP = os.path.join(ROOT, "bin", "ks-mcp.py")

LATEST_PROTOCOL = "2025-11-25"
RAW_VERBS = ["screenshot", "screenshot-permission", "compare", "pane-shot"]

# Canned daemon state: enough shape to prove the JSON survived the round trip.
STATE = {
    "view": "notes",
    "visible": True,
    "views": {
        "notes": {"shown": True, "key": True, "terminal": False, "frame": [10, 20, 800, 600]}
    },
}

_UNSET = object()


class FakeDaemon:
    """A one-request-per-connection unix-socket daemon with canned replies."""

    def __init__(self):
        # A short path: sun_path is capped (104 bytes on macOS), so no long
        # tempfile prefixes — /tmp itself is short and always writable.
        self.dir = tempfile.mkdtemp(prefix="ksmcp-", dir="/tmp")
        self.path = os.path.join(self.dir, "daemon.sock")
        self._stop = threading.Event()
        self.srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.srv.bind(self.path)
        self.srv.listen(8)
        self.srv.settimeout(0.2)  # let the accept loop notice stop()
        self.thread = threading.Thread(target=self._serve, daemon=True)
        self.thread.start()

    def reply_for(self, line: str):
        if line == "state":
            return json.dumps(STATE, separators=(",", ":"))
        if line.startswith("do:"):
            action = line[3:]
            if action == "bogus-xyz":
                return json.dumps({"error": f"unknown action {action}"}, separators=(",", ":"))
            # any known-looking verb: the daemon answers with fresh state
            return json.dumps(STATE, separators=(",", ":"))
        if line.split("\t", 1)[0] == "screenshot-permission":
            return "granted"
        if line.split("\t", 1)[0] == "screenshot":
            return "error: screenshot unavailable in headless mode"
        return json.dumps(STATE, separators=(",", ":"))

    def _serve(self):
        while not self._stop.is_set():
            try:
                conn, _ = self.srv.accept()
            except socket.timeout:
                continue
            except OSError:
                break
            try:
                with conn:
                    data = b""
                    while b"\n" not in data:
                        chunk = conn.recv(65536)
                        if not chunk:
                            break
                        data += chunk
                    line = data.decode("utf-8", "replace").strip()
                    if not line:
                        continue
                    conn.sendall(self.reply_for(line).encode("utf-8") + b"\n")
            except OSError:
                continue

    def stop(self):
        self._stop.set()
        try:
            self.srv.close()
        except OSError:
            pass
        self.thread.join(timeout=5)
        shutil.rmtree(self.dir, ignore_errors=True)


class McpClient:
    """Drives bin/ks-mcp.py over stdin/stdout, line-delimited."""

    def __init__(self, cmd, env):
        self.p = subprocess.Popen(
            cmd,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=env,
        )
        self.q: queue.Queue = queue.Queue()
        self.stderr = bytearray()
        threading.Thread(target=self._pump_stdout, daemon=True).start()
        threading.Thread(target=self._pump_stderr, daemon=True).start()

    def _pump_stdout(self):
        try:
            for raw in self.p.stdout:
                self.q.put(raw)
        except ValueError:  # pipe closed underneath us on teardown
            pass
        finally:
            self.q.put(None)

    def _pump_stderr(self):
        try:
            for raw in self.p.stderr:
                self.stderr += raw
        except ValueError:
            pass

    def send(self, obj):
        self.send_raw((json.dumps(obj) + "\n").encode("utf-8"))

    def send_raw(self, data: bytes):
        self.p.stdin.write(data)
        self.p.stdin.flush()

    def recv(self, timeout=10.0):
        try:
            item = self.q.get(timeout=timeout)
        except queue.Empty:
            raise AssertionError("timed out waiting for a reply line")
        if item is None:
            raise AssertionError("ks-mcp closed stdout unexpectedly")
        return json.loads(item.decode("utf-8", "replace"))

    def alive(self):
        return self.p.poll() is None

    def finish(self, timeout=10.0):
        """Close stdin (EOF) and return the exit code."""
        try:
            self.p.stdin.close()
        except OSError:
            pass
        code = self.p.wait(timeout=timeout)
        self._close_pipes()
        return code

    def _close_pipes(self):
        for stream in (self.p.stdout, self.p.stderr, self.p.stdin):
            try:
                stream.close()
            except (OSError, ValueError):
                pass

    def kill(self):
        if self.p.poll() is None:
            try:
                self.p.stdin.close()
            except OSError:
                pass
            try:
                self.p.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.p.kill()
                self.p.wait()
        self._close_pipes()


class McpTestCase(unittest.TestCase):
    def setUp(self):
        self.daemon = None
        self.client = None
        self.addCleanup(self._cleanup)

    def _cleanup(self):
        if self.client is not None:
            self.client.kill()
        if self.daemon is not None:
            self.daemon.stop()

    def start_daemon(self):
        self.daemon = FakeDaemon()
        return self.daemon.path

    def start_server(self, socket_path=None, env_extra=None, flag=False):
        if self.client is not None:  # only one live server per test
            self.client.kill()
            self.client = None
        cmd = [sys.executable, "-B", KS_MCP]
        if socket_path is not None:
            cmd += ["--socket", socket_path]
        if flag:
            cmd.append("--read-only")
        env = dict(os.environ)
        env.pop("KS_MCP_READ_ONLY", None)  # never inherit the caller's mode
        if env_extra:
            env.update(env_extra)
        self.client = McpClient(cmd, env)
        return self.client

    def rpc(self, rid, method, params=_UNSET):
        req = {"jsonrpc": "2.0", "id": rid, "method": method}
        if params is not _UNSET:
            req["params"] = params
        self.client.send(req)
        return self.client.recv()

    def notify(self, method, params=_UNSET):
        req = {"jsonrpc": "2.0", "method": method}
        if params is not _UNSET:
            req["params"] = params
        self.client.send(req)

    def call_tool(self, rid, name, arguments=_UNSET):
        params = {"name": name}
        if arguments is not _UNSET:
            params["arguments"] = arguments
        return self.rpc(rid, "tools/call", params)


class Handshake(McpTestCase):
    def test_initialize_and_tools_list(self):
        path = self.start_daemon()
        self.start_server(path)
        init = self.rpc(0, "initialize", {
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": {"name": "test", "version": "1"},
        })
        res = init["result"]
        self.assertEqual(res["protocolVersion"], "2025-11-25")
        self.assertEqual(res["serverInfo"], {"name": "kitchen-sink", "version": "1.0.0"})
        self.assertEqual(res["capabilities"], {"tools": {"listChanged": False}})
        self.assertIn(path, res["instructions"])  # names the socket it points at

        # an unknown/old protocol negotiates down to our latest
        init2 = self.rpc(1, "initialize", {"protocolVersion": "1999-01-01"})
        self.assertEqual(init2["result"]["protocolVersion"], LATEST_PROTOCOL)

        # notifications/initialized must produce NO reply: the next line is
        # the tools/list reply we actually asked for.
        self.notify("notifications/initialized")
        tools = self.rpc(2, "tools/list")
        self.assertEqual(tools["id"], 2, "notification must not emit a reply line")
        names = [t["name"] for t in tools["result"]["tools"]]
        self.assertEqual(names, ["ks_state", "ks_do", "ks_raw", "ks_help"])

    def test_tool_descriptions_guide_agents(self):
        self.start_daemon()
        self.start_server(self.daemon.path)
        tools = {t["name"]: t for t in self.rpc(1, "tools/list")["result"]["tools"]}

        # Clients that show only the first description line (opencode 2.0.26
        # truncates to 117 chars + "...") must still see the key fact.
        for name, tool in tools.items():
            first = tool["description"].split("\n", 1)[0]
            self.assertTrue(first, name)
            self.assertLessEqual(len(first), 117, f"{name}: {first!r}")
        self.assertIn("ks_help", tools["ks_do"]["description"].split("\n", 1)[0])
        self.assertIn("WARNING", tools["ks_raw"]["description"].split("\n", 1)[0])
        self.assertIn("ks_help", tools["ks_state"]["description"].split("\n", 1)[0])

        do_desc = tools["ks_do"]["description"]
        self.assertIn("Valid actions", do_desc)
        for action in ("open:<view>", "toggle-terminal", "cycle", "home",
                       "pane:h|j|k|l", "screenshot:<sub>"):
            self.assertIn(action, do_desc, action)
        self.assertIn("notes", do_desc)  # the open:<view> list names the views

        raw_desc = tools["ks_raw"]["description"]
        self.assertIn("WARNING", raw_desc)
        self.assertIn("pane-shot", raw_desc)
        self.assertIn("capture the real screen", raw_desc)

        self.assertEqual(
            tools["ks_raw"]["inputSchema"]["properties"]["verb"]["enum"], RAW_VERBS
        )
        self.assertTrue(tools["ks_state"]["annotations"]["readOnlyHint"])
        self.assertFalse(tools["ks_raw"]["annotations"]["readOnlyHint"])


class Tools(McpTestCase):
    def test_ks_state_returns_state_json(self):
        self.start_daemon()
        self.start_server(self.daemon.path)
        res = self.call_tool(1, "ks_state")["result"]
        self.assertNotIn("isError", res)
        self.assertEqual(len(res["content"]), 1)
        self.assertEqual(res["content"][0]["type"], "text")
        self.assertEqual(json.loads(res["content"][0]["text"]), STATE)

    def test_ks_do_success_returns_state(self):
        self.start_daemon()
        self.start_server(self.daemon.path)
        res = self.call_tool(1, "ks_do", {"action": "hide"})["result"]
        self.assertNotIn("isError", res)
        self.assertEqual(json.loads(res["content"][0]["text"]), STATE)

    def test_ks_do_daemon_error_is_iserror(self):
        self.start_daemon()
        self.start_server(self.daemon.path)
        res = self.call_tool(1, "ks_do", {"action": "bogus-xyz"})["result"]
        self.assertTrue(res["isError"])
        self.assertIn("unknown action bogus-xyz", res["content"][0]["text"])

    def test_ks_do_rejects_bad_actions_before_the_daemon(self):
        self.start_daemon()
        self.start_server(self.daemon.path)
        for i, args in enumerate(({}, {"action": ""}, {"action": 5}, {"action": "a\nb"})):
            r = self.call_tool(10 + i, "ks_do", args)
            self.assertNotIn("result", r, args)
            self.assertEqual(r["error"]["code"], -32602, args)

    def test_ks_raw_rejects_unknown_verb(self):
        self.start_daemon()
        self.start_server(self.daemon.path)
        r = self.call_tool(1, "ks_raw", {"verb": "rm-rf"})
        self.assertNotIn("result", r)
        self.assertEqual(r["error"]["code"], -32602)

    def test_ks_raw_screenshot_permission(self):
        self.start_daemon()
        self.start_server(self.daemon.path)
        res = self.call_tool(1, "ks_raw", {"verb": "screenshot-permission"})["result"]
        self.assertNotIn("isError", res)
        self.assertEqual(res["content"][0]["text"], "granted")

    def test_ks_raw_screenshot_daemon_error_is_an_error_result(self):
        self.start_daemon()
        self.start_server(self.daemon.path)
        for args in (["-g"], []):
            res = self.call_tool(1, "ks_raw", {"verb": "screenshot", "args": args})["result"]
            self.assertTrue(res["isError"], args)
            self.assertIn("headless", res["content"][0]["text"])

    def test_ks_help_returns_the_guide(self):
        self.start_daemon()
        self.start_server(self.daemon.path)
        res = self.call_tool(1, "ks_help")["result"]
        self.assertNotIn("isError", res)
        text = res["content"][0]["text"]
        for needle in ("show", "back", "paths:show", "WARNING", "state"):
            self.assertIn(needle, text, needle)

    def test_ks_help_topic_actions(self):
        self.start_daemon()
        self.start_server(self.daemon.path)
        res = self.call_tool(1, "ks_help", {"topic": "actions"})["result"]
        text = res["content"][0]["text"]
        self.assertIn("Valid actions", text)
        self.assertIn("open:<view>", text)
        self.assertNotIn("WARNING", text)  # the raw warnings are their own section

    def test_ks_help_unknown_topic_is_iserror(self):
        self.start_daemon()
        self.start_server(self.daemon.path)
        res = self.call_tool(1, "ks_help", {"topic": "nope"})["result"]
        self.assertTrue(res["isError"])
        self.assertIn("topic", res["content"][0]["text"])


class Protocol(McpTestCase):
    def test_invalid_utf8_is_parse_error_and_server_survives(self):
        self.start_daemon()
        c = self.start_server(self.daemon.path)
        c.send_raw(b"\xff\xfe\n")
        err = c.recv()
        self.assertEqual(err["error"]["code"], -32700)
        self.assertIsNone(err["id"])
        self.assertTrue(c.alive())
        # a following request is still served
        r = self.rpc(1, "tools/list")
        self.assertEqual(r["id"], 1)
        self.assertIn("result", r)
        # EOF is a clean exit
        self.assertEqual(c.finish(), 0)

    def test_deeply_nested_json_is_parse_error_and_server_survives(self):
        self.start_daemon()
        c = self.start_server(self.daemon.path)
        deep = b'{"jsonrpc":"2.0","id":1,"method":"ping","params":' + b"[" * 200000 + b"]" * 200000 + b"}\n"
        c.send_raw(deep)
        self.assertEqual(c.recv()["error"]["code"], -32700)
        self.assertTrue(c.alive())
        self.assertIn("result", self.rpc(2, "tools/list"))

    def test_non_finite_id_is_never_echoed_as_infinity(self):
        self.start_daemon()
        c = self.start_server(self.daemon.path)
        c.send_raw(b'{"jsonrpc":"2.0","id":1e400,"method":"ping"}\n')
        # json.loads accepts `Infinity`, so check the decoded id, not the text.
        self.assertIsNone(c.recv()["id"])

    def test_over_long_ks_do_action_reports_request_too_long(self):
        self.start_daemon()
        self.start_server(self.daemon.path)
        r = self.rpc(1, "tools/call", {"name": "ks_do", "arguments": {"action": "open:files" + "A" * 8000}})
        self.assertTrue(r["result"]["isError"])
        self.assertIn("request too long", r["result"]["content"][0]["text"])

    def test_params_wrong_type_is_invalid_params(self):
        self.start_daemon()
        self.start_server(self.daemon.path)
        r = self.rpc(1, "tools/call", [1])
        self.assertEqual(r["error"]["code"], -32602)
        r = self.rpc(2, "initialize", "x")
        self.assertEqual(r["error"]["code"], -32602)
        r = self.rpc(3, "tools/list", [1])
        self.assertEqual(r["error"]["code"], -32602)

    def test_tools_call_arguments_must_be_an_object(self):
        self.start_daemon()
        self.start_server(self.daemon.path)
        r = self.rpc(1, "tools/call", {"name": "ks_state", "arguments": [1]})
        self.assertEqual(r["error"]["code"], -32602)

    def test_empty_and_absent_params_still_work(self):
        self.start_daemon()
        self.start_server(self.daemon.path)
        # empty list keeps the lenient default
        self.assertIn("result", self.rpc(1, "initialize", []))
        # absent params
        self.assertIn("result", self.rpc(2, "tools/list"))
        # explicit null
        self.assertIn("result", self.rpc(3, "tools/list", None))
        # empty object
        self.assertIn("result", self.rpc(4, "tools/list", {}))
        # and a real call with no params
        self.assertIn("result", self.rpc(5, "tools/call", {"name": "ks_state"}))

    def test_unknown_method_is_method_not_found(self):
        self.start_daemon()
        self.start_server(self.daemon.path)
        r = self.rpc(1, "definitely/not/a/method")
        self.assertEqual(r["error"]["code"], -32601)
        self.assertIn("method not found", r["error"]["message"])

    def test_notifications_get_no_reply(self):
        self.start_daemon()
        self.start_server(self.daemon.path)
        self.notify("notifications/cancelled")
        # next reply belongs to this real request, proving the notify was silent
        self.assertEqual(self.rpc(7, "ping")["id"], 7)


class ReadOnly(McpTestCase):
    def _ro(self):
        self.start_daemon()
        self.start_server(self.daemon.path, env_extra={"KS_MCP_READ_ONLY": "yes"})

    def test_ks_do_is_hidden_and_rejected(self):
        self._ro()
        names = [t["name"] for t in self.rpc(1, "tools/list")["result"]["tools"]]
        self.assertEqual(names, ["ks_state", "ks_raw", "ks_help"])
        r = self.call_tool(2, "ks_do", {"action": "hide"})
        self.assertEqual(r["error"]["code"], -32602)
        self.assertIn("read-only mode", r["error"]["message"])

    def test_ks_help_is_listed_and_works_read_only(self):
        self._ro()
        names = [t["name"] for t in self.rpc(1, "tools/list")["result"]["tools"]]
        self.assertIn("ks_help", names)
        res = self.call_tool(2, "ks_help", {"topic": "actions"})["result"]
        self.assertNotIn("isError", res)
        self.assertIn("Valid actions", res["content"][0]["text"])

    def test_mutating_raw_verbs_are_blocked(self):
        self._ro()
        for i, verb in enumerate(("screenshot", "pane-shot", "compare")):
            r = self.call_tool(10 + i, "ks_raw", {"verb": verb})
            self.assertEqual(r["error"]["code"], -32602, verb)
            self.assertIn("disabled", r["error"]["message"], verb)

    def test_allowed_raw_verb_and_state_still_work(self):
        self._ro()
        res = self.call_tool(1, "ks_raw", {"verb": "screenshot-permission"})["result"]
        self.assertEqual(res["content"][0]["text"], "granted")
        res = self.call_tool(2, "ks_state")["result"]
        self.assertEqual(json.loads(res["content"][0]["text"]), STATE)

    def test_read_only_flag_matches_env(self):
        self.start_daemon()
        self.start_server(self.daemon.path, flag=True)
        names = [t["name"] for t in self.rpc(1, "tools/list")["result"]["tools"]]
        self.assertEqual(names, ["ks_state", "ks_raw", "ks_help"])


class SocketResolution(McpTestCase):
    def test_ks_socket_env_is_used(self):
        path = self.start_daemon()
        self.start_server(None, env_extra={"KS_SOCKET": path})
        res = self.call_tool(1, "ks_state")["result"]
        self.assertEqual(json.loads(res["content"][0]["text"]), STATE)

    def test_socket_flag_wins_over_env(self):
        path = self.start_daemon()
        missing = os.path.join(self.daemon.dir, "missing.sock")
        self.start_server(missing, env_extra={"KS_SOCKET": path})
        res = self.call_tool(1, "ks_state")["result"]
        self.assertTrue(res["isError"])
        self.assertIn(missing, res["content"][0]["text"])


class DaemonDown(McpTestCase):
    def _missing_socket(self):
        tmp = tempfile.mkdtemp(prefix="ksmcp-none-", dir="/tmp")
        self.addCleanup(shutil.rmtree, tmp, True)
        return os.path.join(tmp, "missing.sock")

    def test_clear_tool_error_not_a_crash(self):
        missing = self._missing_socket()
        self.start_server(missing)
        res = self.call_tool(1, "ks_state")["result"]
        self.assertTrue(res["isError"])
        self.assertIn("not answering", res["content"][0]["text"])
        self.assertIn(missing, res["content"][0]["text"])
        self.assertTrue(self.client.alive(), "the bridge must survive a dead daemon")
        # ks_do takes the same path
        res = self.call_tool(2, "ks_do", {"action": "hide"})["result"]
        self.assertTrue(res["isError"])
        self.assertTrue(self.client.alive())

    def test_ks_help_works_with_the_daemon_down(self):
        missing = self._missing_socket()
        self.start_server(missing)
        res = self.call_tool(1, "ks_help")["result"]
        self.assertNotIn("isError", res, "ks_help must not need the daemon")
        text = res["content"][0]["text"]
        for needle in ("show", "back", "paths:show"):
            self.assertIn(needle, text, needle)


if __name__ == "__main__":
    unittest.main(verbosity=2)
