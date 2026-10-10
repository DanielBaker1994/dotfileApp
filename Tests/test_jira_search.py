#!/usr/bin/env python3
"""jira search glue — filter menu model + criteria assembly.

    python3 Tests/test_jira_search.py
"""
from __future__ import annotations

import os
import sys
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(ROOT, "pylib"))

import jira_search as js


class FilterKinds(unittest.TestCase):
    def keys(self, catalog=None):
        return [k["key"] for k in js.filter_kinds(catalog or [])]

    def test_base_kinds_in_order(self):
        kinds = js.filter_kinds([])
        self.assertEqual([k["key"] for k in kinds][:5],
                         ["assignee", "reporter", "status", "statusCategory", "issuetype"])
        self.assertEqual([k["kind"] for k in kinds][8:11], ["date", "date", "date"])

    def test_catalog_labels_override_titles(self):
        kinds = js.filter_kinds([
            {"field": "assignee", "label": "Owner"},
            {"field": "release", "label": "Fix Version"},
            {"field": "updated", "label": "Changed"},
            {"field": "title", "label": ""},  # empty -> fallback
        ])
        by_key = {k["key"]: k["title"] for k in kinds}
        self.assertEqual(by_key["assignee"], "Owner")
        self.assertEqual(by_key["fixVersion"], "Fix Version")
        self.assertEqual(by_key["updated"], "Changed within")
        self.assertEqual(by_key["field:title"], "Summary contains")

    def test_catalog_extras_skip_known_fields(self):
        kinds = js.filter_kinds([
            {"field": "customfield_100", "label": "Story Points"},
            {"field": "key"},
            {"field": "status"},
            {"field": "description", "label": "Desc"},
            {"field": "epic", "label": ""},
        ])
        self.assertIn({"key": "field:customfield_100", "title": "Story Points contains", "kind": "text"}, kinds)
        self.assertIn({"key": "field:epic", "title": "epic contains", "kind": "text"}, kinds)
        self.assertNotIn("field:key", [k["key"] for k in kinds])
        self.assertNotIn("field:status", [k["key"] for k in kinds])


class Criteria(unittest.TestCase):
    def test_text_only(self):
        self.assertEqual(js.criteria({"text": "  retry bug  "}), {"text": "retry bug"})
        self.assertEqual(js.criteria({"text": "   "}), {})

    def test_projects(self):
        self.assertEqual(js.criteria({"projects": ["A"], "projectsAll": False}), {"projects": ["A"]})
        self.assertEqual(js.criteria({"projects": ["A"], "projectsAll": True}), {})
        self.assertEqual(js.criteria({"projects": [], "projectsAll": False}), {})

    def test_rows_pickers_choices_texts(self):
        crit = js.criteria({"rows": [
            {"key": "status", "selected": ["Done", "In Review"]},
            {"key": "assignee", "selected": []},            # empty picker -> nothing
            {"key": "updated", "value": ""},                # choice: empty value stays
            {"key": "field:title", "text": "  retry  "},
            {"key": "field:epic", "text": "  "},            # blank -> nothing
            {"key": "labels", "text": "not-a-field"},       # text rows only count field:
        ]})
        self.assertEqual(crit, {"status": ["Done", "In Review"], "updated": "",
                                "fields": {"title": "retry"}})

    def test_choice_value_present_even_when_empty(self):
        self.assertEqual(js.criteria({"rows": [{"key": "created", "value": ""}]}), {"created": ""})

    def test_max_results_only_positive_ints(self):
        self.assertEqual(js.criteria({"maxResults": "50"}), {"maxResults": 50})
        self.assertEqual(js.criteria({"maxResults": "0"}), {})
        self.assertEqual(js.criteria({"maxResults": ""}), {})
        self.assertEqual(js.criteria({"maxResults": " 3 "}), {})  # Int() strictness
        self.assertEqual(js.criteria({"maxResults": "x"}), {})

    def test_full_example(self):
        crit = js.criteria({
            "text": "timeout",
            "projects": ["APP"],
            "projectsAll": False,
            "rows": [
                {"key": "assignee", "selected": ["currentUser()"]},
                {"key": "resolved", "value": "-2w"},
                {"key": "field:description", "text": "retry"},
            ],
            "maxResults": "25",
        })
        self.assertEqual(crit, {
            "text": "timeout",
            "projects": ["APP"],
            "assignee": ["currentUser()"],
            "resolved": "-2w",
            "fields": {"description": "retry"},
            "maxResults": 25,
        })


if __name__ == "__main__":
    unittest.main(verbosity=2)
