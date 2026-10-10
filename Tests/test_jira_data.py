#!/usr/bin/env python3
"""jira data glue — the word lists, status categories, workflow steps and the
cell-style rule table the app evaluates per cell.

    python3 Tests/test_jira_data.py
"""
from __future__ import annotations

import json
import os
import shutil
import sys
import tempfile
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


class Workflow(unittest.TestCase):
    def test_absent_or_empty_is_none(self):
        self.assertIsNone(jd.workflow_steps(None))
        self.assertIsNone(jd.workflow_steps(""))
        self.assertIsNone(jd.workflow_steps("  , , "))

    def test_steps_split_trim_and_keep_order(self):
        self.assertEqual(jd.workflow_steps("To Do, In Progress , Done"),
                         ["To Do", "In Progress", "Done"])


class StyleRules(unittest.TestCase):
    def rules(self, values=None):
        return jd.style_rules(values or {})

    def test_status_rule_order_cancelled_then_done(self):
        r = self.rules()
        self.assertEqual([x["words"] for x in r["status"][:2]],
                         [r["words"]["cancelled"], r["words"]["done"]])
        self.assertEqual(r["status"][0]["style"]["tone"], "dim")
        self.assertTrue(r["status"][0]["style"]["quietsRow"])
        self.assertEqual(r["status"][1]["style"]["mark"], "filled")

    def test_rules_embed_configured_words(self):
        r = self.rules({"status-blocked-words": "stuck"})
        self.assertEqual(r["status"][2]["words"], ["stuck"])

    def test_dim_fields_are_data(self):
        self.assertIn("key", self.rules()["dimFields"])
        self.assertIn("release", self.rules()["dimFields"])

    def test_priority_urgent_and_fallback(self):
        r = self.rules()
        self.assertEqual(r["priority"][0]["style"]["tone"], "danger")
        self.assertTrue(r["priority"][0]["style"]["bold"])
        self.assertEqual(r["priorityFallback"], {"tone": "dim"})

    def test_release_status_order_unreleased_first(self):
        r = self.rules()
        self.assertEqual([x["contains"] for x in r["releaseStatus"]], ["unreleased", "released"])
        self.assertEqual(r["releaseStatus"][0]["style"]["tone"], "warning")

    def test_status_fallback(self):
        self.assertEqual(self.rules()["statusFallback"], {"tone": "dim", "mark": "hollow"})


class Comments(unittest.TestCase):
    def setUp(self):
        self.root = tempfile.mkdtemp(prefix="jira-comments-")
        self.addCleanup(shutil.rmtree, self.root, True)
        self.path = os.path.join(self.root, "issues.json")

    def write(self, data):
        with open(self.path, "w", encoding="utf-8") as fh:
            json.dump(data, fh)

    def test_rows_for_one_key(self):
        self.write({
            "A-1": {"comments": [
                {"author": "ada", "body": "hi", "created": "2026-01-01T00:00:00Z"},
                {"author": "bob", "body": "yo", "created": "2026-01-02T00:00:00Z"},
            ]},
            "A-2": {"comments": [{"author": "x", "body": "y", "created": "z"}]},
        })
        out = jd.comments(self.path, "A-1")
        self.assertGreater(out["stamp"], 0)
        self.assertEqual([c["author"] for c in out["comments"]], ["ada", "bob"])
        self.assertEqual(jd.comments(self.path, "A-2")["comments"],
                         [{"author": "x", "body": "y", "created": "z"}])

    def test_missing_key_and_issues_without_comments(self):
        self.write({"A-1": {"comments": []}, "A-2": {}, "A-3": "not a dict"})
        self.assertEqual(jd.comments(self.path, "A-1")["comments"], [])
        self.assertEqual(jd.comments(self.path, "A-2")["comments"], [])
        self.assertEqual(jd.comments(self.path, "nope")["comments"], [])

    def test_non_string_fields_become_empty(self):
        self.write({"A-1": {"comments": [{"author": 7, "body": None, "created": ["x"]}]}})
        self.assertEqual(jd.comments(self.path, "A-1")["comments"],
                         [{"author": "", "body": "", "created": ""}])

    def test_a_non_dict_comment_skips_the_whole_issue(self):
        self.write({"A-1": {"comments": ["nope"]}})
        self.assertEqual(jd.comments(self.path, "A-1")["comments"], [])

    def test_broken_json_reads_as_empty(self):
        with open(self.path, "w") as fh:
            fh.write("{nope")
        self.assertEqual(jd.comments(self.path, "A-1")["comments"], [])

    def test_missing_file(self):
        self.assertEqual(jd.comments(os.path.join(self.root, "none.json"), "A-1"),
                         {"stamp": 0, "comments": []})

    def test_parsed_once_per_mtime(self):
        self.write({"A-1": {"comments": [{"author": "one", "body": "1", "created": "c1"}]}})
        first = jd.comments(self.path, "A-1")
        st = os.stat(self.path)
        self.write({"A-1": {"comments": [{"author": "two", "body": "2", "created": "c2"}]}})
        os.utime(self.path, ns=(st.st_atime_ns, st.st_mtime_ns))  # same mtime: cached
        self.assertEqual(jd.comments(self.path, "A-1"), first)
        os.utime(self.path, ns=(st.st_atime_ns, st.st_mtime_ns + 1_000_000_000))
        self.assertEqual(jd.comments(self.path, "A-1")["comments"][0]["author"], "two")


if __name__ == "__main__":
    unittest.main(verbosity=2)
