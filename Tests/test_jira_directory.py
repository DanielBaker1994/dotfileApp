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


if __name__ == "__main__":
    unittest.main(verbosity=2)
