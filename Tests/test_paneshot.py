#!/usr/bin/env python3
"""pane-shot — args, config clamping and the herdr JSON envelope (the
Swift suite's cases; process handling stays Swift).

    python3 Tests/test_paneshot.py
"""
from __future__ import annotations

import os
import sys
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(ROOT, "pylib"))

import paneshot as ps


def p(s):
    try:
        return ps.parse_args(s.split(" ") if s else [])
    except ps.Problem:
        return None


class Args(unittest.TestCase):
    def test_no_args(self):
        self.assertEqual(p(""), ps.Args(), "no args")

    def test_pane_and_lines(self):
        a = p("--pane wB:p1 --lines 50")
        self.assertEqual(a.pane, "wB:p1")
        self.assertEqual(p("--lines 50").lines, 50, "pane + lines")
        self.assertTrue(p("-n all").all, "lines all")

    def test_file_and_switches(self):
        a = p("--file - --no-save --no-copy")
        self.assertEqual((a.file, a.save, a.copy), ("-", False, False), "file + switches")

    def test_bad_args_refused(self):
        for s in ("--lines x", "--lines", "--bogus", "--lines -3"):
            self.assertIsNone(p(s), s)


class Config(unittest.TestCase):
    def test_defaults(self):
        d = ps.Config({})
        self.assertEqual(d.lines, 200)
        self.assertTrue(d.save and d.copy)
        self.assertEqual(d.font, "")
        self.assertEqual(d.font_size, 0)

    def test_values_and_clamps(self):
        c = ps.Config({"lines": "5000", "save": "false", "font-size": "14",
                       "padding": "-3", "herdr-bin": "/x/herdr"})
        self.assertEqual(c.lines, ps.MAX_LINES, "lines clamped to herdr's cap")
        self.assertFalse(c.save)
        self.assertEqual(c.font_size, 14)
        self.assertEqual(c.padding, 0)
        self.assertEqual(c.herdr_bin, "/x/herdr", "values")


class Herdr(unittest.TestCase):
    OK = ('{"id":"cli:pane:current","result":{"pane":{"pane_id":"wB:p1J",'
          '"scroll":{"viewport_rows":60},"terminal_title_stripped":"build things",'
          '"agent":"claude"},"type":"pane_current"}}')

    def test_pane_parsed(self):
        self.assertEqual(ps.pane_from_json(self.OK),
                         {"id": "wB:p1J", "title": "build things", "viewportRows": 60})

    def test_no_title_falls_back_to_id(self):
        bare = '{"result":{"pane":{"pane_id":"wB:p2","scroll":{"viewport_rows":26}}}}'
        self.assertEqual(ps.pane_from_json(bare)["title"], "wB:p2")

    def test_error_envelope(self):
        err = '{"id":"x","error":{"code":"server_not_running","message":"no herdr server is running"}}'
        with self.assertRaises(ps.HerdrFailure) as cm:
            ps.pane_from_json(err)
        self.assertEqual(cm.exception.message, "no herdr server is running")

    def test_lines(self):
        self.assertEqual(ps.lines_for(60, 200, False), 260, "screen + history")
        self.assertEqual(ps.lines_for(60, 5000, False), ps.MAX_LINES, "capped")
        self.assertEqual(ps.lines_for(60, 0, True), ps.MAX_LINES, "all")

    def test_environment_drops_pane_vars(self):
        env = ps.environment({"HERDR_PANE_ID": "a", "HERDR_TAB_ID": "b",
                              "HERDR_SOCKET_PATH": "/s", "HOME": "/h"})
        self.assertEqual(env, {"HERDR_SOCKET_PATH": "/s", "HOME": "/h"},
                         "pane env dropped, socket kept")

    def test_non_json_and_missing_pane(self):
        with self.assertRaises(ps.HerdrFailure):
            ps.pane_from_json("not json")
        with self.assertRaises(ps.HerdrFailure):
            ps.pane_from_json('{"result":{}}')


if __name__ == "__main__":
    unittest.main(verbosity=2)
