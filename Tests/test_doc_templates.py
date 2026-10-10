#!/usr/bin/env python3
"""doc_templates — the notes template logic, ported from DocTemplates.swift.

    python3 Tests/test_doc_templates.py
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

import doc_templates as dt


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


class Marker(unittest.TestCase):
    def test_no_marker(self):
        self.assertIsNone(dt.current("# Title\n\nbody"))

    def test_insert(self):
        a = dt.apply("# Title\n\nbody", "paper")
        self.assertEqual(a, '<div class="doc paper"></div>\n\n# Title\n\nbody')
        self.assertEqual(dt.current(a), "paper")

    def test_switch(self):
        a = dt.apply("# Title\n\nbody", "paper")
        b = dt.apply(a, "terminal")
        self.assertEqual(b, '<div class="doc terminal"></div>\n\n# Title\n\nbody')

    def test_footer_kept(self):
        foot = '<div class="doc paper" data-foot="Confidential — A&B"></div>\n\n# T'
        c = dt.apply(foot, "executive")
        self.assertEqual(c, '<div class="doc executive" data-foot="Confidential — A&B"></div>\n\n# T')

    def test_remove_restores_the_note(self):
        plain = "# Title\n\nbody"
        self.assertEqual(dt.apply(dt.apply(plain, "paper"), None), plain)

    def test_remove_without_marker_is_a_noop(self):
        plain = "# Title\n\nbody"
        self.assertEqual(dt.apply(plain, None), plain)

    def test_other_divs_are_not_markers(self):
        self.assertIsNone(dt.parse('<div class="note"></div>'))

    def test_empty_note(self):
        self.assertEqual(dt.apply("", "paper"), '<div class="doc paper"></div>\n\n')


class Names(unittest.TestCase):
    def test_default_names(self):
        self.assertEqual(dt.names(None), dt.BUILTIN)

    def test_configured_names(self):
        self.assertEqual(dt.names(" a , b,, "), ["a", "b"])

    def test_palette_blocks_only(self):
        css = (":root { --p-bg: #fff; }\n"
               ":root:has(.paper) {\n  --p-bg: #fff; --p-text: #000;\n}\n"
               ":root:has(.doc) body { page: doc; }\n"
               ":root:has(.nord) { --p-bg: #2e3440; }\n"
               ":root:has(.dracula) { --p-text: #fff; }")
        self.assertEqual(dt.from_css(css), ["paper", "nord"])

    def test_names_come_from_the_css(self):
        css = ":root:has(.paper) { --p-bg: #fff; }\n:root:has(.nord) { --p-bg: #2e3440; }"
        self.assertEqual(dt.names(None, css), ["paper", "nord"])

    def test_config_overrides_the_css(self):
        css = ":root:has(.paper) { --p-bg: #fff; }"
        self.assertEqual(dt.names("x, y", css), ["x", "y"])

    def test_no_css_palettes_means_builtin(self):
        self.assertEqual(dt.names(None, ""), dt.BUILTIN)


class Protocol(unittest.TestCase):
    def test_names(self):
        code, reply = call("doc_templates.names", {"configured": "a, b"})
        self.assertEqual(code, 0)
        self.assertEqual(reply["result"], {"names": ["a", "b"]})

    def test_current(self):
        code, reply = call("doc_templates.current",
                           {"text": '<div class="doc nord" data-foot="x"></div>\n\n# T'})
        self.assertEqual(code, 0)
        self.assertEqual(reply["result"], {"template": "nord"})

    def test_apply(self):
        code, reply = call("doc_templates.apply",
                           {"text": "# T\n\nbody", "template": "dracula"})
        self.assertEqual(code, 0)
        self.assertEqual(reply["result"]["text"],
                         '<div class="doc dracula"></div>\n\n# T\n\nbody')

    def test_menu(self):
        code, reply = call("doc_templates.menu",
                           {"text": '<div class="doc nord"></div>\n\n# T',
                            "configured": "", "css": ""})
        self.assertEqual(code, 0)
        self.assertEqual(reply["result"]["current"], "nord")
        self.assertEqual(reply["result"]["names"], dt.BUILTIN)

    def test_edit(self):
        code, reply = call("doc_templates.edit",
                           {"text": '<div class="doc paper" data-foot="x"></div>\n\n# T',
                            "template": "nord"})
        self.assertEqual(code, 0)
        self.assertEqual(reply["result"],
                         {"remove": 1, "insert": ['<div class="doc nord" data-foot="x"></div>']})

    def test_edit_plain(self):
        code, reply = call("doc_templates.edit", {"text": "# T", "template": None})
        self.assertEqual(code, 0)
        self.assertEqual(reply["result"], {"remove": 0, "insert": []})


if __name__ == "__main__":
    unittest.main(verbosity=2)
