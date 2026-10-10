#!/usr/bin/env python3
"""jsonmgr — the one JSON data manager (PLAN-json-manager, wave M0).

    python3 Tests/test_jsonmgr.py

Covers: the static registry (every home exists, parses, no unregistered
shipped JSON), the error style (JsonError, never cached), mtime snapshot
semantics, the access ledger + WS_JSON_TRACE, lazy_module exports, thread
safety and the CLI (--report/--json/--check, 3.9).
"""
from __future__ import annotations

import importlib.util
import io
import json
import os
import shutil
import subprocess
import sys
import tempfile
import threading
import unittest
import contextlib

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(ROOT, "pylib"))

import jsonmgr  # noqa: E402

JSONMGR_PY = os.path.join(ROOT, "pylib", "jsonmgr.py")
SHIPPED_DIRS = ("pylib", "jira", "confluence", "notify", "settings_hub", "vim")
ALLOWED_UNREGISTERED = ("jira/team.example.json", "vim/snippets/markdown.json")
# the preseeded shipped homes, before any test registers a throwaway name
PRESEEDED = dict(jsonmgr._HOMES)


def clean_env(**extra):
    env = {k: v for k, v in os.environ.items()
           if k not in ("WS_JSON_ROOT", "WS_JSON_TRACE")}
    env.update(extra)
    return env


class TempRootCase(unittest.TestCase):
    """A temp root: every home this test uses is a throwaway name."""

    def setUp(self):
        self.root = os.path.realpath(tempfile.mkdtemp(prefix="jsonmgr-test-"))
        self.addCleanup(shutil.rmtree, self.root, True)
        self.addCleanup(os.environ.pop, "WS_JSON_ROOT", None)
        self.addCleanup(os.environ.pop, "WS_JSON_TRACE", None)
        os.environ["WS_JSON_ROOT"] = self.root
        os.environ.pop("WS_JSON_TRACE", None)

    def write(self, name, payload):
        path = os.path.join(self.root, name + ".json")
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w", encoding="utf-8") as fh:
            json.dump(payload, fh)
        jsonmgr.register(name)
        return path

    def touch_mtime(self, path):
        st = os.stat(path)
        os.utime(path, ns=(st.st_atime_ns, st.st_mtime_ns + 1_000_000_000))

    def parses(self, name):
        for row in jsonmgr.report()["homes"]:
            if row["name"] == name:
                return row["parses"]
        raise KeyError(name)


class Registry(unittest.TestCase):
    def test_every_home_exists_and_parses_to_dict(self):
        for name, rel in sorted(PRESEEDED.items()):
            self.assertEqual(name + ".json", rel, name)
            data = jsonmgr.load(name)
            self.assertIsInstance(data, dict, name)

    def test_no_unregistered_shipped_json(self):
        known = {os.path.normpath(rel) for rel in PRESEEDED.values()}
        known |= {os.path.normpath(p) for p in ALLOWED_UNREGISTERED}
        found = []
        for top in SHIPPED_DIRS:
            for dirpath, _dirs, files in os.walk(os.path.join(ROOT, top)):
                for f in files:
                    if f.endswith(".json"):
                        rel = os.path.relpath(os.path.join(dirpath, f), ROOT)
                        found.append(os.path.normpath(rel))
        self.assertEqual([], sorted(set(found) - known),
                         "shipped JSON must be registered (or allowlisted)")

    def test_allowlisted_non_homes_exist(self):
        for rel in ALLOWED_UNREGISTERED:
            self.assertTrue(os.path.exists(os.path.join(ROOT, rel)), rel)


class Errors(TempRootCase):
    def test_missing_home(self):
        jsonmgr.register("t/err-missing")
        with self.assertRaises(jsonmgr.JsonError) as ctx:
            jsonmgr.load("t/err-missing")
        e = ctx.exception
        self.assertEqual(e.kind, "missing")
        self.assertEqual(e.name, "t/err-missing")
        self.assertIn("t/err-missing", str(e))
        self.assertIn(os.path.join(self.root, "t", "err-missing.json"), str(e))

    def test_failure_is_not_cached(self):
        jsonmgr.register("t/err-retry")
        with self.assertRaises(jsonmgr.JsonError):
            jsonmgr.load("t/err-retry")
        self.write("t/err-retry", {"value": 1})
        self.assertEqual(jsonmgr.load("t/err-retry")["value"], 1)

    def test_malformed(self):
        path = self.write("t/err-bad", {})
        with open(path, "w", encoding="utf-8") as fh:
            fh.write("{nope")
        jsonmgr.register("t/err-bad")
        with self.assertRaises(jsonmgr.JsonError) as ctx:
            jsonmgr.load("t/err-bad")
        self.assertEqual(ctx.exception.kind, "malformed")
        self.assertIn("malformed JSON", str(ctx.exception))
        with open(path, "w", encoding="utf-8") as fh:
            fh.write('{"value": 2}')
        self.assertEqual(jsonmgr.load("t/err-bad")["value"], 2)

    def test_non_dict_root_is_shape(self):
        self.write("t/err-shape", [1, 2])
        jsonmgr.register("t/err-shape")
        with self.assertRaises(jsonmgr.JsonError) as ctx:
            jsonmgr.load("t/err-shape")
        self.assertEqual(ctx.exception.kind, "shape")

    def test_not_a_value_error(self):
        self.assertFalse(issubclass(jsonmgr.JsonError, ValueError))

    def test_unknown_home(self):
        with self.assertRaises(jsonmgr.JsonError) as ctx:
            jsonmgr.load("t/never-registered")
        self.assertEqual(ctx.exception.kind, "shape")

    def test_field_missing_key_and_get_default(self):
        self.write("t/err-keys", {"value": 1})
        jsonmgr.register("t/err-keys")
        with self.assertRaises(jsonmgr.JsonError) as ctx:
            jsonmgr.field("t/err-keys", "nope")
        self.assertEqual(ctx.exception.kind, "shape")
        self.assertEqual(jsonmgr.get("t/err-keys", "nope", default=7), 7)
        self.assertEqual(jsonmgr.get("t/err-keys", "value"), 1)
        self.assertEqual(jsonmgr.field("t/err-keys", "value"), 1)


