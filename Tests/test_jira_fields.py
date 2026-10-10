#!/usr/bin/env python3
"""jira fields — the column-spec codec and the built-in label table.

    python3 Tests/test_jira_fields.py
"""
from __future__ import annotations

import os
import sys
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(ROOT, "pylib"))

import jira_fields as jf


class Parse(unittest.TestCase):
    def test_defaults(self):
        self.assertEqual(jf.parse_columns("key"),
                         [{"field": "key", "title": "key", "width": 0.0, "align": "left",
                           "sortable": False, "filterable": False}])

    def test_full_segments(self):
        cols = jf.parse_columns(" status : Status : 120 : RIGHT : filter+sort , title")
        self.assertEqual(cols[0], {"field": "status", "title": "Status", "width": 120.0,
                                   "align": "right", "sortable": True, "filterable": True})
        self.assertEqual(cols[1]["field"], "title")

    def test_width_fraction_garbage_and_negative_clamp(self):
        self.assertEqual(jf.parse_columns("a::12.5")[0]["width"], 12.5)
        self.assertEqual(jf.parse_columns("a::x")[0]["width"], 0.0)
        self.assertEqual(jf.parse_columns("a:: -3 ")[0]["width"], 0.0)

    def test_align_falls_back_to_left(self):
        self.assertEqual(jf.parse_columns("a:::middle")[0]["align"], "left")
        self.assertEqual(jf.parse_columns("a")[0]["align"], "left")

    def test_flag_separators(self):
        self.assertTrue(jf.parse_columns("a::::filter|sort")[0]["sortable"])
        self.assertTrue(jf.parse_columns("a::::sort/filter")[0]["filterable"])
        self.assertTrue(jf.parse_columns("a::::SORT")[0]["sortable"])

    def test_empty_and_whitespace(self):
        self.assertEqual(jf.parse_columns(""), [])
        self.assertEqual(jf.parse_columns("  , , "), [])
        self.assertEqual(jf.parse_columns(":title"), [])


class Serialize(unittest.TestCase):
    def col(self, **over):
        c = {"field": "a", "title": "T", "width": 120.0, "align": "left",
             "sortable": False, "filterable": False}
        c.update(over)
        return c

    def test_integral_and_fractional_widths(self):
        self.assertEqual(jf.serialize_columns([self.col()]), "a:T:120:left")
        self.assertEqual(jf.serialize_columns([self.col(width=120.5)]), "a:T:120.5:left")
        self.assertEqual(jf.serialize_columns([self.col(width=0)]), "a:T:0:left")

    def test_titles_off(self):
        self.assertEqual(jf.serialize_columns([self.col()], titles=False), "a::120:left")

    def test_flags_filter_first(self):
        self.assertEqual(jf.serialize_columns([self.col(sortable=True, filterable=True)]),
                         "a:T:120:left:filter+sort")
        self.assertEqual(jf.serialize_columns([self.col(sortable=True)]), "a:T:120:left:sort")

    def test_round_trip(self):
        spec = "key:Key:80:left:filter, status:Status:120:right:filter+sort, epic"
        self.assertEqual(jf.parse_columns(jf.serialize_columns(jf.parse_columns(spec))),
                         jf.parse_columns(spec))


class Labels(unittest.TestCase):
    def test_spot_checks(self):
        self.assertEqual(jf.BASE_FIELD_LABELS["key"], "Key")
        self.assertEqual(jf.BASE_FIELD_LABELS["epic"], "Epic / parent")
        self.assertEqual(jf.BASE_FIELD_LABELS["release"], "Fix versions")


if __name__ == "__main__":
    unittest.main(verbosity=2)
