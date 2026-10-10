#!/usr/bin/env python3
"""ignore_rules — the gitignore matcher (was the IgnoreRules part of PathShelf.swift).

    python3 Tests/test_ignore_rules.py
"""
from __future__ import annotations

import os
import shutil
import subprocess
import sys
import tempfile
import time
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)
sys.path.insert(0, os.path.join(ROOT, "pylib"))

import ignore_rules as ir


def m(pat, rel, dir=False, global_=False, home=None):
    p = ir.compile_pattern(pat, global_, home)
    if not p:
        return False
    if p["dirOnly"] and not dir:
        return False
    return p["regex"].search(rel) is not None


class Patterns(unittest.TestCase):
    def test_globs(self):
        self.assertTrue(m("*.pyc", "a/b/x.pyc"))
        self.assertFalse(m("*.pyc", "a/b/x.pyc.txt"))
        self.assertTrue(m("/top.txt", "top.txt") and not m("/top.txt", "a/top.txt"))
        self.assertTrue(m("a/b.txt", "a/b.txt") and not m("a/b.txt", "x/a/b.txt"))
        self.assertTrue(m("build/", "x/build", dir=True) and not m("build/", "x/build"))
        self.assertTrue(m("**/cache", "a/b/cache") and m("**/cache", "cache"))
        self.assertTrue(m("docs/**", "docs/a/b.md") and not m("docs/**", "docs"))
        self.assertTrue(m("a/**/z", "a/z") and m("a/**/z", "a/b/c/z"))
        self.assertTrue(m("[Tt]humbs.db", "Thumbs.db") and m("[Tt]humbs.db", "x/thumbs.db"))
        self.assertTrue(m("[!a]x", "bx") and not m("[!a]x", "ax"))
        self.assertTrue(m("?.txt", "a.txt") and not m("?.txt", "ab.txt") and not m("?.txt", "/.txt"))
        self.assertTrue(m("\\#hash", "#hash"))

    def test_specials(self):
        self.assertIsNone(ir.compile_pattern("# comment"))
        self.assertIsNone(ir.compile_pattern("   "))
        self.assertTrue(ir.compile_pattern("!keep.log")["negate"])
        self.assertTrue(m("trail\\ ", "trail "))
        self.assertTrue(m("x  ", "x"))

    def test_tilde_path(self):
        home = "/Users/someone"
        p = ir.compile_pattern("~/Secret/", True, home)
        self.assertIsNotNone(p)
        self.assertTrue(p["dirOnly"])
        self.assertTrue(p["regex"].search("Users/someone/Secret"))


