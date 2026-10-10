#!/usr/bin/env python3
"""jira dashboard edits — job draft assembly, live payload, limit parsing.

    python3 Tests/test_jira_dashboard.py
"""
from __future__ import annotations

import os
import sys
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(ROOT, "pylib"))

import jira_dashboard as jdash


class Args(unittest.TestCase):
    def test_pairs_trim_and_first_equals_splits(self):
        self.assertEqual(jdash.parse_args("a=1, b=2=x, bad, =x, c ="),
                         {"a": "1", "b": "2=x", "c": ""})

    def test_empty(self):
        self.assertEqual(jdash.parse_args(""), {})
        self.assertEqual(jdash.parse_args("  ,  "), {})


class Limit(unittest.TestCase):
    def test_empty_is_default_zero(self):
        self.assertEqual(jdash.check_limit("", "Page size"), {"ok": True, "value": 0})
        self.assertEqual(jdash.check_limit("   ", "Page size"), {"ok": True, "value": 0})

    def test_whole_numbers(self):
        self.assertEqual(jdash.check_limit(" 25 ", "Page size"), {"ok": True, "value": 25})
        self.assertEqual(jdash.check_limit("+3", "Page size"), {"ok": True, "value": 3})
        self.assertEqual(jdash.check_limit("0", "Page size"), {"ok": True, "value": 0})

    def test_failures_carry_the_message(self):
        for bad in ["x", "3.5", "-1", "1 000"]:
            out = jdash.check_limit(bad, "Max issues")
            self.assertFalse(out["ok"], bad)
            self.assertEqual(out["message"], "✗ Max issues must be a whole number (empty = default)")


class Draft(unittest.TestCase):
    def params(self, **over):
        p = {"kind": "job", "name": " team-bugs ", "type": "issues",
             "pageSize": "50", "maxTotal": "500", "window": " 30m ",
             "enabled": True, "projects": ["A"], "projectsAll": False,
             "queryIndex": 0, "jql": "", "jobTitle": "", "args": "",
             "columns": "key,title"}
        p.update(over)
        return p

    def test_basic_job(self):
        out = jdash.draft(self.params())
        self.assertTrue(out["ok"])
        self.assertEqual(out["draft"], {
            "name": "team-bugs", "type": "issues", "maxResults": 50, "maxTotal": 500,
            "window": "30m", "enabled": True, "columns": "key,title",
            "projects": ["A"], "jql": "", "job": "", "args": {},
        })

    def test_projects_star_when_all_or_empty(self):
        self.assertEqual(jdash.draft(self.params(projectsAll=True))["draft"]["projects"], "*")
        self.assertEqual(jdash.draft(self.params(projects=[]))["draft"]["projects"], "*")

    def test_directory_type_drops_columns(self):
        self.assertNotIn("columns", jdash.draft(self.params(type="directory"))["draft"])

    def test_jql_only_for_issues_query_one(self):
        self.assertEqual(jdash.draft(self.params(queryIndex=1, jql=" x "))["draft"]["jql"], "x")
        self.assertEqual(jdash.draft(self.params(queryIndex=1, jql=" x ", type="directory"))["draft"]["jql"], "")

    def test_job_slice_and_args_for_team_jobs(self):
        out = jdash.draft(self.params(queryIndex=2, jobTitle="team.json: night-audit",
                                      args=" days=30,label=ops"))
        self.assertEqual(out["draft"]["job"], "night-audit")
        self.assertEqual(out["draft"]["args"], {"days": "30", "label": "ops"})
        short = jdash.draft(self.params(queryIndex=2, jobTitle="ab"))["draft"]["job"]
        self.assertEqual(short, "")

    def test_limit_failure_short_circuits(self):
        out = jdash.draft(self.params(pageSize="nope"))
        self.assertFalse(out["ok"])
        self.assertIn("Page size", out["message"])
        self.assertNotIn("draft", out)

    def test_live_payload(self):
        out = jdash.live_persist({"maxResults": "25", "columns": "key,status"})
        self.assertEqual(out, {"ok": True, "draft": {"columns": "key,status", "maxResults": 25}})
        self.assertFalse(jdash.live_persist({"maxResults": "-2"})["ok"])


if __name__ == "__main__":
    unittest.main(verbosity=2)
