#!/usr/bin/env python3
"""config_text — the one commands.toml codec (was ConfigText.swift).

    python3 Tests/test_config_text.py
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)
sys.path.insert(0, os.path.join(ROOT, "pylib"))

import config_text as ct


def call(method, params):
    env = dict(os.environ)
    env["PYTHONPATH"] = os.path.join(ROOT, "pylib")
    env["PYTHONDONTWRITEBYTECODE"] = "1"
    r = subprocess.run(
        [sys.executable, "-B", "-m", "helper", "--call", method, "--params",
         json.dumps(params)],
        capture_output=True, text=True, cwd=ROOT, env=env,
    )
    return r.returncode, json.loads(r.stdout)


def entry(line):
    e = ct.config_entry(line)
    return [e[0], e[1]] if e else None


def setting(text, section, kv):
    return "\n".join(ct.config_setting(text.split("\n"), section, kv))


class Entries(unittest.TestCase):
    def test_values(self):
        self.assertEqual(entry("vim-mode = true"), ["vim-mode", "true"])
        self.assertEqual(entry("  width = 780  "), ["width", "780"])
        self.assertEqual(entry('shell = "/bin/zsh"'), ["shell", "/bin/zsh"])
        self.assertEqual(entry('re = \'\\d+ "x"\''), ["re", '\\d+ "x"'])
        self.assertEqual(entry('t = "a\\tb\\n\\"q\\" \\u00e9"'), ["t", 'a\tb\n"q" é'])
        self.assertEqual(entry('paths = ["~/a.md", \'~/b.md\', 3]'), ["paths", "~/a.md, ~/b.md, 3"])
        self.assertEqual(entry("n = 42 # answer"), ["n", "42"])
        self.assertEqual(entry('s = "x" # note'), ["s", "x"])
        self.assertEqual(entry("font = Menlo # legacy"), ["font", "Menlo # legacy"])
        self.assertEqual(entry('"view: keys" = "what"'), ["view: keys", "what"])
        self.assertEqual(entry('url = "a=b"'), ["url", "a=b"])
        self.assertEqual(entry("empty ="), ["empty", ""])

    def test_non_entries(self):
        for line in ["# comment", "", "   ", "[app]", "no equals sign"]:
            self.assertIsNone(entry(line), line)


class Lines(unittest.TestCase):
    def test_config_line(self):
        self.assertEqual(ct.config_line("float", "true"), "float = true")
        self.assertEqual(ct.config_line("width", "-12.5"), "width = -12.5")
        self.assertEqual(ct.config_line("width", "007"), 'width = "007"')
        self.assertEqual(ct.config_line("shell", "/bin/zsh"), 'shell = "/bin/zsh"')
        self.assertEqual(ct.config_line("view: keys", "x"), '"view: keys" = "x"')

    def test_round_trips(self):
        tricky = ["", "plain", 'with "quotes"', "back\\slash", "tab\tand\nnewline",
                  "# not a comment", "a, b, c", "é ü 🐙", "\x07", "[not an array]",
                  "true", "3.14"]
        for v in tricky:
            self.assertEqual(entry(ct.config_line("k", v)), ["k", v],
                             "round trip %r" % (v,))

    def test_round_trip_key_with_equals(self):
        self.assertEqual(entry(ct.config_line("a b = c", "v")), ["a b = c", "v"])


class Sections(unittest.TestCase):
    def test_headers(self):
        self.assertEqual(ct.config_section_header("[app]"), "app")
        self.assertEqual(ct.config_section_header("  [ notes ]  "), "notes")
        self.assertIsNone(ct.config_section_header("key = [a]"))
        self.assertIsNone(ct.config_section_header("[unclosed"))

    def test_lines_keep_empties(self):
        text = "a\n\nb\n"
        self.assertEqual("\n".join(text.split("\n")), text)

    def test_section_entries(self):
        lines = ("# top\n[app]\nfloat = true\n# comment\nshell = \"/bin/zsh\"\n"
                 "[notes]\npaths = \"~/a.md\"\n[ app ]\nfloat = false\n").split("\n")
        app = ct.config_section_entries(lines, "app")
        self.assertEqual([e[0] for e in app], [2, 4, 8])
        self.assertEqual([e[1] for e in app], ["float", "shell", "float"])
        self.assertEqual([e[2] for e in app], ["true", "/bin/zsh", "false"])
        self.assertEqual([e[1] for e in ct.config_section_entries(lines, "notes")], ["paths"])
        self.assertEqual(ct.config_section_entries(lines, "missing"), [])


class Setting(unittest.TestCase):
    def setUp(self):
        self.base = ('[app]\nshell = "/bin/bash"\n\n# notes window\n[notes]\n'
                     'enabled = true\nvim-mode = false\n\n')

    def test_update_in_place(self):
        self.assertEqual(setting(self.base, "notes", [("vim-mode", "true")]),
                         self.base.replace("vim-mode = false", "vim-mode = true"))

    def test_new_key_after_the_sections_last_entry(self):
        self.assertEqual(setting(self.base, "app", [("float", "true")]),
                         self.base.replace('shell = "/bin/bash"',
                                           'shell = "/bin/bash"\nfloat = true'))

    def test_new_key_under_an_empty_section(self):
        self.assertEqual(setting("[app]\n", "app", [("k", "v")]), '[app]\nk = "v"\n')

    def test_missing_section_appended(self):
        self.assertEqual(setting(self.base, "runtime", [("test", "value")]),
                         self.base + '\n\n[runtime]\ntest = "value"')

    def test_remove_keeps_everything_else(self):
        self.assertEqual(setting(self.base, "app", [("shell", None)]),
                         self.base.replace('shell = "/bin/bash"\n', ""))

    def test_removing_an_absent_key_changes_nothing(self):
        self.assertEqual(setting(self.base, "app", [("absent", None)]), self.base)

    def test_several_keys_in_one_edit(self):
        self.assertEqual(
            setting(self.base, "notes", [("enabled", "false"), ("vim-mode", None), ("font", "Menlo")]),
            self.base.replace("enabled = true\nvim-mode = false", 'enabled = false\nfont = "Menlo"'))

    def test_a_duplicated_key_edits_the_last_one(self):
        self.assertEqual(setting("[a]\nx = 1\nx = 2\n", "a", [("x", "3")]), "[a]\nx = 1\nx = 3\n")

    def test_a_section_split_in_two_gets_the_new_key_late(self):
        self.assertEqual(setting("[a]\nk = 1\n[b]\n[a]\n", "a", [("n", "2")]),
                         "[a]\nk = 1\n[b]\n[a]\nn = 2\n")

    def test_a_spaced_header_is_found(self):
        self.assertEqual(setting("[ a ]\nk = 1\n", "a", [("k", "2")]), "[ a ]\nk = 2\n")

    def test_same_key_in_another_section_is_not_touched(self):
        self.assertEqual(setting("[b]\nk = 1\n", "a", [("k", "2")]), "[b]\nk = 1\n\n\n[a]\nk = 2")


class Protocol(unittest.TestCase):
    def test_line(self):
        code, reply = call("config.line", {"key": "paths", "value": "a b"})
        self.assertEqual(code, 0)
        self.assertEqual(reply["result"]["line"], 'paths = "a b"')

    def test_section_entries(self):
        code, reply = call("config.section_entries",
                           {"text": "[app]\nfloat = true\n", "section": "app"})
        self.assertEqual(code, 0)
        self.assertEqual(reply["result"]["entries"], [{"index": 1, "key": "float", "value": "true"}])

    def test_setting(self):
        code, reply = call("config.setting",
                           {"text": "[app]\nk = 1\n", "section": "app",
                            "kv": [["k", "2"], ["n", None]]})
        self.assertEqual(code, 0)
        self.assertEqual(reply["result"]["text"], "[app]\nk = 2\n")

    def test_decode(self):
        code, reply = call("config.decode",
                           {"text": "# c\n[app]\nk = 1\ngarbage\n"})
        self.assertEqual(code, 0)
        lines = reply["result"]["lines"]
        self.assertEqual(lines[1], {"index": 1, "trimmed": "[app]", "header": "app"})
        self.assertEqual(lines[2], {"index": 2, "trimmed": "k = 1", "key": "k", "value": "1"})
        self.assertEqual(lines[3], {"index": 3, "trimmed": "garbage"})


if __name__ == "__main__":
    unittest.main(verbosity=2)
