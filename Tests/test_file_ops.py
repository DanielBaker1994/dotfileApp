#!/usr/bin/env python3
"""file_ops — file mutations + undo, ported from FileOps.swift.

    python3 Tests/test_file_ops.py
"""
from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)
sys.path.insert(0, os.path.join(ROOT, "pylib"))

import file_ops as fo


class FileOpsBase(unittest.TestCase):
    def setUp(self):
        self.root = tempfile.mkdtemp(prefix="fileops-test-")
        self.trashed = []
        self.addCleanup(self._cleanup)

    def _cleanup(self):
        for p in self.trashed:
            try:
                shutil.rmtree(p, ignore_errors=True)
                os.remove(p)
            except OSError:
                pass
        shutil.rmtree(self.root, ignore_errors=True)

    def write(self, rel, text="x"):
        path = os.path.join(self.root, rel)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w") as f:
            f.write(text)

    def exists(self, rel):
        return os.path.exists(os.path.join(self.root, rel))

    def read(self, rel):
        try:
            with open(os.path.join(self.root, rel)) as f:
                return f.read()
        except OSError:
            return None

    def undo(self, stack):
        u = fo.undo(stack)
        if u:
            self.trashed += [c[1] for c in u["outcome"]["changes"] if c[0] is not None and c[1] is not None]
        return u


class Names(FileOpsBase):
    def test_free_and_copy_names(self):
        self.write("n/a.txt")
        self.write("n/a 2.txt")
        self.write("n/noext")
        self.write("n/.env")
        d = os.path.join(self.root, "n")
        self.assertEqual(os.path.basename(fo.free_url("b.txt", d)), "b.txt")
        self.assertEqual(os.path.basename(fo.free_url("a.txt", d)), "a 3.txt")
        self.assertEqual(os.path.basename(fo.free_url("noext", d)), "noext 2")
        self.assertEqual(os.path.basename(fo.copy_url(os.path.join(d, "a.txt"))), "a copy.txt")
        self.write("n/a copy.txt")
        self.assertEqual(os.path.basename(fo.copy_url(os.path.join(d, "a.txt"))), "a copy 2.txt")
        self.assertEqual(os.path.basename(fo.copy_url(os.path.join(d, "noext"))), "noext copy")
        self.assertEqual(os.path.basename(fo.copy_url(os.path.join(d, ".env"))), ".env copy")


class CopyAndUndo(FileOpsBase):
    def test_copy_then_undo(self):
        stack = fo.stack_new()
        self.write("c/src/one.txt", "1")
        self.write("c/src/two.txt", "2")
        self.write("c/dst/one.txt", "old")
        out = fo.transfer([self.root + "/c/src/one.txt", self.root + "/c/src/two.txt"],
                          self.root + "/c/dst", False, stack)
        self.assertIsNone(out["failed"])
        self.assertEqual(len(out["changes"]), 2)
        self.assertTrue(all(c[0] is None for c in out["changes"]))
        self.assertEqual(self.read("c/dst/one.txt"), "old")
        self.assertEqual(self.read("c/dst/one 2.txt"), "1")
        self.assertTrue(self.exists("c/src/one.txt") and self.exists("c/dst/two.txt"))
        u = self.undo(stack)
        self.assertIsNotNone(u)
        self.assertFalse(self.exists("c/dst/one 2.txt"))
        self.assertFalse(self.exists("c/dst/two.txt"))
        self.assertEqual(self.read("c/dst/one.txt"), "old")
        self.assertIsNone(fo.undo(stack))


class MoveAndUndo(FileOpsBase):
    def test_move_then_undo(self):
        stack = fo.stack_new()
        self.write("m/src/f.txt", "f")
        self.write("m/src/dir/in.txt", "in")
        os.makedirs(self.root + "/m/dst", exist_ok=True)
        out = fo.transfer([self.root + "/m/src/f.txt", self.root + "/m/src/dir"],
                          self.root + "/m/dst", True, stack)
        self.assertEqual(len(out["changes"]), 2)
        self.assertEqual(out["changes"][0][0], self.root + "/m/src/f.txt")
        self.assertFalse(self.exists("m/src/f.txt"))
        self.assertTrue(self.exists("m/dst/f.txt") and self.exists("m/dst/dir/in.txt"))
        same = fo.transfer([self.root + "/m/dst/f.txt"], self.root + "/m/dst", True, stack)
        self.assertEqual(same["changes"], [])
        self.assertIsNone(same["failed"])
        inside = fo.transfer([self.root + "/m/dst/dir"], self.root + "/m/dst/dir", True, stack)
        self.assertEqual(inside["changes"], [])
        self.assertIsNotNone(inside["failed"])
        u = self.undo(stack)
        self.assertEqual(len(u["outcome"]["changes"]), 2)
        self.assertTrue(self.exists("m/src/f.txt") and self.exists("m/src/dir/in.txt"))
        self.assertFalse(self.exists("m/dst/f.txt"))


