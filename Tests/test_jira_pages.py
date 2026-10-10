#!/usr/bin/env python3
"""jira pages — the ticket detail page template and the comment fragment.

    python3 Tests/test_jira_pages.py
"""
from __future__ import annotations

import datetime
import os
import re
import sys
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(ROOT, "pylib"))

import jira_data
import jira_pages as jp

WORDS = jira_data.words({})
COLORS = {k: "#123456" for k in jp._HEX_KEYS}


def params(**over):
    p = {
        "fields": {"key": "APP-1", "title": "Fix it", "status": "Done",
                   "assignee": "Ada Lovelace", "created": "2020-03-04T05:06:07.000+0000"},
        "title": "row fallback",
        "workflow": None,
        "categories": {"Done": "done"},
        "words": WORDS,
        "labels": {},
        "colors": COLORS,
        "url": None,
        "comments": None,
    }
    p.update(over)
    return p


class Page(unittest.TestCase):
    def test_basics_and_escaping(self):
        html = jp.ticket_html(params(fields={"key": "A-1", "title": "<b>&\"risk\"</b>",
                                             "description": "one\nline\n\nsecond"}))
        self.assertIn('<span class="key">A-1</span>', html)
        self.assertIn("&lt;b&gt;&amp;&quot;risk&quot;&lt;/b&gt;", html)
        self.assertIn('<p>one<br>line</p><p>second</p>', html)
        self.assertNotRegex(html, r"\$[a-z_]")  # every template placeholder substituted

    def test_title_falls_back_to_summary_then_row_title(self):
        self.assertIn("<h1>sum</h1>", jp.ticket_html(params(fields={"summary": "sum"})))
        self.assertIn("<h1>row fallback</h1>", jp.ticket_html(params(fields={})))

    def test_no_description_placeholder(self):
        self.assertIn("No description.", jp.ticket_html(params(fields={})))

    def test_stepper_uses_directory_categories(self):
        html = jp.ticket_html(params(
            fields={"status": "Doing"},
            categories={"Doing": "indeterminate"}))
        self.assertIn('<span class="step done">✓ To Do</span>', html)
        self.assertIn('class="step now">In Progress <span class="sub">· Doing</span>', html)
        self.assertIn('class="step ">Done</span>', html)

    def test_stepper_word_fallback_when_directory_is_silent(self):
        html = jp.ticket_html(params(fields={"status": "Won't Do"}, categories={}))
        self.assertIn('class="step now">Done <span class="sub">· Won\'t Do</span>', html)

    def test_explicit_workflow_wins(self):
        html = jp.ticket_html(params(fields={"status": "Doing"},
                                     workflow="Triage, Doing, Shipped"))
        self.assertIn('<span class="step done">✓ Triage</span>', html)
        self.assertIn('class="step now">Doing</span>', html)
        self.assertNotIn("To Do", html)

    def test_no_stepper_without_status(self):
        html = jp.ticket_html(params(fields={"status": ""}))
        self.assertNotIn('<div class="steps">', html)

    def test_pills_priority_release_and_label_overflow(self):
        labels = ", ".join("L%d" % i for i in range(8))
        html = jp.ticket_html(params(fields={
            "priority": "High", "releaseLabel": "2026.1", "labels": labels}))
        self.assertIn('<span class="pill warn">High</span>', html)
        self.assertIn('<span class="pill">2026.1</span>', html)
        self.assertIn('title="L6, L7">+2</span>', html)
        self.assertEqual(html.count('<span class="pill dim">'), 6)

    def test_props_assignee_default_and_filters(self):
        html = jp.ticket_html(params(fields={"reporter": "", "project": "APP"}))
        self.assertIn("<dt>Assignee</dt><dd>Unassigned</dd>", html)
        self.assertIn("<dt>Project</dt><dd>APP</dd>", html)
        self.assertNotIn("<dt>Reporter</dt>", html)

    def test_all_fields_sorted_filtered_and_labeled(self):
        html = jp.ticket_html(params(
            fields={"key": "A-1", "zeta": "Z", "alpha": "A", "description": "d",
                    "comments": "x", "__hidden": "h", "empty": ""},
            labels={"alpha": "Alpha!"}))
        table = html.split('<div class="pane" id="f"><dl>')[1].split("</dl>")[0]
        self.assertEqual(table, "<dt>Alpha!</dt><dd>A</dd><dt>key</dt><dd>A-1</dd>"
                                "<dt>zeta</dt><dd>Z</dd>")

    def test_comments_line_loading_then_count(self):
        self.assertIn("Loading comments…", jp.ticket_html(params(comments=None)))
        self.assertIn('<span class="tab" data-t="c" id="ctab">Comments</span>',
                      jp.ticket_html(params(comments=None)))
        html = jp.ticket_html(params(comments=[{"author": "a", "body": "b", "created": ""}]))
        self.assertIn(">Comments 1</span>", html)
        self.assertIn("No comments.", jp.ticket_html(params(comments=[])))

    def test_url_buttons(self):
        html = jp.ticket_html(params(url="https://x/browse/A-1"))
        self.assertIn('data-a="open"', html)
        self.assertIn('data-a="copy-link"', html)
        self.assertNotIn('data-a="open"', jp.ticket_html(params(url=None)))

    def test_colors_reach_the_css_variables(self):
        html = jp.ticket_html(params())
        self.assertIn("--bg:#123456;", html)
        self.assertIn("--warn:#123456", html)


class CommentsHTML(unittest.TestCase):
    def test_empty(self):
        self.assertEqual(jp.comments_html([]), '<p class="empty">No comments.</p>')

    def test_newest_first_with_initials_and_breaks(self):
        html = jp.comments_html([
            {"author": "Ada Lovelace", "body": "first", "created": ""},
            {"author": "grace", "body": "line1\nline2", "created": ""},
        ])
        self.assertLess(html.index("line1<br>line2"), html.index("Ada Lovelace"))
        self.assertIn('<span class="av">AL</span>', html)
        self.assertIn('<span class="av">g</span>', html)
        self.assertIn("<b>Ada Lovelace</b>", html)

    def test_escaping(self):
        html = jp.comments_html([{"author": "<x>", "body": "a&b", "created": ""}])
        self.assertIn("<b>&lt;x&gt;</b>", html)
        self.assertIn("a&amp;b", html)

    def test_skips_non_dict_rows(self):
        self.assertEqual(jp.comments_html(["nope"]), "")


class Date(unittest.TestCase):
    def test_other_year_is_date_only(self):
        self.assertEqual(jp.date("2020-03-04T05:06:07.000+0000"), "Mar 4, 2020")
        self.assertEqual(jp.date("2020-03-04T05:06:07+0000"), "Mar 4, 2020")
        self.assertEqual(jp.date("2020-03-04"), "Mar 4, 2020")

    def test_current_year_keeps_the_time(self):
        now = datetime.datetime.now().astimezone()
        stamp = now.strftime("%Y-%m-%dT%H:%M:%S%z")
        self.assertRegex(jp.date(stamp), r"^[A-Z][a-z]{2} \d{1,2}, \d{2}:\d{2}$")

    def test_unparseable_and_empty_pass_through(self):
        self.assertEqual(jp.date("yesterday"), "yesterday")
        self.assertEqual(jp.date(""), "")


if __name__ == "__main__":
    unittest.main(verbosity=2)
