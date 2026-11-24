#!/usr/bin/env python3
"""jira boards — the column/card mapping.

    python3 Tests/test_jira_boards.py
"""
from __future__ import annotations

import os
import sys
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(ROOT, "pylib"))

import jira_boards as jb
import jira_data

WORDS = jira_data.words({})


def row(key, status="Open", title="", **over):
    r = {"key": key, "status": status, "title": title or ("Title " + key),
         "type": "Bug", "priority": "High", "assignee": "", "rowTitle": ""}
    r.update(over)
    return r


class Columns(unittest.TestCase):
    def params(self, rows, **over):
        p = {"rows": rows, "columns": [], "categories": {}, "words": WORDS,
             "people": {}, "categoryNames": ["To Do", "In Progress", "Done"]}
        p.update(over)
        return p

    def test_category_columns_without_a_spec(self):
        out = jb.board_columns(self.params([
            row("A-1", "Backlog"), row("A-2", "In Review"), row("A-3", "Done"),
        ]))["columns"]
        self.assertEqual([c["name"] for c in out], ["To Do", "In Progress", "Done"])
        self.assertEqual([[k["key"] for k in c["cards"]] for c in out], [["A-1"], ["A-2"], ["A-3"]])
        self.assertEqual([c["more"] for c in out], [0, 0, 0])

    def test_directory_categories_win(self):
        out = jb.board_columns(self.params(
            [row("A-1", "Reviewing")], categories={"Reviewing": "indeterminate"}))["columns"]
        self.assertEqual([k["key"] for k in out[1]["cards"]], ["A-1"])

    def test_board_spec_maps_statuses_and_collects_the_rest(self):
        out = jb.board_columns(self.params(
            [row("A-1", "To Do"), row("A-2", "Doing"), row("A-3", "Odd")],
            columns=[{"name": "Ready", "statuses": ["To Do"]},
                     {"name": "Doing", "statuses": ["Doing"]}]))["columns"]
        self.assertEqual([c["name"] for c in out], ["Ready", "Doing", jb.OTHER_COLUMN])
        self.assertEqual([k["key"] for k in out[2]["cards"]], ["A-3"])

    def test_done_column_clamps_only_when_fully_done(self):
        rows = [row("D-%02d" % i, "Done") for i in range(31)]
        out = jb.board_columns(self.params(rows))["columns"]
        self.assertEqual(len(out[2]["cards"]), jb.DONE_LIMIT)
        self.assertEqual(out[2]["more"], 1)
        mixed = [row("D-1", "Done"), row("D-2", "Backlog")]
        out = jb.board_columns(self.params(mixed))["columns"]
        self.assertEqual(len(out[0]["cards"]), 1)  # Backlog column keeps its one card

    def test_people_mapping_and_title_fallback(self):
        r = row("A-1", "Done", assignee="ada")
        r["title"] = ""
        r["rowTitle"] = "Row Fallback"
        out = jb.board_columns(self.params([r], people={"ada": "Ada Lovelace"}))["columns"]
        card = out[2]["cards"][0]
        self.assertEqual(card["assignee"], "Ada Lovelace")
        self.assertEqual(card["title"], "Row Fallback")
        out = jb.board_columns(self.params([row("A-2", "Done", assignee="ghost")]))["columns"]
        self.assertEqual(out[2]["cards"][0]["assignee"], "ghost")

    def test_empty_rows_still_yield_the_columns(self):
        out = jb.board_columns(self.params([]))["columns"]
        self.assertEqual([c["name"] for c in out], ["To Do", "In Progress", "Done"])
        self.assertTrue(all(not c["cards"] for c in out))


if __name__ == "__main__":
    unittest.main(verbosity=2)
