#!/usr/bin/env python3
"""helper (pylib/helper) tests — the app's persistent python worker.

    python3 Tests/test_helper.py
"""
from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)
sys.path.insert(0, os.path.join(ROOT, "pylib"))

from helper import VERSION, dispatch, method

import helper.methods as _methods  # noqa: F401 — registers the production methods


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


class Methods(unittest.TestCase):
    def test_jira_paths_method(self):
        reply = dispatch({"id": 1, "method": "jira.paths"})
        self.assertTrue(reply["ok"], reply)
        res = reply["result"]
        for key in ("configJson", "teamJson", "legacyConfig", "cacheDir", "outDir"):
            self.assertIsInstance(res.get(key), str, key)
            self.assertTrue(res[key], key)
        self.assertIn("status", res["cache"])
        self.assertIn("issues", res["cache"])

    def test_jira_search_methods(self):
        reply = dispatch({"id": 1, "method": "jira.filter_kinds", "params": {"catalog": []}})
        self.assertTrue(reply["ok"], reply)
        self.assertIn("assignee", [k["key"] for k in reply["result"]["kinds"]])
        reply = dispatch({"id": 2, "method": "jira.criteria", "params": {"text": " x ", "rows": []}})
        self.assertTrue(reply["ok"], reply)
        self.assertEqual(reply["result"], {"text": "x"})

    def test_jira_directory_method(self):
        tmp = tempfile.mkdtemp(prefix="jira-dir-")
        self.addCleanup(shutil.rmtree, tmp, True)
        path = os.path.join(tmp, "directory.json")
        with open(path, "w") as fh:
            json.dump({"fetchedAt": "t", "projects": [{"key": "APP"}]}, fh)
        reply = dispatch({"id": 1, "method": "jira.directory", "params": {"path": path}})
        self.assertTrue(reply["ok"], reply)
        self.assertEqual(reply["result"]["fetchedAt"], "t")
        self.assertEqual(reply["result"]["projects"], [{"key": "APP", "name": ""}])

    def test_jira_options_method(self):
        tmp = tempfile.mkdtemp(prefix="jira-opts-")
        self.addCleanup(shutil.rmtree, tmp, True)
        path = os.path.join(tmp, "directory.json")
        with open(path, "w") as fh:
            json.dump({"users": [{"id": "u1", "name": "Ada"}]}, fh)
        reply = dispatch({"id": 1, "method": "jira.options",
                          "params": {"kind": "user", "path": path}})
        self.assertTrue(reply["ok"], reply)
        self.assertEqual([o["id"] for o in reply["result"]["options"]], ["currentUser()", "u1"])
        reply = dispatch({"id": 2, "method": "jira.options",
                          "params": {"kind": "value", "values": ["A"]}})
        self.assertEqual(reply["result"]["options"][0]["title"], "A")
        reply = dispatch({"id": 3, "method": "jira.options", "params": {"kind": "nope"}})
        self.assertFalse(reply["ok"])

    def test_jira_columns_methods(self):
        # Pinned to the CURRENT jira_fields contract: parse keeps width_px
        # (None = auto, fixed pixels when set — the round trip the Rust seam
        # used to drop), serialize emits the JSON columns form.
        reply = dispatch({"id": 1, "method": "jira.columns_parse",
                          "params": {"spec": "key:Key:80:left:filter+sort"}})
        self.assertTrue(reply["ok"], reply)
        self.assertEqual(reply["result"]["columns"][0],
                         {"field": "key", "title": "Key", "width": 80.0, "width_px": None,
                          "align": "left", "sortable": True, "filterable": True})
        reply = dispatch({"id": 2, "method": "jira.columns_serialize", "params": {
            "columns": [{"field": "a", "title": "T", "width": 120.0, "align": "left",
                         "sortable": True, "filterable": True}], "titles": False}})
        self.assertTrue(reply["ok"], reply)
        self.assertEqual(reply["result"]["spec"], '[{"field":"a","width":120,"filter":true,"sort":true}]')

    def test_jira_project_keys_method(self):
        reply = dispatch({"id": 1, "method": "jira.project_keys", "params": {"raw": "sam1, 1x"}})
        self.assertTrue(reply["ok"], reply)
        self.assertEqual(reply["result"], {"keys": ["SAM1"], "bad": ["1X"]})

    def test_confluence_preview_method(self):
        reply = dispatch({"id": 1, "method": "confluence.preview_html", "params": {
            "page": {"site": "https://x/wiki", "html": "<p>hi</p>", "title": "t"},
            "terms": [], "colors": {}}})
        self.assertTrue(reply["ok"], reply)
        html = reply["result"]["html"]
        self.assertIn("<p>hi</p>", html)
        self.assertIn("window.wsNext", html)

    def test_config_methods(self):
        reply = dispatch({"id": 1, "method": "config.tri", "params": {"text": "On"}})
        self.assertEqual(reply["result"]["value"], True)
        reply = dispatch({"id": 2, "method": "config.tri", "params": {"text": "maybe"}})
        self.assertIsNone(reply["result"]["value"])
        reply = dispatch({"id": 3, "method": "config.resolve_binary", "params": {"name": "/bin/sh"}})
        self.assertEqual(reply["result"]["path"], "/bin/sh")


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
    def serve(self, lines, replies, timeout=20):
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

    def by_id(self, replies):
        return {r["id"]: r for r in replies}

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
        got = self.by_id(replies)
        self.assertEqual(sorted(got), [1, 2, 3, 4])
        self.assertTrue(got[1]["ok"])
        self.assertFalse(got[2]["ok"])
        self.assertTrue(got[3]["ok"])
        self.assertTrue(got[4]["ok"])

    def test_a_bad_line_does_not_kill_the_server(self):
        code, replies, _ = self.serve(
            ["{nope\n", line({"id": 1, "method": "ping"})],
            replies=2,
        )
        self.assertEqual(code, 0)
        bad = [r for r in replies if r["id"] is None][0]
        self.assertIn("bad json", bad["error"]["message"])
        self.assertTrue([r for r in replies if r["id"] == 1][0]["ok"])

    def test_unknown_requests_never_print_to_stderr(self):
        code, _, err = self.serve([line({"id": 1, "method": "nope"})], replies=1)
        self.assertEqual(code, 0)
        self.assertEqual(err.strip(), "")