class GitParity(unittest.TestCase):
    def setUp(self):
        self.root = tempfile.mkdtemp(prefix="ignore-test-")
        self.addCleanup(shutil.rmtree, self.root, True)

    def git(self, cwd, args, stdin=""):
        env = dict(os.environ, GIT_CONFIG_GLOBAL="/dev/null", GIT_CONFIG_NOSYSTEM="1")
        r = subprocess.run(["/usr/bin/git"] + args, cwd=cwd, input=stdin,
                           capture_output=True, text=True, env=env)
        return r.returncode, r.stdout

    def write(self, path, text):
        try:
            os.makedirs(os.path.dirname(path), exist_ok=True)
        except OSError:
            pass
        try:
            with open(path, "w") as f:
                f.write(text)
        except OSError:
            pass

    def test_check_ignore_parity(self):
        repo = self.root + "/repo"
        code, _ = self.git(self.root, ["init", "-q", repo])
        if code != 0:
            self.skipTest("git not available")
        self.write(repo + "/.gitignore", """# comments and blanks are skipped

*.log
!keep.log
build/
/top.txt
docs/**/*.tmp
**/cache/
a/b/c.txt
[Tt]humbs.db
\\#hash.txt
deep/**
!deep/keep.txt
name-only
""")
        self.write(repo + "/sub/.gitignore", "*.md\n!README.md\n/local.txt\n")
        rels = [
            "x.log", "keep.log", "sub/y.log", "sub/keep.log", "build/out.o", "src/build/out.o", "build.txt",
            "top.txt", "sub/top.txt", "docs/a/b/c.tmp", "docs/c.tmp", "other/c.tmp", "cache/x", "q/cache/x",
            "a/b/c.txt", "x/a/b/c.txt", "Thumbs.db", "z/thumbs.db", "#hash.txt", "deep/x/y", "deep/keep.txt",
            "name-only", "z/name-only", "name-only/inside.txt", "sub/notes.md", "sub/README.md", "sub/local.txt",
            "sub/deeper/local.txt", "plain.txt", "sub/plain.md.txt",
        ]
        for r in rels:
            self.write(repo + "/" + r, "")
        _, out = self.git(repo, ["check-ignore", "--stdin"], "\n".join(rels) + "\n")
        git_ignored = set(out.splitlines())
        rules = ir.Rules(home=self.root, shelf_file=None, git_excludes=self.root + "/no-such-global")
        rules.set_recheck(0)
        for r in rels:
            mine = rules.ignored(repo + "/" + r)
            theirs = r in git_ignored
            self.assertEqual(mine, theirs, "git parity: %s" % r)
        self.write(self.root + "/norepo/.gitignore", "*.txt\n")
        self.write(self.root + "/norepo/a.txt", "")
        self.assertFalse(rules.ignored(self.root + "/norepo/a.txt"),
                         ".gitignore outside a git repo is not honored")

    def test_ignore_files_and_shelf(self):
        d = self.root + "/rg"
        self.write(d + "/.ignore", "*.secret\nx.txt\n")
        self.write(d + "/.rgignore", "!x.txt\n")
        self.write(d + "/inner/.ignore", "!inner.secret\n")
        for f in ["a.secret", "x.txt", "inner/inner.secret", "inner/other.secret", "fine.md"]:
            self.write(d + "/" + f, "")
        shelf = self.root + "/paths.ignore"
        self.write(shelf, "*.md\n!special.md\n~/Private/\n")
        rules = ir.Rules(home=self.root, shelf_file=shelf, git_excludes=self.root + "/no-such-global")
        rules.set_recheck(0)
        self.assertTrue(rules.ignored(d + "/a.secret"), ".ignore applies outside a repo")
        self.assertFalse(rules.ignored(d + "/x.txt"), ".rgignore beats .ignore in the same folder")
        self.assertFalse(rules.ignored(d + "/inner/inner.secret"), "a deeper folder's ! re-includes")
        self.assertTrue(rules.ignored(d + "/inner/other.secret"), "the parent folder's rule still applies below")
        self.assertTrue(rules.ignored(d + "/fine.md"), "the shelf file applies everywhere")
        self.write(d + "/special.md", "")
        self.assertFalse(rules.ignored(d + "/special.md"), "the shelf file's ! re-includes")
        self.write(self.root + "/Private/doc.pdf", "")
        self.assertTrue(rules.ignored(self.root + "/Private/doc.pdf"), "~/ folder in the shelf file")
        global_file = self.root + "/global-ignore"
        self.write(global_file, "*.bak\n")
        rules.set_git_excludes(global_file)
        self.write(d + "/old.bak", "")
        self.assertTrue(rules.ignored(d + "/old.bak"), "global git excludes apply")
        self.write(shelf, "*.md\n")
        time.sleep(0.02)
        self.write(shelf, "*.md\n# edited\n")
        self.assertTrue(rules.ignored(d + "/special.md"), "ignore-file edits apply without a restart")
        self.write(self.root + "/.gitconfig", "[user]\n  name = x\n[core]\n  excludesFile = ~/my-ignore\n")
        self.assertEqual(ir.git_excludes_file(self.root), self.root + "/my-ignore",
                         "core.excludesFile from ~/.gitconfig (~ expanded)")


if __name__ == "__main__":
    unittest.main(verbosity=2)
