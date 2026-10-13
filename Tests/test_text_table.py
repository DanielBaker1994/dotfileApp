#!/usr/bin/env python3
"""text_table — delimiter detection, normalization, the format registry and
the file-based CLI (the "Text to Table" panel's engine).

    python3 Tests/test_text_table.py      # or: ./ws test text-table
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PYLIB = os.path.join(ROOT, "pylib")
sys.path.insert(0, PYLIB)

from text_table import FORMATS, parse_text, render  # noqa: E402
from text_table.parse import is_rule_row  # noqa: E402


class Detect(unittest.TestCase):
    def test_tab_keeps_empty_cells(self):
        t = parse_text("Name\tStatus\tOwner\nDocs\t\tLee\n")
        self.assertEqual(t.delimiter, "tab")
        self.assertEqual(t.header, ["Name", "Status", "Owner"])
        self.assertEqual(t.rows, [["Docs", "", "Lee"]])

    def test_markdown_pipe_table_and_rule_row(self):
        t = parse_text("| Name | Status |\n|------|:------:|\n| a | b \\| c |\n")
        self.assertEqual(t.delimiter, "pipe")
        self.assertEqual(t.rows, [["a", "b | c"]])
        self.assertIn("dropped 1 separator row", t.warnings)

    def test_colon_ignores_times_and_urls(self):
        t = parse_text("Field: Value\nDue: 10:30 today\nLink: https://a.b/c\n")
        self.assertEqual(t.delimiter, "colon")
        self.assertEqual(t.rows, [["Due", "10:30 today"], ["Link", "https://a.b/c"]])

    def test_dash_runs_are_separators(self):
        t = parse_text("Item —— Cost\nCoffee —— 3.50\n————————\nBagel -- 2.25\n")
        self.assertEqual(t.delimiter, "dashes")
        self.assertEqual(t.rows, [["Coffee", "3.50"], ["Bagel", "2.25"]])

    def test_multi_space_columns(self):
        t = parse_text("Name      Status        Owner\nLogin     In Progress   Dan\n")
        self.assertEqual(t.delimiter, "spaces")
        self.assertEqual(t.rows, [["Login", "In Progress", "Dan"]])

    def test_consistency_beats_frequency(self):
        # Commas are more frequent, but only tabs give every row 2 cells.
        text = "Name\tNotes\nA\tred, green, blue, cyan\nB\tone\nC\tx, y\n"
        self.assertEqual(parse_text(text).delimiter, "tab")

    def test_csv_quoting(self):
        t = parse_text('name,desc\n"Widget, large",big\n')
        self.assertEqual(t.delimiter, "comma")
        self.assertEqual(t.rows, [["Widget, large", "big"]])

    def test_prose_is_one_column(self):
        t = parse_text("Just a sentence.\nAnd another, with a comma.\n")
        self.assertIsNone(t.delimiter)
        self.assertEqual(t.columns, 1)

    def test_forced_delimiter(self):
        t = parse_text("a b c\nd e f\n", delimiter="space")
        self.assertEqual(t.header, ["a", "b", "c"])


class Normalize(unittest.TestCase):
    def test_ragged_rows_pad_and_fold(self):
        t = parse_text("A\tB\tC\n1\t2\n3\t4\t5\t6\n7\t8\t9\n")
        self.assertEqual(t.rows, [["1", "2", ""], ["3", "4", "5 6"], ["7", "8", "9"]])
        self.assertTrue(any("padded" in w for w in t.warnings))
        self.assertTrue(any("joined" in w for w in t.warnings))

    def test_ocr_noise(self):
        t = parse_text("﻿Name \tOwner  \r\n\r\nA​\t  Dan   Baker\r\n")
        self.assertEqual(t.header, ["Name", "Owner"])
        self.assertEqual(t.rows, [["A", "Dan Baker"]])

    def test_no_header(self):
        t = parse_text("1\t2\n3\t4\n", has_header=False)
        self.assertEqual(t.header, ["Column 1", "Column 2"])
        self.assertEqual(len(t.rows), 2)

    def test_empty(self):
        t = parse_text("  \n\n")
        self.assertTrue(t.is_empty())
        self.assertEqual(render(t, "outlook"), "")

    def test_rule_rows(self):
        for s in ["---", "|---|:--:|", "+----+----+", "======", "——————", "-----"]:
            self.assertTrue(is_rule_row(s), s)
        for s in ["a---b", "--", "| a | b |", "Total —— 5"]:
            self.assertFalse(is_rule_row(s), s)


TABLE = "Name\tPoints\nA|b\t5\n<x>\t12\n"


class Formats(unittest.TestCase):
    def setUp(self):
        self.t = parse_text(TABLE)

    def test_registry_is_complete(self):
        for f in FORMATS.values():
            self.assertIn(f.mime, ("text/plain", "text/html"), f.name)
            if f.fallback:
                self.assertIn(f.fallback, FORMATS)
                self.assertEqual(f.mime, "text/html")
            self.assertTrue(f.render(self.t))

    def test_markdown_escapes_and_aligns_numbers(self):
        md = render(self.t, "markdown").splitlines()
        self.assertEqual(md[0], "| Name | Points |")
        self.assertEqual(md[1], "| ---- | -----: |")
        self.assertEqual(md[2], "| A\\|b |      5 |")

    def test_outlook_is_inline_styled_and_escaped(self):
        h = render(self.t, "outlook")
        self.assertIn('<meta charset="utf-8">', h)
        self.assertIn("&lt;x&gt;", h)
        self.assertNotIn("<style", h)
        self.assertNotIn("class=", h)
        self.assertEqual(h.count("<th style="), 2)
        self.assertIn("text-align:right", h)

    def test_tsv_csv_grid_jira(self):
        self.assertEqual(render(self.t, "tsv"), "Name\tPoints\nA|b\t5\n<x>\t12\n")
        self.assertEqual(render(self.t, "csv").splitlines()[1], "A|b,5")
        self.assertTrue(render(self.t, "grid").startswith("+------+--------+\n| Name | Points |"))
        self.assertEqual(render(self.t, "jira").splitlines()[1], "|A\\|b|5|")


def _read(path):
    with open(path, encoding="utf-8") as fh:
        return fh.read()


class Cli(unittest.TestCase):
    def run_cli(self, *args, stdin=None):
        env = dict(os.environ, PYTHONPATH=PYLIB, PYTHONDONTWRITEBYTECODE="1")
        return subprocess.run(
            [sys.executable, "-m", "text_table", *args],
            input=stdin, capture_output=True, text=True, env=env,
        )

    def test_files_in_files_out(self):
        with tempfile.TemporaryDirectory() as d:
            src, out, fb, meta = (os.path.join(d, n) for n in ("in.txt", "out.html", "out.md", "meta.json"))
            with open(src, "w") as fh:
                fh.write(TABLE)
            r = self.run_cli(src, out, "--format", "outlook", "--fallback-out", fb, "--meta", meta)
            self.assertEqual(r.returncode, 0, r.stderr)
            self.assertIn("<table", _read(out))
            self.assertTrue(_read(fb).startswith("| Name"))
            m = json.loads(_read(meta))
            self.assertEqual(
                {k: m[k] for k in ("ok", "mime", "fallbackFormat", "delimiter", "columns", "rows")},
                {"ok": True, "mime": "text/html", "fallbackFormat": "markdown", "delimiter": "tab", "columns": 2, "rows": 2},
            )
            self.assertEqual(sorted(os.listdir(d)), ["in.txt", "meta.json", "out.html", "out.md"], "no temp leftovers")

    def test_stdin_stdout_filter(self):
        r = self.run_cli("-", "-", "--format", "markdown", stdin=TABLE)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertTrue(r.stdout.startswith("| Name | Points |"))

    def test_empty_input_exit_1(self):
        r = self.run_cli("-", "-", stdin="\n\n")
        self.assertEqual(r.returncode, 1)

    def test_usage_errors(self):
        self.assertEqual(self.run_cli("-", "-", "--format", "nope", stdin="a").returncode, 2)
        self.assertEqual(self.run_cli().returncode, 2)

    def test_list_formats_json(self):
        r = self.run_cli("--list-formats", "--json")
        names = [f["name"] for f in json.loads(r.stdout)]
        self.assertEqual(names[0], "outlook")
        self.assertEqual(set(names), set(FORMATS))


if __name__ == "__main__":
    unittest.main(verbosity=1)