class UndoBlocked(FileOpsBase):
    def test_undo_never_overwrites(self):
        stack = fo.stack_new()
        self.write("b/src/f.txt", "new")
        os.makedirs(self.root + "/b/dst", exist_ok=True)
        fo.transfer([self.root + "/b/src/f.txt"], self.root + "/b/dst", True, stack)
        self.write("b/src/f.txt", "squatter")
        u = self.undo(stack)
        self.assertEqual(u["outcome"]["changes"], [])
        self.assertIsNotNone(u["outcome"]["failed"])
        self.assertEqual(self.read("b/src/f.txt"), "squatter")
        self.assertEqual(self.read("b/dst/f.txt"), "new")


class TrashAndUndo(FileOpsBase):
    def test_trash_then_undo(self):
        stack = fo.stack_new()
        self.write("t/gone.txt", "g")
        self.write("t/dir/in.txt", "in")
        self.write("t/stay.txt")
        out = fo.trash([self.root + "/t/gone.txt", self.root + "/t/dir",
                        self.root + "/t/missing.txt"], stack)
        self.assertEqual(len(out["changes"]), 2)
        self.assertIsNotNone(out["failed"])
        self.assertFalse(self.exists("t/gone.txt"))
        self.assertFalse(self.exists("t/dir"))
        self.assertTrue(self.exists("t/stay.txt"))
        self.assertTrue(all(os.path.exists(c[1]) and c[1] != c[0] for c in out["changes"]))
        u = self.undo(stack)
        self.assertEqual(u["what"], "trash of 2 items")
        self.assertEqual(self.read("t/gone.txt"), "g")
        self.assertEqual(self.read("t/dir/in.txt"), "in")
        self.assertTrue(all(not os.path.exists(c[1]) for c in out["changes"]))


class CreateDuplicateRename(FileOpsBase):
    def test_create_duplicate_rename(self):
        stack = fo.stack_new()
        os.makedirs(self.root + "/n2", exist_ok=True)
        a = fo.create("untitled folder", self.root + "/n2", True, stack)
        b = fo.create("untitled folder", self.root + "/n2", True, stack)
        self.assertTrue(os.path.isdir(a["changes"][0][1]))
        self.assertEqual(b["changes"][0][1], self.root + "/n2/untitled folder 2")
        f = fo.create("untitled.txt", self.root + "/n2", False, stack)
        self.assertEqual(self.read("n2/untitled.txt"), "")
        self.assertIsNone(f["failed"])
        none = fo.create("x", self.root + "/no-such-dir", True, stack)
        self.assertEqual(none["changes"], [])
        self.assertIsNotNone(none["failed"])

        self.write("n2/doc.md", "d")
        d = fo.duplicate([self.root + "/n2/doc.md"], stack)
        self.assertEqual(self.read("n2/doc copy.md"), "d")
        self.assertIsNone(d["changes"][0][0])

        os.rename(self.root + "/n2/doc.md", self.root + "/n2/Doc.md")
        fo.record_rename(self.root + "/n2/doc.md", self.root + "/n2/Doc.md", stack)
        u = self.undo(stack)
        names = os.listdir(self.root + "/n2")
        self.assertEqual(u["what"], "rename of doc.md")
        self.assertIn("doc.md", names)
        self.assertNotIn("Doc.md", names)
        for _ in range(4):
            self.undo(stack)
        self.assertFalse(self.exists("n2/doc copy.md"))
        self.assertFalse(self.exists("n2/untitled folder"))
        self.assertFalse(self.exists("n2/untitled.txt"))
        self.assertFalse(fo.stack_state(stack)["canUndo"])


class Protocol(unittest.TestCase):
    def serve(self, requests):
        env = dict(os.environ)
        env["PYTHONPATH"] = os.path.join(ROOT, "pylib")
        env["PYTHONDONTWRITEBYTECODE"] = "1"
        p = subprocess.run(
            [sys.executable, "-B", "-m", "helper"],
            input="".join(json.dumps(r) + "\n" for r in requests),
            capture_output=True, text=True, cwd=ROOT, env=env,
        )
        return [json.loads(line) for line in p.stdout.splitlines()]

    def test_round_trip(self):
        tmp = tempfile.mkdtemp(prefix="fileops-proto-")
        self.addCleanup(shutil.rmtree, tmp, True)
        replies = self.serve([
            {"id": 1, "method": "fileops.stack_new", "params": {"limit": 5}},
            {"id": 2, "method": "fileops.create",
             "params": {"name": "a.txt", "dir": tmp, "folder": False, "stack": 1}},
            {"id": 3, "method": "fileops.stack_state", "params": {"handle": 1}},
            {"id": 4, "method": "fileops.undo", "params": {"stack": 1}},
            {"id": 5, "method": "fileops.stack_drop", "params": {"handle": 1}},
        ])
        self.assertEqual(len(replies), 5)
        path = replies[1]["result"]["changes"][0][1]
        self.assertTrue(path.startswith(tmp))
        self.assertEqual(replies[2]["result"]["count"], 1)
        self.assertIn("outcome", replies[3]["result"])
        self.assertFalse(os.path.exists(path))
        self.assertTrue(replies[4]["ok"])


if __name__ == "__main__":
    unittest.main(verbosity=2)
