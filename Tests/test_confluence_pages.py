#!/usr/bin/env python3
"""confluence preview — URL absolutisation, wsconf wrapping, page template.

    python3 Tests/test_confluence_pages.py
"""
from __future__ import annotations

import base64
import os
import re
import sys
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(ROOT, "pylib"))

import confluence_pages as cp

BASE = "https://x.atlassian.net/wiki"


def unwrap(src):
    b = src.split("://fetch/", 1)[1]
    b += "=" * (-len(b) % 4)
    return base64.urlsafe_b64decode(b).decode()


class Absolute(unittest.TestCase):
    def test_passthrough(self):
        for u in ("http://a/b", "https://a/b", "data:image/png;base64,x"):
            self.assertEqual(cp.absolute(u, BASE), u)

    def test_no_scheme_base_passes_through(self):
        self.assertEqual(cp.absolute("/x", "not a url"), "/x")

    def test_protocol_relative(self):
        self.assertEqual(cp.absolute("//cdn.x/a.png", BASE), "https://cdn.x/a.png")

    def test_absolute_paths_use_the_origin(self):
        self.assertEqual(cp.absolute("/wiki/images/a.png", BASE),
                         "https://x.atlassian.net/wiki/images/a.png")
        self.assertEqual(cp.absolute("/other", "https://x:8443/wiki"), "https://x:8443/other")

    def test_download_paths_keep_the_context_path(self):
        self.assertEqual(cp.absolute("/download/thumb.png", BASE), BASE + "/download/thumb.png")
        self.assertEqual(cp.absolute("/wiki/download/x", BASE),
                         "https://x.atlassian.net/wiki/download/x")

    def test_relative_hangs_off_the_base(self):
        self.assertEqual(cp.absolute("images/a.png", BASE), BASE + "/images/a.png")


class Rewrite(unittest.TestCase):
    def test_on_site_images_are_wrapped(self):
        html = cp.rewrite('<img alt="a" src="/wiki/images/a.png">', BASE)
        m = re.search(r'src="([^"]+)"', html)
        self.assertTrue(m.group(1).startswith("wsconf://fetch/"))
        self.assertEqual(unwrap(m.group(1)), "https://x.atlassian.net/wiki/images/a.png")

    def test_off_site_images_are_only_absolutised(self):
        html = cp.rewrite('<img src="https://cdn.example/a.png">', BASE)
        self.assertIn('src="https://cdn.example/a.png"', html)
        self.assertNotIn("wsconf://", html)

    def test_entities_are_unescaped_before_absolutising(self):
        html = cp.rewrite('<img src="/wiki/i?a=1&amp;b=2">', BASE)
        self.assertEqual(unwrap(re.search(r'src="([^"]+)"', html).group(1)),
                         "https://x.atlassian.net/wiki/i?a=1&b=2")

    def test_srcset_is_neutralised(self):
        html = cp.rewrite('<img src="/x.png" srcset="/x 2x">', BASE)
        self.assertIn("data-srcset=", html)
        self.assertNotIn(" srcset=", html)

    def test_uppercase_and_missing_src(self):
        html = cp.rewrite('<IMG SRC="/wiki/a.png"><img alt="b">', BASE)
        self.assertIn("wsconf://", html)
        self.assertIn('<img alt="b">', html)

    def test_no_host_base_changes_nothing(self):
        html = '<img src="/x.png">'
        self.assertEqual(cp.rewrite(html, "nope"), html)


def params(**over):
    p = {
        "page": {"site": BASE, "html": "<p>Hello retry</p>", "title": "My <Page>"},
        "row": {"type": "page", "url": "/wiki/p", "title": "row title"},
        "site": BASE,
        "terms": [{"text": "retry logic", "phrase": False, "prefix": True}],
        "colors": {"light": "dark", "text": "rgba(1,2,3,1.000)", "text92": "rgba(1,2,3,0.920)",
                   "dim": "rgba(4,4,4,1.000)", "accent": "rgba(5,5,5,1.000)",
                   "mantle": "rgba(6,6,6,1.000)", "hairline": "rgba(7,7,7,1.000)",
                   "warn": "rgba(8,8,8,1.000)", "warn35": "rgba(8,8,8,0.350)",
                   "warn75": "rgba(8,8,8,0.750)", "info": "rgba(9,9,9,1.000)"},
    }
    p.update(over)
    return p


class Preview(unittest.TestCase):
    def test_title_body_and_colors(self):
        html = cp.preview_html(params())
        self.assertIn('<div class="ws-title">My &lt;Page&gt;</div><p>Hello retry</p>', html)
        self.assertIn("color-scheme: dark;", html)
        self.assertIn("color: rgba(1,2,3,1.000);", html)
        self.assertIn("mark.wsh.on { background: rgba(8,8,8,0.750);", html)

    def test_terms_are_embedded_as_json(self):
        html = cp.preview_html(params())
        self.assertIn('const terms = [{"text":"retry logic","phrase":false,"prefix":true}];', html)
        self.assertIn("window.wsNext", html)
        self.assertIn("'\\\\b' + w.join('\\\\s+')", html)

    def test_no_placeholder_left(self):
        html = cp.preview_html(params())
        self.assertIsNone(re.search(r"@[a-zA-Z]{2,}", html))

    def test_attachment_image_embeds_the_absolute_url(self):
        html = cp.preview_html(params(
            page={"site": BASE, "type": "attachment", "mediaType": "image/png",
                  "url": "/wiki/att.png", "html": "", "title": "a"}))
        m = re.search(r'<img src="([^"]+)">', html)
        self.assertTrue(m.group(1).startswith("wsconf://fetch/"))
        self.assertEqual(unwrap(m.group(1)), "https://x.atlassian.net/wiki/att.png")

    def test_attachment_file_hint(self):
        html = cp.preview_html(params(
            page={"site": BASE, "type": "attachment", "mediaType": "application/pdf",
                  "url": "/wiki/f.pdf", "html": "", "title": "a"}))
        self.assertIn("Attachment (application/pdf) — open it in the browser.", html)

    def test_empty_body_placeholder_and_row_fallbacks(self):
        html = cp.preview_html(params(page={"html": ""}, row={"title": "row title"}))
        self.assertIn("(this page has no body)", html)
        self.assertIn('<div class="ws-title">row title</div>', html)

    def test_page_body_images_are_rewritten(self):
        html = cp.preview_html(params(page={"site": BASE, "html": '<img src="/wiki/a.png">', "title": "t"}))
        self.assertIn("wsconf://fetch/", html)


if __name__ == "__main__":
    unittest.main(verbosity=2)
