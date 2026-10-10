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

FIXTURES = os.path.join(ROOT, "Tests", "helper_fixtures")

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


class ScriptBridge(unittest.TestCase):
    def run_script(self, **params):
        return dispatch({"id": 1, "method": "script.run", "params": params})

    def test_runs_a_script_in_its_folder(self):
        reply = self.run_script(folder=FIXTURES, script="echo_script.py",
                                args=["a", "b"], stdin="hello\n")
        self.assertTrue(reply["ok"], reply)
        res = reply["result"]
        self.assertEqual(res["code"], 0)
        self.assertEqual(res["stderr"], "")
        payload = json.loads(res["stdout"].strip().splitlines()[-1])
        self.assertEqual(payload["echo"], "hello\n")
        self.assertEqual(payload["argv"], ["a", "b"])

    def test_unicode_stdin_round_trips(self):
        reply = self.run_script(folder=FIXTURES, script="echo_script.py", stdin="héllo ✓\n")
        payload = json.loads(reply["result"]["stdout"].strip().splitlines()[-1])
        self.assertEqual(payload["echo"], "héllo ✓\n")

    def test_a_failing_script_is_data_not_an_error(self):
        reply = self.run_script(folder=FIXTURES, script="fail_script.py")
        self.assertTrue(reply["ok"], reply)
        self.assertEqual(reply["result"]["code"], 3)
        self.assertIn("boom: fixture failure", reply["result"]["stderr"])

    def test_missing_script_is_an_error(self):
        reply = self.run_script(folder=FIXTURES, script="no_such_script.py")
        self.assertFalse(reply["ok"])
        self.assertIn("no such script", reply["error"]["message"])

    def test_script_without_a_name_is_an_error(self):
        reply = self.run_script(folder=FIXTURES)
        self.assertFalse(reply["ok"])
        self.assertIn("script is required", reply["error"]["message"])

    def test_timeout_is_an_error(self):
        reply = self.run_script(folder=FIXTURES, script="sleepy_script.py",
                                args=["5"], timeout=0.1)
        self.assertFalse(reply["ok"])
        self.assertIn("timed out", reply["error"]["message"])

    def test_jira_paths_method(self):
        reply = dispatch({"id": 1, "method": "jira.paths"})
        self.assertTrue(reply["ok"], reply)
        res = reply["result"]
        for key in ("configJson", "teamJson", "legacyConfig", "cacheDir", "outDir"):
            self.assertIsInstance(res.get(key), str, key)
            self.assertTrue(res[key], key)
        self.assertIn("status", res["cache"])
        self.assertIn("issues", res["cache"])

    def test_jira_style_method(self):
        reply = dispatch({"id": 1, "method": "jira.style", "params": {
            "values": {"status-done-words": "shipped"}}})
        self.assertTrue(reply["ok"], reply)
        res = reply["result"]
        self.assertEqual(res["words"]["done"], ["shipped"])
        self.assertEqual(res["status"][1]["words"], ["shipped"])
        self.assertIn("key", res["dimFields"])

    def test_jira_comments_method(self):
        tmp = tempfile.mkdtemp(prefix="jira-comments-")
        self.addCleanup(shutil.rmtree, tmp, True)
        path = os.path.join(tmp, "issues.json")
        with open(path, "w") as fh:
            json.dump({"A-1": {"comments": [{"author": "a", "body": "b", "created": "c"}]}}, fh)
        reply = dispatch({"id": 1, "method": "jira.comments", "params": {"path": path, "key": "A-1"}})
        self.assertTrue(reply["ok"], reply)
        self.assertEqual(reply["result"]["comments"], [{"author": "a", "body": "b", "created": "c"}])

    def test_jira_page_methods(self):
        reply = dispatch({"id": 1, "method": "jira.ticket_html", "params": {
            "fields": {"key": "A-1", "status": "Done"},
            "categories": {"Done": "done"}, "words": {}, "colors": {}, "labels": {}}})
        self.assertTrue(reply["ok"], reply)
        self.assertIn("A-1", reply["result"]["html"])
        reply = dispatch({"id": 2, "method": "jira.comments_html", "params": {"comments": []}})
        self.assertTrue(reply["ok"], reply)
        self.assertIn("No comments.", reply["result"]["html"])

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

    def test_jira_draft_method(self):
        reply = dispatch({"id": 1, "method": "jira.draft", "params": {
            "kind": "live", "maxResults": "25", "columns": "key"}})
        self.assertTrue(reply["ok"], reply)
        self.assertEqual(reply["result"]["draft"]["maxResults"], 25)
        reply = dispatch({"id": 2, "method": "jira.draft", "params": {
            "kind": "job", "pageSize": "x"}})
        self.assertTrue(reply["ok"], reply)
        self.assertFalse(reply["result"]["ok"])
        self.assertIn("Page size", reply["result"]["message"])

    def test_jira_edit_methods(self):
        reply = dispatch({"id": 1, "method": "jira.field_label", "params": {
            "field": "title", "value": " Heading ", "default": "Title", "current": {}}})
        self.assertTrue(reply["ok"], reply)
        self.assertEqual(reply["result"]["value"], {"title": "Heading"})
        reply = dispatch({"id": 2, "method": "jira.default", "params": {
            "key": "maxResults", "value": "x"}})
        self.assertTrue(reply["ok"], reply)
        self.assertFalse(reply["result"]["ok"])

    def test_jira_status_text_methods(self):
        reply = dispatch({"id": 1, "method": "jira.progress_text", "params": {"progress": {"message": "x"}}})
        self.assertTrue(reply["ok"], reply)
        self.assertEqual(reply["result"]["text"], "x")
        reply = dispatch({"id": 2, "method": "jira.header_state", "params": {"enabled": False, "setupDone": True}})
        self.assertTrue(reply["ok"], reply)
        self.assertEqual(reply["result"]["line"], "○ Polling off")


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

    def test_a_slow_call_does_not_block_fast_calls(self):
        code, replies, _ = self.serve([
            line({"id": 1, "method": "script.run", "params": {
                "folder": FIXTURES, "script": "sleepy_script.py", "args": ["1"]}}),
            line({"id": 2, "method": "ping"}),
            line({"id": 3, "method": "ping"}),
            line({"id": 4, "method": "shutdown"}),
        ], replies=4)
        self.assertEqual(code, 0)
        ids = [r["id"] for r in replies]
        self.assertLess(ids.index(2), ids.index(1), "fast calls overtake the slow one")
        self.assertLess(ids.index(3), ids.index(1), "fast calls overtake the slow one")
        self.assertEqual(ids[-1], 4, "shutdown answers last, after the drain")
        self.assertTrue(self.by_id(replies)[1]["ok"])

    def test_errors_do_not_poison_other_calls(self):
        code, replies, _ = self.serve([
            line({"id": 1, "method": "nope"}),
            line({"id": 2, "method": "script.run", "params": {
                "folder": FIXTURES, "script": "fail_script.py"}}),
            "{nope\n",
            line({"id": 3, "method": "ping"}),
            line({"id": 4, "method": "shutdown"}),
        ], replies=5)
        self.assertEqual(code, 0)
        got = self.by_id(replies)
        self.assertFalse(got[1]["ok"])
        self.assertTrue(got[2]["ok"])
        self.assertEqual(got[2]["result"]["code"], 3)
        self.assertTrue(got[3]["ok"])
        self.assertFalse(got[None]["ok"])

    def test_shutdown_drains_in_flight(self):
        code, replies, _ = self.serve([
            line({"id": 1, "method": "script.run", "params": {
                "folder": FIXTURES, "script": "sleepy_script.py", "args": ["0.6"]}}),
            line({"id": 2, "method": "shutdown"}),
        ], replies=2)
        self.assertEqual(code, 0)
        self.assertEqual([r["id"] for r in replies], [1, 2])
        self.assertTrue(replies[0]["ok"])

    def test_eof_drains_in_flight(self):
        code, replies, _ = self.serve([
            line({"id": 1, "method": "script.run", "params": {
                "folder": FIXTURES, "script": "sleepy_script.py", "args": ["0.6"]}}),
        ], replies=1)
        self.assertEqual(code, 0)
        self.assertTrue(replies[0]["ok"])


if __name__ == "__main__":
    unittest.main(verbosity=2)
