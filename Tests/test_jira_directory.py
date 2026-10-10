#!/usr/bin/env python3
"""jira directory cache — directory.json parse + mtime keying.

    python3 Tests/test_jira_directory.py
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

import jira_data
import jira_directory as jdir


class Load(unittest.TestCase):
    def setUp(self):
        self.root = tempfile.mkdtemp(prefix="jira-dir-")
        self.addCleanup(shutil.rmtree, self.root, True)
        self.n = 0

    def path(self, data):
        self.n += 1
        path = os.path.join(self.root, "directory-%d.json" % self.n)
        with open(path, "w", encoding="utf-8") as fh:
            json.dump(data, fh)
        return path

    def test_missing_file_is_empty(self):
        d = jdir.load(os.path.join(self.root, "none.json"))
        self.assertEqual(d["fetchedAt"], "")
        self.assertEqual(d["projects"], [])
        self.assertEqual(d["users"], [])
        self.assertEqual(d["statusCategories"], {})

    def test_full_parse(self):
        d = jdir.load(self.path({
            "fetchedAt": "2026-01-01T00:00:00Z",
            "projects": [{"key": "APP", "name": "App"}],
            "users": [{"id": "u1", "name": "Ada", "username": "ada", "email": "",
                       "projects": ["APP"]}],
            "statuses": ["To Do", "Done"],
            "statusCategories": {"Done": "done", "Weird": 7},
            "issueTypes": ["Bug"],
            "priorities": ["High"],
            "fields": [{"id": "customfield_1", "name": "Points", "custom": True, "type": "number"}],
            "versions": [{"name": "2026.1", "project": "APP", "releaseDate": "", "released": False}],
            "boards": [{"id": "7", "name": "Board", "type": "scrum", "projects": ["APP"]}],
            "labels": [{"name": "api", "projects": ["APP"], "count": 3}],
        }))
        self.assertEqual(d["fetchedAt"], "2026-01-01T00:00:00Z")
        self.assertEqual(d["projects"], [{"key": "APP", "name": "App"}])
        self.assertEqual(d["users"][0]["name"], "Ada")
        self.assertEqual(d["statusCategories"], {"Done": "done"})  # non-string value dropped
        self.assertEqual(d["fields"][0], {"id": "customfield_1", "name": "Points",
                                          "custom": True, "type": "number"})
        self.assertEqual(d["versions"][0]["id"], "")
        self.assertEqual(d["boards"], [{"id": "7", "name": "Board", "type": "scrum", "projects": ["APP"]}])
        self.assertEqual(d["labels"], [{"name": "api", "projects": ["APP"], "count": 3}])

    def test_defaults_and_rejections(self):
        d = jdir.load(self.path({
            "projects": [{"key": "K"}, {"name": "no key"}],
            "users": [{"id": "u1"}, {"name": "no id"}, {"id": 7}],
            "labels": [{"name": "l", "count": "x"}],
            "statuses": ["a", 7],            # one bad element rejects the list
            "statusCategories": "nope",
        }))
        self.assertEqual(d["projects"], [{"key": "K", "name": ""}])
        self.assertEqual(d["users"], [{"id": "u1", "name": "u1", "username": "", "email": "",
                                       "projects": []}])
        self.assertEqual(d["labels"], [{"name": "l", "projects": [], "count": 0}])
        self.assertEqual(d["statuses"], [])
        self.assertEqual(d["statusCategories"], {})
        # a list with any non-dict element is rejected wholesale (the old
        # Swift cast did the same)
        self.assertEqual(jdir.load(self.path({"projects": [{"key": "K"}, "nope"]}))["projects"], [])

    def test_broken_json_is_empty(self):
        path = os.path.join(self.root, "broken.json")
        with open(path, "w") as fh:
            fh.write("{nope")
        self.assertEqual(jdir.load(path)["projects"], [])

    def test_parsed_once_per_mtime(self):
        path = self.path({"fetchedAt": "one"})
        first = jdir.load(path)
        st = os.stat(path)
        with open(path, "w", encoding="utf-8") as fh:
            json.dump({"fetchedAt": "two"}, fh)
        os.utime(path, ns=(st.st_atime_ns, st.st_mtime_ns))  # same mtime: cached
        self.assertIs(jdir.load(path), first)
        os.utime(path, ns=(st.st_atime_ns, st.st_mtime_ns + 1_000_000_000))
        self.assertEqual(jdir.load(path)["fetchedAt"], "two")


class Options(unittest.TestCase):
    def test_project_options_follow_the_scope_order(self):
        d = {"projects": [{"key": "A", "name": "Alpha"}, {"key": "B", "name": "Beta"}]}
        self.assertEqual(jdir.project_options(d, ["B", "A", "Z"]),
                         [{"id": "B", "title": "B", "detail": "Beta", "group": "", "unused": False},
                          {"id": "A", "title": "A", "detail": "Alpha", "group": "", "unused": False},
                          {"id": "Z", "title": "Z", "detail": "", "group": "", "unused": False}])

    def test_user_options_me_first_and_detail_composition(self):
        d = {"users": [
            {"id": "u1", "name": "Ada", "username": "ada", "email": "a@x", "projects": ["A", "B"]},
            {"id": "u2", "name": "Bob", "username": "Bob", "email": "", "projects": []},
        ]}
        opts = jdir.user_options(d)
        self.assertEqual(opts[0], {"id": "currentUser()", "title": "Me",
                                   "detail": "currentUser()", "group": "", "unused": False})
        self.assertEqual(opts[1]["detail"], "ada · a@x · A, B")
        self.assertEqual(opts[2]["detail"], "")  # username == name, no email, no projects
        self.assertEqual(jdir.user_options(d, me=False)[0]["id"], "u1")

    def test_value_options(self):
        self.assertEqual(jdir.value_options(["Bug", 7, "Task"]),
                         [{"id": "Bug", "title": "Bug", "detail": "", "group": "", "unused": False},
                          {"id": "Task", "title": "Task", "detail": "", "group": "", "unused": False}])

    def test_status_options_group_by_category(self):
        d = {"statuses": ["To Do", "Doing", "Done"],
             "statusCategories": {"To Do": "new", "Doing": "indeterminate", "Done": "done"}}
        opts = jdir.status_options(d, {})
        self.assertEqual([o["group"] for o in opts], ["To Do", "In Progress", "Done"])

    def test_status_options_word_fallback_and_custom_names(self):
        d = {"statuses": ["Won't Do"], "statusCategories": {}}
        opts = jdir.status_options(d, jira_data.words({}), ["N", "P", "D"])
        self.assertEqual(opts[0]["group"], "D")

    def test_label_options_sort_scope_and_detail(self):
        d = {"labels": [
            {"name": "L10", "projects": ["A"], "count": 2},
            {"name": "L2", "projects": ["A"], "count": 2},
            {"name": "zzz", "projects": ["B"], "count": 9},
            {"name": "one", "projects": ["A"], "count": 1},
            {"name": "none", "projects": ["A"], "count": 0},
        ]}
        opts = jdir.label_options(d, ["A"])
        self.assertEqual([o["id"] for o in opts], ["L2", "L10", "one", "none"])
        self.assertEqual(opts[0]["detail"], "2 issues · A")
        self.assertEqual(opts[2]["detail"], "1 issue · A")
        self.assertEqual(opts[3]["detail"], "A")
        self.assertEqual([o["id"] for o in jdir.label_options(d, None)], ["zzz", "L2", "L10", "one", "none"])

    def test_label_options_fold_tail_marks_unused(self):
        d = {"labels": [{"name": "n%03d" % i, "projects": [], "count": 0} for i in range(60)]}
        opts = jdir.label_options(d)
        self.assertEqual(len(opts), 60)
        self.assertFalse(opts[49]["unused"])
        self.assertTrue(opts[50]["unused"])
        self.assertTrue(opts[59]["unused"])
        small = jdir.label_options({"labels": [{"name": "a", "projects": [], "count": 0}] * 40})
        self.assertFalse(any(o["unused"] for o in small))

    def test_version_options_group_dates_and_state(self):
        d = {"versions": [
            {"name": "2026.2", "project": "A", "releaseDate": "", "released": False},
            {"name": "2026.2", "project": "B", "releaseDate": "2026-06-01", "released": False},
            {"name": "2026.1", "project": "A", "releaseDate": "2026-01-01", "released": True},
            {"name": "2027.1", "project": "C", "releaseDate": "2027-01-01", "released": False},
        ]}
        opts = jdir.version_options(d, ["A", "B"])
        self.assertEqual([o["id"] for o in opts], ["2026.2", "2026.1"])
        self.assertEqual(opts[0]["detail"], "A, B · 2026-06-01 · unreleased")
        self.assertEqual(opts[1]["detail"], "A · 2026-01-01 · released")
        self.assertEqual(jdir.version_options(d, None)[2]["detail"], "C · 2027-01-01 · unreleased")

    def test_version_options_no_date_fallback(self):
        d = {"versions": [{"name": "v", "project": "A", "releaseDate": "", "released": True}]}
        self.assertEqual(jdir.version_options(d, None)[0]["detail"], "A · no date · released")


if __name__ == "__main__":
    unittest.main(verbosity=2)
