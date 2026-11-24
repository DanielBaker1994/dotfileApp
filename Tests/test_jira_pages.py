#!/usr/bin/env python3
"""jira pages — the status category names the board and directory pickers use.

    python3 Tests/test_jira_pages.py
"""
from __future__ import annotations

import os
import sys
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(ROOT, "pylib"))

import jira_pages as jp


class CategoryNames(unittest.TestCase):
    def test_three_non_empty_status_categories(self):
        self.assertEqual(len(jp.CATEGORY_NAMES), 3)
        self.assertTrue(all(isinstance(n, str) and n for n in jp.CATEGORY_NAMES))


if __name__ == "__main__":
    unittest.main(verbosity=2)
