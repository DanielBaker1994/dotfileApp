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


class SnakeCase(unittest.TestCase):
    def test_words_symbols_and_leading_digit(self):
        self.assertEqual(jdash.snake_case("Story Points"), "story_points")
        self.assertEqual(jdash.snake_case("  Type-of  thing__x "), "type_of_thing_x")
        self.assertEqual(jdash.snake_case("1st Place"), "f_1st_place")
        self.assertEqual(jdash.snake_case("already_ok"), "already_ok")
        self.assertEqual(jdash.snake_case(""), "")


class CustomField(unittest.TestCase):
    def params(self, **over):
        p = {"raw": "Story Points — customfield_10016", "alias": "", "isNew": True, "label": "",
             "description": "", "customs": [{"id": "customfield_10016", "name": "Story Points"}],
             "currentCustomFields": {}, "currentLabels": {}}
        p.update(over)
        return p

    def test_id_alias_and_label_defaults(self):
        out = jdash.custom_field_entry(self.params())
        self.assertTrue(out["ok"])
        self.assertEqual(out["alias"], "story_points")
        self.assertEqual(out["save"]["value"]["story_points"],
                         {"field_id": "customfield_10016", "label": "Story Points"})
        self.assertEqual(out["save"]["done"], "added story_points")
        self.assertIsNone(out["followup"])

    def test_name_match_is_case_insensitive(self):
        out = jdash.custom_field_entry(self.params(raw="story points"))
        self.assertEqual(out["save"]["value"]["story_points"]["field_id"], "customfield_10016")

    def test_unresolved_field(self):
        out = jdash.custom_field_entry(self.params(raw="nope", customs=[]))
        self.assertFalse(out["ok"])
        self.assertIn("customfield_NNNNN", out["message"])

    def test_alias_derives_from_label_then_id(self):
        out = jdash.custom_field_entry(self.params(raw="customfield_9", customs=[],
                                                   alias="", label="My Custom Label"))
        self.assertEqual(out["alias"], "my_custom_label")
        self.assertEqual(out["save"]["value"]["my_custom_label"]["label"], "My Custom Label")
        out = jdash.custom_field_entry(self.params(raw="customfield_9", customs=[],
                                                   alias="given", label=""))
        self.assertEqual(out["save"]["value"]["given"], {"field_id": "customfield_9", "label": "given"})

    def test_description_and_updated_done(self):
        out = jdash.custom_field_entry(self.params(alias="sp", isNew=False, description=" points "))
        self.assertEqual(out["save"]["value"]["sp"]["description"], "points")
        self.assertEqual(out["save"]["done"], "updated sp")

    def test_label_clear_followup(self):
        out = jdash.custom_field_entry(self.params(alias="sp", isNew=False,
                                                   currentLabels={"sp": "x", "k": "y"}))
        self.assertEqual(out["followup"]["value"], {"k": "y"})
        self.assertEqual(out["followup"]["done"], "label of sp")


class FieldLabel(unittest.TestCase):
    def test_custom_value_sets_and_reports(self):
        out = jdash.field_label_save({"field": "title", "value": " Heading ",
                                      "default": "Title", "current": {"k": "v"}})
        self.assertEqual(out["value"], {"k": "v", "title": "Heading"})
        self.assertEqual(out["done"], "title → “Heading”")

    def test_empty_or_default_removes(self):
        for value in ("", "   ", "Title"):
            out = jdash.field_label_save({"field": "title", "value": value,
                                          "default": "Title", "current": {"title": "Old"}})
            self.assertEqual(out["value"], {}, value)
            self.assertEqual(out["done"], "title back to “Title”")


class KeyValue(unittest.TestCase):
    def test_trim_and_save(self):
        out = jdash.key_value_save({"name": " my_bugs ", "value": " p in ({projects}) ",
                                    "existing": False, "current": {}})
        self.assertEqual(out["value"], {"my_bugs": "p in ({projects})"})
        self.assertEqual(out["done"], "added my_bugs")
        out = jdash.key_value_save({"name": "my_bugs", "value": "x", "existing": True, "current": {}})
        self.assertEqual(out["done"], "updated my_bugs")

    def test_empty_fields_beep(self):
        self.assertEqual(jdash.key_value_save({"name": " ", "value": "x"}),
                         {"ok": False, "beep": True})
        self.assertEqual(jdash.key_value_save({"name": "x", "value": " "}),
                         {"ok": False, "beep": True})


class Defaults(unittest.TestCase):
    def test_whole_number_saves(self):
        out = jdash.default_save({"key": "maxResults", "value": " 25 ", "current": {"k": 1}})
        self.assertEqual(out["value"], {"k": 1, "maxResults": 25})
        self.assertEqual(out["done"], "maxResults = 25")

    def test_bad_values_carry_the_message(self):
        for bad in ("x", "-1", "3.5", ""):
            out = jdash.default_save({"key": "pageSize", "value": bad})
            self.assertFalse(out["ok"], bad)
            self.assertEqual(out["message"], "✗ pageSize must be a whole number")


if __name__ == "__main__":
    unittest.main(verbosity=2)
