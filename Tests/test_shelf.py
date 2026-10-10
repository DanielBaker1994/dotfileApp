#!/usr/bin/env python3
"""path shelf — canonical/normalize, the store list rules, JSON format and
rename re-keying (the Swift suite keeps its end-to-end integration cases).

    python3 Tests/test_shelf.py
"""
from __future__ import annotations

import json
import os
import shutil
import socket
import sys
import tempfile
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(ROOT, "pylib"))

import shelf


class Canonical(unittest.TestCase):
    def setUp(self):
        self.root = os.path.realpath(tempfile.mkdtemp(prefix="shelf-test-"))
        self.addCleanup(shutil.rmtree, self.root, True)

    def test_missing_is_none(self):
        self.assertIsNone(shelf.canonical(self.root + "/nope.txt"))

    def test_file_and_dir(self):
        f = self.root + "/a.txt"
        open(f, "w").write("x")
        self.assertEqual(shelf.canonical(f), {"path": f, "isFile": True, "isDir": False})
        self.assertEqual(shelf.canonical(self.root),
                         {"path": self.root, "isFile": False, "isDir": True})

    def test_symlinks_resolve(self):
        d = self.root + "/real"
        os.makedirs(d)
        link = self.root + "/link"
        os.symlink(d, link)
        got = shelf.canonical(link + "/x.txt")
        self.assertIsNone(got)
        open(d + "/x.txt", "w").write("x")
        self.assertEqual(shelf.canonical(link + "/x.txt")["path"], d + "/x.txt")

    def test_socket_is_not_a_file(self):
        s = self.root + "/s.sock"
        srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.addCleanup(srv.close)
        self.addCleanup(lambda: os.path.exists(s) and os.remove(s))
        srv.bind(s)
        got = shelf.canonical(s)
        self.assertFalse(got["isFile"] or got["isDir"])


class Normalize(unittest.TestCase):
    def test_tmp_prefix(self):
        self.assertEqual(shelf.normalize("/tmp/a/../b"), "/private/tmp/b",
                         "/tmp normalized to /private/tmp")
        self.assertEqual(shelf.normalize("/tmp"), "/private/tmp")

    def test_file_url(self):
        self.assertEqual(shelf.normalize("file:///Users/x/a%20b.txt"), "/Users/x/a b.txt")

    def test_plain_and_tilde(self):
        self.assertEqual(shelf.normalize("/a/b/../c"), "/a/c")
        self.assertEqual(shelf.normalize("~/x"), os.path.expanduser("~") + "/x")


class Store(unittest.TestCase):
    def items(self):
        return [{"path": "/f%d" % i, "at": float(i), "why": "clipboard"} for i in range(30)]

    def test_bump_moves_and_caps(self):
        items = shelf.bump(self.items(), "/f10", "filefast", 99.0, 25)
        self.assertEqual(len(items), 25)
        self.assertEqual(items[0], {"path": "/f10", "at": 99.0, "why": "filefast"})
        items = shelf.bump(items, "/f5", "modified", 100.0, 25)
        self.assertEqual(items[0]["why"], "clipboard", "an edit keeps the last why")
        items = shelf.bump(items, "/new", "screenshot", 101.0, 25)
        self.assertEqual(items[0]["path"], "/new")
        self.assertEqual(len(items), 25)

    def test_finalize_sorts_dedups_caps(self):
        raw = [{"path": "/a", "at": 1.0, "why": "clipboard"},
               {"path": "/b", "at": 3.0, "why": "clipboard"},
               {"path": "/a", "at": 2.0, "why": "clipboard"}]
        self.assertEqual([i["path"] for i in shelf.finalize(raw, 25)], ["/b", "/a"])
        self.assertEqual(len(shelf.finalize(raw, 1)), 1)

    def test_load_candidates_activity_rules(self):
        import tempfile as tf
        root = tf.mkdtemp(prefix="shelf-load-")
        self.addCleanup(shutil.rmtree, root, True)
        f = root + "/f.txt"
        open(f, "w").write("x")
        raw = [
            {"path": f, "at": 1.0, "why": "created"},          # file + activity → kept
            {"path": root, "at": 2.0, "why": "modified"},      # dir + activity → dropped
            {"path": root, "at": 3.0, "why": "copied"},        # dir + copied → kept
            {"path": root + "/gone", "at": 4.0, "why": "created"},  # missing → dropped
            {"path": f, "at": 5.0, "why": "bogus"},            # unknown why → modified
            {"path": 7, "at": 6.0, "why": "created"},          # bad type → skipped
        ]
        out = shelf.load_candidates(raw)
        self.assertEqual([(os.path.basename(i["path"]), i["why"]) for i in out],
                         [(os.path.basename(f), "created"),
                          (os.path.basename(root), "copied"),
                          (os.path.basename(f), "modified")])

    def test_save_and_reload_round_trip(self):
        root = tempfile.mkdtemp(prefix="shelf-save-")
        self.addCleanup(shutil.rmtree, root, True)
        path = root + "/paths.json"
        f = root + "/f.txt"
        open(f, "w").write("x")
        items = [{"path": f, "at": 2.0, "why": "clipboard"}]
        shelf.save(path, items)
        raw = json.load(open(path))
        got = shelf.finalize(shelf.load_candidates(raw), 25)
        self.assertEqual(got, [{"path": shelf.canonical(f)["path"], "at": 2.0, "why": "clipboard"}])


class Rekey(unittest.TestCase):
    def test_cases(self):
        self.assertEqual(shelf.rekey("/a.txt", "/a.txt", "/b.txt"), "/b.txt")
        self.assertEqual(shelf.rekey("/dir/x.txt", "/dir", "/new"), "/new/x.txt")
        self.assertIsNone(shelf.rekey("/other/x.txt", "/dir", "/new"))
        self.assertIsNone(shelf.rekey("/dirsuffix", "/dir", "/new"))


if __name__ == "__main__":
    unittest.main(verbosity=2)
