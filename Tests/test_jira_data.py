#!/usr/bin/env python3
"""jira data glue — the word lists and the status categories.

    python3 Tests/test_jira_data.py
"""
from __future__ import annotations

import os
import sys
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(ROOT, "pylib"))

import jira_data as jd


class Words(unittest.TestCase):
    def test_defaults_when_config_is_missing(self):
        w = jd.words({})
        self.assertIn("done", w["done"])
        self.assertIn("won't", w["cancelled"])
        self.assertIn("p0", w["urgent"])

    def test_config_replaces_only_that_list(self):
        w = jd.words({"status-done-words": "shipped, nailed"})
        self.assertEqual(w["done"], ["shipped", "nailed"])
        self.assertIn("cancel", w["cancelled"])

    def test_casing_trim_and_empty_parts(self):
        w = jd.words({"status-done-words": " Done , ,DONE,  "})
        self.assertEqual(w["done"], ["done", "done"])

    def test_blank_config_value_falls_back_to_default(self):
        w = jd.words({"status-done-words": ""})
        self.assertIn("resolved", w["done"])

    def test_matches_is_case_ready_substring_any(self):
        self.assertTrue(jd.matches("in progress", ["progress", "review"]))
        self.assertFalse(jd.matches("done", ["don't", "pending"]))


class Category(unittest.TestCase):
    def words(self, values=None):
        return jd.words(values or {})

    def test_directory_map_wins_and_is_exact(self):
        cats = {"To Do": "new", "Review": "indeterminate", "Finished": "done"}
        w = self.words()
        self.assertEqual(jd.category("To Do", cats, w), 0)
        self.assertEqual(jd.category("Review", cats, w), 1)
        self.assertEqual(jd.category("Finished", cats, w), 2)
        # map is exact-match; the word fallback is case-insensitive
        self.assertEqual(jd.category("to do", cats, w), 0)

    def test_word_fallback_done_and_cancelled(self):
        w = self.words()
        self.assertEqual(jd.category("Shipped", {}, w), 2)
        self.assertEqual(jd.category("Won't Do", {}, w), 2)
        self.assertEqual(jd.category("Backlog", {}, w), 0)
        self.assertEqual(jd.category("In review", {}, w), 1)

    def test_word_fallback_uses_configured_words(self):
        w = self.words({"status-done-words": "nailed"})
        self.assertEqual(jd.category("nailed it", {}, w), 2)
        self.assertEqual(jd.category("shipped", {}, w), 1)  # no longer a done word

    def test_no_words_at_all_defaults_to_in_progress(self):
        self.assertEqual(jd.category("anything", {}, {}), 1)


if __name__ == "__main__":
    unittest.main(verbosity=2)
