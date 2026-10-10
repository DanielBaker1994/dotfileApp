#!/usr/bin/env python3
"""status — the switcher's CPU / RAM / battery / unread samplers (was SwitcherStatus.swift).

    python3 Tests/test_status.py
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

import status as st


def call(method, params):
    env = dict(os.environ)
    env["PYTHONPATH"] = os.path.join(ROOT, "pylib")
    env["PYTHONDONTWRITEBYTECODE"] = "1"
    r = subprocess.run(
        [sys.executable, "-B", "-m", "helper", "--call", method, "--params",
         json.dumps(params)],
        capture_output=True, text=True, cwd=ROOT, env=env,
    )
    return r.returncode, json.loads(r.stdout)


class Samplers(unittest.TestCase):
    def test_cpu_ticks(self):
        a = st.cpu_ticks()
        self.assertIsNotNone(a)
        self.assertGreaterEqual(a[1], a[0])
        self.assertGreater(a[1], 0)

    def test_ram(self):
        ram = st.ram_percent()
        self.assertIsNotNone(ram)
        self.assertTrue(0 <= ram <= 100, ram)

    def test_battery(self):
        b = st.battery()
        if b is not None:
            self.assertTrue(0 <= b["pct"] <= 100, b)
            self.assertIsInstance(b["charging"], bool)


class Unread(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.mkdtemp(prefix="status-test-")
        self.addCleanup(shutil.rmtree, self.dir, True)

    def stub(self, payload):
        path = os.path.join(self.dir, "stub.py")
        with open(path, "w") as f:
            f.write("import json, sys\nprint(json.dumps(%r))\nsys.exit(0)\n" % payload)
        return path

    def test_missing_script(self):
        self.assertEqual(st.read_unread(self.dir + "/nope.py"), [])

    def test_counts(self):
        script = self.stub({"sources": [
            {"app": "Notes", "count": 0},
            {"app": "Jira", "count": 5, "mentions": 2, "warn": True},
            {"app": "Mail", "count": 1200},
            {"app": "Chat", "count": "•"},
            {"app": "NoCount"},
        ]})
        chips = st.read_unread(script)
        self.assertEqual([c["app"] for c in chips], ["Notes", "Jira", "Mail", "Chat", "NoCount"])
        self.assertEqual([c["count"] for c in chips], ["", "5", "999+", "•", ""])
        self.assertEqual(chips[1]["mentions"], 2)
        self.assertTrue(chips[1]["warn"])

    def test_broken_script(self):
        path = os.path.join(self.dir, "bad.py")
        with open(path, "w") as f:
            f.write("raise SystemExit(3)\n")
        self.assertEqual(st.read_unread(path), [])


class Gather(unittest.TestCase):
    def test_gather_shape(self):
        tmp = tempfile.mkdtemp(prefix="status-gather-")
        self.addCleanup(shutil.rmtree, tmp, True)
        script = os.path.join(tmp, "stub.py")
        with open(script, "w") as f:
            f.write('import json\nprint(json.dumps({"sources": [{"app": "Notes", "count": 3}]}))\n')
        g = st.gather(script, delay=0.05)
        self.assertIn("cpu", g)
        self.assertIn("ram", g)
        self.assertIn("battery", g)
        self.assertEqual(g["chips"], [{"app": "Notes", "count": "3", "mentions": 0, "warn": False}])
        if g["cpu"] is not None:
            self.assertTrue(0 <= g["cpu"] <= 100 * 64, g["cpu"])


class Protocol(unittest.TestCase):
    def test_gather(self):
        tmp = tempfile.mkdtemp(prefix="status-proto-")
        self.addCleanup(shutil.rmtree, tmp, True)
        script = os.path.join(tmp, "stub.py")
        with open(script, "w") as f:
            f.write('import json\nprint(json.dumps({"sources": [{"app": "Mail", "count": 7}]}))\n')
        code, reply = call("status.gather", {"notify_script": script, "delay": 0.02})
        self.assertEqual(code, 0)
        self.assertEqual(reply["result"]["chips"][0]["app"], "Mail")
        self.assertEqual(reply["result"]["chips"][0]["count"], "7")


if __name__ == "__main__":
    unittest.main(verbosity=2)