class JsonLaziness(unittest.TestCase):
    """REVIEW #6a/#8: a lazy module's bad JSON fails the CALL, not the
    worker, and the next call retries - the failure is never cached."""

    def test_bad_json_is_retried_in_the_same_worker(self):
        root = tempfile.mkdtemp(prefix="helper-lazy-")
        self.addCleanup(shutil.rmtree, root, True)
        env = helper_env()
        env["WS_JSON_ROOT"] = root
        p = subprocess.Popen(
            [sys.executable, "-B", "-m", "helper"],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            text=True, cwd=ROOT, env=env,
        )
        try:
            def call(rid, method, params=None):
                req = {"id": rid, "method": method}
                if params is not None:
                    req["params"] = params
                p.stdin.write(line(req))
                p.stdin.flush()
                return json.loads(p.stdout.readline())

            first = call(1, "shelf.bump", {"items": [], "path": "/x"})
            self.assertFalse(first["ok"], first)
            self.assertIn("json data", first["error"]["message"])
            self.assertIn("missing", first["error"]["message"])
            self.assertTrue(call(2, "ping")["ok"], "the worker stays alive")
            os.makedirs(os.path.join(root, "pylib"), exist_ok=True)
            with open(os.path.join(root, "pylib", "shelf.json"), "w") as fh:
                json.dump({"max_limit": 9, "whys": ["a"], "activity_whys": ["b"]}, fh)
            third = call(3, "shelf.bump", {"items": [], "path": "/x"})
            self.assertTrue(third["ok"], third)
            self.assertEqual(third["result"]["items"][0]["path"], "/x")
            self.assertTrue(call(4, "shutdown")["ok"])
        finally:
            if p.poll() is None:
                p.stdin.close()
                try:
                    p.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    p.kill()


if __name__ == "__main__":
    unittest.main(verbosity=2)