class Mtime(TempRootCase):
    def test_snapshot_until_key_changes(self):
        path = self.write("t/mtime", {"value": 1})
        jsonmgr.register("t/mtime")
        first = jsonmgr.load("t/mtime")
        self.assertIs(first, jsonmgr.load("t/mtime"))
        self.assertEqual(self.parses("t/mtime"), 1)

    def test_plain_load_stats_and_reparses(self):
        path = self.write("t/mtime-edit", {"value": 1})
        jsonmgr.register("t/mtime-edit")
        first = jsonmgr.load("t/mtime-edit")
        self.write("t/mtime-edit", {"value": 2})
        self.touch_mtime(path)
        second = jsonmgr.load("t/mtime-edit")
        self.assertIsNot(first, second)
        self.assertEqual(second["value"], 2)

    def test_refresh_forces_reparse(self):
        self.write("t/mtime-refresh", {"value": 1})
        jsonmgr.register("t/mtime-refresh")
        first = jsonmgr.load("t/mtime-refresh")
        second = jsonmgr.load("t/mtime-refresh", refresh=True)
        self.assertIsNot(first, second)
        self.assertEqual(second, first)
        self.assertEqual(self.parses("t/mtime-refresh"), 2)

    def test_reload_drops_and_rereads(self):
        path = self.write("t/mtime-reload", {"value": 1})
        jsonmgr.register("t/mtime-reload")
        old = jsonmgr.load("t/mtime-reload")
        self.write("t/mtime-reload", {"value": 3})
        self.touch_mtime(path)
        jsonmgr.reload("t/mtime-reload")
        self.assertEqual(jsonmgr.load("t/mtime-reload")["value"], 3)
        self.assertIsNot(old, jsonmgr.load("t/mtime-reload"))

    def test_failure_drops_stale_snapshot(self):
        path = self.write("t/mtime-stale", {"value": 1})
        jsonmgr.register("t/mtime-stale")
        jsonmgr.load("t/mtime-stale")
        with open(path, "w", encoding="utf-8") as fh:
            fh.write("{broken")
        self.touch_mtime(path)
        with self.assertRaises(jsonmgr.JsonError):
            jsonmgr.load("t/mtime-stale")
        self.write("t/mtime-stale", {"value": 5})
        self.assertEqual(jsonmgr.load("t/mtime-stale")["value"], 5)


class Ledger(TempRootCase):
    def _fixture_module(self, source, name):
        path = os.path.join(self.root, name + ".py")
        with open(path, "w", encoding="utf-8") as fh:
            fh.write(source)
        spec = importlib.util.spec_from_file_location(name, path)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        return module

    def test_consumer_module_and_keypath_dedupe(self):
        self.write("t/ledger", {"value": 41})
        mod = self._fixture_module(
            "import jsonmgr\n\n"
            "def touch():\n"
            "    return jsonmgr.field('t/ledger', 'value')\n",
            "jsonmgr_ledger_fixture")
        self.assertEqual(mod.touch(), 41)
        self.assertEqual(mod.touch(), 41)
        rows = [r for r in jsonmgr.ledger()
                if r["consumer"] == "jsonmgr_ledger_fixture"
                and r["home"] == "t/ledger" and r["keypath"] == "value"]
        self.assertEqual(len(rows), 1, rows)
        self.assertEqual(rows[0]["count"], 2)

    def test_trace_mode_records_indexing(self):
        self.write("t/trace", {"outer": {"inner": 5}, "scalar": 1})
        jsonmgr.register("t/trace")
        os.environ["WS_JSON_TRACE"] = "1"
        data = jsonmgr.load("t/trace")
        self.assertIsInstance(data, dict)
        self.assertNotEqual(type(data), dict)
        buf = io.StringIO()
        with contextlib.redirect_stderr(buf):
            self.assertEqual(data["outer"]["inner"], 5)
            self.assertEqual(data["scalar"], 1)
        lines = buf.getvalue()
        self.assertIn("jsonmgr: %s -> t/trace -> outer\n" % __name__, lines)
        self.assertIn("jsonmgr: %s -> t/trace -> outer.inner\n" % __name__, lines)
        self.assertIn("jsonmgr: %s -> t/trace -> scalar\n" % __name__, lines)

    def test_trace_off_is_a_plain_dict(self):
        self.write("t/no-trace", {"value": 1})
        jsonmgr.register("t/no-trace")
        self.assertEqual(type(jsonmgr.load("t/no-trace")), dict)


