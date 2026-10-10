#!/usr/bin/env python3
"""setup checks — preflight JSON models + the summary line.

    python3 Tests/test_setup_checks.py
"""
from __future__ import annotations

import os
import sys
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(ROOT, "pylib"))

import setup_checks as sc


def check(**over):
    d = {"id": "python", "group": "core", "level": "required", "title": "Python",
         "detail": "python 3.11+", "fix": "brew install python", "action": "brew", "ok": True}
    d.update(over)
    return d


class Parse(unittest.TestCase):
    def test_fields_and_defaults(self):
        out = sc.parse_checks([check(extra=1), {"title": 7}, "nope", None])
        self.assertEqual(out[0]["title"], "Python")
        self.assertTrue(out[0]["ok"])
        self.assertEqual(out[1], {"id": "", "group": "", "level": "", "title": "",
                                  "detail": "", "fix": "", "action": "", "ok": False})

    def test_ok_is_strict(self):
        self.assertFalse(sc.parse_checks([check(ok=1)])[0]["ok"])

    def test_non_list(self):
        self.assertEqual(sc.parse_checks(None), [])


class Summarize(unittest.TestCase):
    def test_required_failure_wins(self):
        out = sc.summarize([check(ok=False, title="Python", detail="missing"),
                            check(id="x", group="features", level="optional", ok=False)])
        self.assertEqual(out, {"message": "Python: missing", "tone": "danger"})

    def test_warned_count(self):
        out = sc.summarize([check(), check(id="a", group="features", level="optional", ok=False)])
        self.assertEqual(out["message"], "Ready. 1 optional feature is off — see the list.")
        self.assertEqual(out["tone"], "warning")
        out = sc.summarize([check()] + [check(id=str(i), group="features", level="optional", ok=False)
                                        for i in range(2)])
        self.assertEqual(out["message"], "Ready. 2 optional features are off — see the list.")

    def test_stack_only(self):
        out = sc.summarize([check(), check(id="b", group="stack", level="optional", ok=False)])
        self.assertEqual(out["tone"], "dim")
        self.assertIn("Hotkeys", out["message"])

    def test_all_green(self):
        self.assertEqual(sc.summarize([check()]), {"message": "Everything is in place.",
                                                   "tone": "success"})
        self.assertEqual(sc.summarize(None)["tone"], "success")


if __name__ == "__main__":
    unittest.main(verbosity=2)
