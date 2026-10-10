#!/usr/bin/env python3
"""jira setup — the project-key grammar.

    python3 Tests/test_jira_setup.py
"""
from __future__ import annotations

import os
import sys
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(ROOT, "pylib"))

import jira_setup as js


class ProjectKeys(unittest.TestCase):
    def test_commas_whitespace_and_uppercasing(self):
        self.assertEqual(js.project_keys("sam1, KAN"),
                         {"keys": ["SAM1", "KAN"], "bad": []})
        self.assertEqual(js.project_keys("sam1 kan\n  app2"),
                         {"keys": ["SAM1", "KAN", "APP2"], "bad": []})

    def test_dedupe_preserves_order(self):
        self.assertEqual(js.project_keys("B, a, b, A")["keys"], ["B", "A"])

    def test_underscores_and_digits_allowed_after_the_first_letter(self):
        self.assertEqual(js.project_keys("A_B2")["keys"], ["A_B2"])

    def test_bad_keys_are_reported(self):
        self.assertEqual(js.project_keys("1X, AB-C, x"),
                         {"keys": ["X"], "bad": ["1X", "AB-C"]})

    def test_empty(self):
        self.assertEqual(js.project_keys(""), {"keys": [], "bad": []})
        self.assertEqual(js.project_keys("  , , "), {"keys": [], "bad": []})


if __name__ == "__main__":
    unittest.main(verbosity=2)
