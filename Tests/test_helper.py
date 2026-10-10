#!/usr/bin/env python3
"""helper (pylib/helper) tests — the app's persistent python worker.

    python3 Tests/test_helper.py
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)
sys.path.insert(0, os.path.join(ROOT, "pylib"))

from helper import VERSION, dispatch, method


def helper_env():
    env = dict(os.environ)
    env["PYTHONPATH"] = os.path.join(ROOT, "pylib")
    env["PYTHONDONTWRITEBYTECODE"] = "1"
    return env


def run(*args, stdin=""):
    return subprocess.run(
        [sys.executable, "-B", "-m", "helper"] + list(args),
        input=stdin, capture_output=True, text=True, cwd=ROOT, env=helper_env(),
    )


def line(obj):
    return json.dumps(obj, separators=(",", ":")) + "\n"


class Dispatch(unittest.TestCase):
    def test_ping(self):
        reply = dispatch({"id": 7, "method": "ping"})
        self.assertEqual(reply, {"id": 7, "ok": True, "result": {"version": VERSION}})

    def test_id_is_echoed_as_sent(self):
        self.assertEqual(dispatch({"id": "abc", "method": "ping"})["id"], "abc")
        self.assertIsNone(dispatch({"method": "ping"})["id"])

    def test_unknown_method(self):
        reply = dispatch({"id": 1, "method": "nope"})
        self.assertFalse(reply["ok"])
        self.assertIn("unknown method: nope", reply["error"]["message"])

    def test_request_must_be_an_object(self):
        for bad in ([1, 2], "ping", 3, None):
            reply = dispatch(bad)
            self.assertFalse(reply["ok"], bad)
            self.assertIn("JSON object", reply["error"]["message"])

    def test_params_must_be_an_object(self):
        reply = dispatch({"id": 1, "method": "ping", "params": [1]})
        self.assertFalse(reply["ok"])
        self.assertIn("params must be a JSON object", reply["error"]["message"])

    def test_handler_failure_is_data_not_a_crash(self):
        @method("test.boom")
        def _boom(params):
            raise ValueError("kaboom")

        reply = dispatch({"id": 2, "method": "test.boom"})
        self.assertFalse(reply["ok"])
        self.assertIn("ValueError: kaboom", reply["error"]["message"])
        self.assertIn("kaboom", reply["error"]["traceback"])


class OneShot(unittest.TestCase):
    def test_call_ping(self):
        r = run("--call", "ping")
        self.assertEqual(r.returncode, 0, r.stderr)
        reply = json.loads(r.stdout)
        self.assertTrue(reply["ok"])
        self.assertEqual(reply["result"]["version"], VERSION)

    def test_call_unknown_exits_nonzero(self):
        r = run("--call", "nope")
        self.assertEqual(r.returncode, 1)
        self.assertFalse(json.loads(r.stdout)["ok"])

    def test_call_with_params(self):
        r = run("--call", "ping", "--params", '{"a": 1}')
        self.assertEqual(r.returncode, 0)
        self.assertTrue(json.loads(r.stdout)["ok"])

    def test_call_bad_params_json(self):
        r = run("--call", "ping", "--params", "{nope")
        self.assertEqual(r.returncode, 1)
        self.assertIn("bad --params json", json.loads(r.stdout)["error"]["message"])

    def test_version(self):
        r = run("--version")
        self.assertEqual(r.returncode, 0)
        self.assertEqual(r.stdout.strip(), str(VERSION))

    def test_once(self):
        r = run("--once", stdin=line({"id": 5, "method": "ping"}))
        self.assertEqual(r.returncode, 0)
        self.assertEqual(json.loads(r.stdout), {"id": 5, "ok": True, "result": {"version": VERSION}})

    def test_once_bad_json_exits_nonzero(self):
        r = run("--once", stdin="{nope\n")
        self.assertEqual(r.returncode, 1)
        self.assertIn("bad json", json.loads(r.stdout)["error"]["message"])

    def test_once_empty_stdin(self):
        r = run("--once", stdin="")
        self.assertEqual(r.returncode, 1)
        self.assertIn("empty request", json.loads(r.stdout)["error"]["message"])


class Server(unittest.TestCase):
    def serve(self, lines, replies, timeout=10):
        p = subprocess.Popen(
            [sys.executable, "-B", "-m", "helper"],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            text=True, cwd=ROOT, env=helper_env(),
        )
        out, err = p.communicate("".join(lines), timeout=timeout)
        self.assertEqual(len(out.splitlines()), replies, "one reply line per request: %r" % out)
        for raw in out.splitlines():
            json.loads(raw)
        return p.returncode, [json.loads(l) for l in out.splitlines()], err

    def test_loop_with_shutdown(self):
        code, replies, _ = self.serve(
            ["\n",
             line({"id": 1, "method": "ping"}),
             line({"id": 2, "method": "nope"}),
             line({"id": 3, "method": "ping"}),
             line({"id": 4, "method": "shutdown"})],
            replies=4,
        )
        self.assertEqual(code, 0)
        self.assertEqual([r["id"] for r in replies], [1, 2, 3, 4])
        self.assertTrue(replies[0]["ok"])
        self.assertFalse(replies[1]["ok"])
        self.assertTrue(replies[3]["ok"])

    def test_a_bad_line_does_not_kill_the_server(self):
        code, replies, _ = self.serve(
            ["{nope\n", line({"id": 1, "method": "ping"})],
            replies=2,
        )
        self.assertEqual(code, 0)
        self.assertIn("bad json", replies[0]["error"]["message"])
        self.assertTrue(replies[1]["ok"])

    def test_unknown_requests_never_print_to_stderr(self):
        code, _, err = self.serve([line({"id": 1, "method": "nope"})], replies=1)
        self.assertEqual(code, 0)
        self.assertEqual(err.strip(), "")


if __name__ == "__main__":
    unittest.main(verbosity=2)
