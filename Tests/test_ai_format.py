#!/usr/bin/env python3
"""ai_format — the AI view's text logic, ported from AIFormat.swift.

    python3 Tests/test_ai_format.py
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

import ai_format as ai

PANDOC = "/opt/homebrew/bin/pandoc"


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


class CSVTables(unittest.TestCase):
    def test_header_rule_row(self):
        t = ai.csv_convert("example,example1,example2\n---\n1,2,3")
        self.assertEqual(t, "| example | example1 | example2 |\n| --- | --- | --- |\n| 1 | 2 | 3 |")

    def test_three_two_column_rows_need_no_rule(self):
        t = ai.csv_convert("name, status\nweb01, up\nweb02, down")
        self.assertTrue(t.startswith("| name | status |\n| --- | --- |\n| web01 | up |"), t)

    def test_two_three_column_rows_need_no_rule(self):
        self.assertIn("| 1 | 2 | 3 |", ai.csv_convert("a,b,c\n1,2,3"))

    def test_email_with_commas_stays_email(self):
        prose = "Hi Raj,\n\nme and John was there, we seen it and it look good.\n\nThanks,\nDan"
        self.assertEqual(ai.csv_convert(prose), prose)

    def test_two_short_comma_lines_are_not_a_table(self):
        two = "Hi team, thanks\nSee you, Dan"
        self.assertEqual(ai.csv_convert(two), two)

    def test_code_blocks_are_left_alone(self):
        fenced = "```\na,b,c\n---\n1,2,3\n```"
        self.assertEqual(ai.csv_convert(fenced), fenced)

    def test_blank_lines_around(self):
        mixed = ai.csv_convert("Status below\nhost,state\n---\nweb01,up\nMore soon.")
        self.assertEqual(mixed, "Status below\n\n| host | state |\n| --- | --- |\n| web01 | up |\n\nMore soon.")

    def test_ragged_rows_are_not_a_table(self):
        self.assertEqual(ai.csv_convert("a,b\n---\n1,2,3"), "a,b\n---\n1,2,3")


class WordGuard(unittest.TestCase):
    def test_a_list_may_drop_ordinals(self):
        steps = "We need to do three things: first update the changelog, second run the tests."
        self.assertTrue(ai.word_check(steps, "We need to do three things:\n\n1. update the changelog\n2. run the tests")["ok"])

    def test_answer_that_adds_words(self):
        self.assertFalse(ai.word_check("What time is the meeting tomorrow?",
                                       "The meeting tomorrow is at 10:00 AM.")["ok"])

    def test_dropped_sentence_is_caught(self):
        self.assertFalse(ai.word_check("Hi team, see you there. You're the best.",
                                       "Hi team, see you there.")["ok"])

    def test_table_header_may_be_new(self):
        self.assertTrue(ai.word_check("Anna is in Paris, Ben is in Rome.",
                                      "| Name | City |\n| --- | --- |\n| Anna | Paris |\n| Ben | Rome |")["ok"])

    def test_markup_is_not_words(self):
        self.assertTrue(ai.word_check("run [[CODE1]] then **stop**",
                                      "- run [[CODE1]]\n- then **stop**")["ok"])


class Rules(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.mkdtemp(prefix="ai-rules-test-")
        self.addCleanup(shutil.rmtree, self.dir, True)

        def write(name, text):
            with open(os.path.join(self.dir, name), "w", encoding="utf-8") as f:
                f.write(text)

        write("a.md", "---\nname: A\noutput: diff\nprompt: Proofread this draft:\nthen: b\ncsv-tables: true\n---\nFix the\ngrammar.\n- one")
        write("b.md", "---\nname: B\nkeep-words: true\nthen: a.md\n---\nFormat it.")
        write("c.md", "---\nthen: nope.md\n---\nx")

    def test_front_matter(self):
        a = ai.rule_load(self.dir + "/a.md")
        self.assertEqual(a["warnings"], [])
        self.assertEqual(a["then"], "b.md")
        self.assertTrue(a["csvTables"])
        self.assertFalse(a["keepWords"])
        self.assertEqual(ai.rule_wrap(a, "hello"), "Proofread this draft:\n\nhello")
        self.assertTrue(ai.rule_prepare(a, "x,y\n---\n1,2").startswith("| x | y |"))
        self.assertEqual(ai.rule_instructions(a, False), "Fix the grammar.\n- one")
        self.assertTrue(ai.rule_preview(a).endswith("\u2192 then b.md"))

    def test_chain(self):
        chain = ai.rule_chain(self.dir + "/a.md")
        self.assertEqual([r["name"] for r in chain], ["A", "B"])
        self.assertTrue(any("loops" in w for w in chain[0]["warnings"]))

    def test_keep_words_accept(self):
        b = ai.rule_chain(self.dir + "/a.md")[1]
        self.assertIsNone(ai.rule_accept(b, "one two", "- one\n- two")["note"])
        bad = ai.rule_accept(b, "one two", "three")
        self.assertEqual(bad["text"], "one two")
        self.assertIsNotNone(bad["note"])

    def test_missing_then_is_a_warning(self):
        rules = ai.rule_chain(self.dir + "/c.md")
        self.assertTrue(any("nope.md" in w for w in rules[0]["warnings"]))

    def test_real_rules(self):
        rules = os.path.join(ROOT, "rules")
        g = ai.rule_chain(rules + "/grammar-check.md")
        self.assertEqual([r["path"].rsplit("/", 1)[-1] for r in g],
                         ["grammar-check.md", "markdown-format.md"])
        self.assertTrue(all(not r["warnings"] for r in g), [r["warnings"] for r in g])
        self.assertTrue(g[1]["keepWords"] and not g[0]["keepWords"])
        self.assertNotIn("table with", g[0]["instructions"].lower())
        self.assertNotIn("bullet list", g[0]["instructions"])
        ask = ai.rule_load(rules + "/ask.md")
        self.assertEqual(ask["warnings"], [])
        self.assertFalse(ask["output"] == "diff")
        self.assertEqual(ask["prompt"], "")


class CodeGuardRules(unittest.TestCase):
    def test_inline_and_fence(self):
        g = ai.code_guard("use `x = 1` here\n\n```swift\nlet a = 1\n```\ndone")
        self.assertEqual(g["codes"], ["`x = 1`", "```swift\nlet a = 1\n```"])
        self.assertIn("[[CODE1]]", g["text"])
        self.assertIn("[[CODE2]]", g["text"])
        r = ai.code_restore(g["codes"], g["text"].replace("[[CODE1]]", "`x = 1`"))
        self.assertEqual(r["missing"], 1)

    def test_round_trip(self):
        src = "keep `a` and `b` intact"
        g = ai.code_guard(src)
        self.assertEqual(ai.code_restore(g["codes"], g["text"])["text"], src)


class RichTextRules(unittest.TestCase):
    def test_webex_tables_become_text(self):
        md = "| a | b |\n| --- | --- |\n| 1 | 2 |"
        self.assertEqual(ai.markdown_for(md, "outlook"), md)
        self.assertNotIn("| a |", ai.markdown_for(md, "webex"))

    @unittest.skipUnless(os.access(PANDOC, os.X_OK), "pandoc missing")
    def test_warning_alert_styled_inline(self):
        h = ai.html("> [!WARNING]\n> Mind the gap\n", "outlook", PANDOC)
        self.assertIsNotNone(h)
        self.assertNotIn('class="warning"', h)
        self.assertIn("border-left:3px solid #9a6700", h)
        self.assertIn('font-weight:bold;color:#9a6700">Warning', h)
        self.assertIn("Mind the gap", h)


class Protocol(unittest.TestCase):
    def test_csv_convert(self):
        code, reply = call("ai.csv_convert", {"s": "a,b\n---\n1,2"})
        self.assertEqual(code, 0)
        self.assertEqual(reply["result"]["text"], "| a | b |\n| --- | --- |\n| 1 | 2 |")

    def test_word_check(self):
        code, reply = call("ai.word_check", {"input": "one two", "output": "three"})
        self.assertEqual(code, 0)
        self.assertFalse(reply["result"]["ok"])
        self.assertEqual(reply["result"]["added"], ["three"])

    def test_rule_round_trip(self):
        tmp = tempfile.mkdtemp(prefix="ai-rule-proto-")
        self.addCleanup(shutil.rmtree, tmp, True)
        path = os.path.join(tmp, "r.md")
        with open(path, "w", encoding="utf-8") as f:
            f.write("---\nname: R\nthen: t\n---\nfix it")
        code, reply = call("ai.rule_load", {"path": path})
        self.assertEqual(code, 0)
        self.assertEqual(reply["result"]["rule"]["then"], "t.md")
        code, reply = call("ai.rule_arguments", {"rule": reply["result"]["rule"], "guarded": False})
        self.assertEqual(code, 0)
        self.assertEqual(reply["result"]["args"], ["respond", "--stream", "-i", "fix it"])

    def test_estimate_and_parts(self):
        code, reply = call("ai.estimate", {"s": "x" * 320})
        self.assertEqual(code, 0)
        self.assertEqual(reply["result"]["tokens"], 100)
        code, reply = call("ai.parts", {"s": "a" * 500 + "\n\n" + "b" * 500, "budget": 100})
        self.assertEqual(code, 0)
        self.assertEqual(len(reply["result"]["parts"]), 2)


if __name__ == "__main__":
    unittest.main(verbosity=2)