class Lazy(TempRootCase):
    def _lazy_module(self, name, exports_source, extra=""):
        path = os.path.join(self.root, name + ".py")
        with open(path, "w", encoding="utf-8") as fh:
            fh.write("import jsonmgr\n\n"
                     "jsonmgr.lazy_module(__name__, globals(), %s)\n%s"
                     % (exports_source, extra))
        spec = importlib.util.spec_from_file_location(name, path)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        return module

    def test_exports_resolve_cast_cache_and_dir(self):
        self.write("t/lazy", {"value": 5, "items": [1, 2]})
        mod = self._lazy_module(
            "jsonmgr_lazy_fixture",
            "{'LAZY': ('t/lazy', ('value',)), "
            "'LAZY_T': ('t/lazy', ('items',), tuple), "
            "'LAZY_CALL': lambda: jsonmgr.field('t/lazy', 'value') + 1}")
        self.assertEqual(mod.LAZY, 5)
        self.assertEqual(mod.LAZY_T, (1, 2))
        self.assertEqual(mod.LAZY_CALL, 6)
        self.assertEqual(mod.__dict__["LAZY"], 5)
        for name in ("LAZY", "LAZY_T", "LAZY_CALL"):
            self.assertIn(name, dir(mod))

    def test_failure_is_retried_not_cached(self):
        mod = self._lazy_module(
            "jsonmgr_lazy_retry",
            "{'LAZY': ('t/lazy-retry', ('value',))}")
        with self.assertRaises(jsonmgr.JsonError):
            mod.LAZY
        self.assertNotIn("LAZY", mod.__dict__)
        self.write("t/lazy-retry", {"value": 9})
        self.assertEqual(mod.LAZY, 9)

    def test_bare_internal_use_is_name_error(self):
        self.write("t/lazy-bare", {"value": 1})
        mod = self._lazy_module(
            "jsonmgr_lazy_bare",
            "{'BARE': ('t/lazy-bare', ('value',))}",
            extra="\ndef bare():\n    return BARE\n")
        with self.assertRaises(NameError):
            mod.bare()

    def test_unknown_attribute_raises_attribute_error(self):
        mod = self._lazy_module("jsonmgr_lazy_unknown", "{'X': ('t/lazy', ('value',))}")
        with self.assertRaises(AttributeError):
            mod.NOPE


class Threads(TempRootCase):
    def test_cold_field_is_one_parse_and_consistent(self):
        self.write("t/threads", {"value": 7})
        jsonmgr.register("t/threads")
        results = []
        errors = []

        def worker():
            try:
                results.append(jsonmgr.field("t/threads", "value"))
            except Exception as e:  # noqa: BLE001
                errors.append(e)

        threads = [threading.Thread(target=worker) for _ in range(8)]
        for t in threads:
            t.start()
        for t in threads:
            t.join()
        self.assertEqual(errors, [])
        self.assertEqual(results, [7] * 8)
        self.assertEqual(self.parses("t/threads"), 1)


class CLI(unittest.TestCase):
    def run_cli(self, *args, python=None, env=None):
        return subprocess.run(
            [python or sys.executable, JSONMGR_PY] + list(args),
            capture_output=True, text=True, env=env or clean_env())

    def test_report_json(self):
        run = self.run_cli("--report", "--json")
        self.assertEqual(run.returncode, 0, run.stderr)
        rep = json.loads(run.stdout)
        self.assertEqual(len(rep["homes"]), len(jsonmgr._HOMES))
        self.assertTrue(all(h["exists"] for h in rep["homes"]))
        self.assertIn("ledger", rep)

    def test_check_ok(self):
        run = self.run_cli("--report", "--check")
        self.assertEqual(run.returncode, 0, run.stderr)

    def test_check_fails_on_bad_root(self):
        tmp = tempfile.mkdtemp(prefix="jsonmgr-cli-")
        self.addCleanup(shutil.rmtree, tmp, True)
        run = self.run_cli("--report", "--check", "--root", tmp)
        self.assertEqual(run.returncode, 1)
        self.assertIn("missing", run.stderr)

    def test_runs_under_python39(self):
        python39 = "/usr/bin/python3"
        if not os.path.exists(python39):
            self.skipTest("/usr/bin/python3 not present")
        run = self.run_cli("--report", "--json", python=python39)
        self.assertEqual(run.returncode, 0, run.stderr)
        self.assertEqual(len(json.loads(run.stdout)["homes"]), len(jsonmgr._HOMES))

    def test_unknown_argument(self):
        self.assertEqual(self.run_cli("--wat").returncode, 2)


if __name__ == "__main__":
    unittest.main(verbosity=2)
