#!/usr/bin/env python3
"""confluence glue — auth header + the rate-limit cooldown decision.

    python3 Tests/test_confluence_glue.py
"""
from __future__ import annotations

import base64
import os
import sys
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(ROOT, "pylib"))

import confluence_glue as cg


class Auth(unittest.TestCase):
    def test_email_implies_basic(self):
        header = cg.auth_header({"email": "a@x", "token": "tok"})
        self.assertEqual(header, "Basic " + base64.b64encode(b"a@x:tok").decode())

    def test_no_email_implies_bearer(self):
        self.assertEqual(cg.auth_header({"token": "tok"}), "Bearer tok")

    def test_explicit_modes_win(self):
        self.assertEqual(cg.auth_header({"auth": "bearer", "email": "a@x", "token": "t"}), "Bearer t")
        self.assertEqual(cg.auth_header({"auth": "BASIC", "email": "", "token": "t"}),
                         "Basic " + base64.b64encode(b":t").decode())

    def test_unknown_mode_derives(self):
        self.assertEqual(cg.auth_header({"auth": "oauth", "email": "a@x", "token": "t"}).split(" ")[0], "Basic")

    def test_no_token_is_empty(self):
        self.assertEqual(cg.auth_header({"email": "a@x", "token": ""}), "")
        self.assertEqual(cg.auth_header({}), "")
        self.assertEqual(cg.auth_header(None), "")


class RateLimit(unittest.TestCase):
    def test_not_limited(self):
        self.assertEqual(cg.rate_limit({}), {"limited": False})
        self.assertEqual(cg.rate_limit(None), {"limited": False})
        self.assertEqual(cg.rate_limit({"rateLimited": False}), {"limited": False})

    def test_seconds_from_retry_in(self):
        self.assertEqual(cg.rate_limit({"rateLimited": True, "retryIn": 45}),
                         {"limited": True, "seconds": 45})

    def test_floor_and_default(self):
        self.assertEqual(cg.rate_limit({"rateLimited": True, "retryIn": 1}),
                         {"limited": True, "seconds": 3})
        self.assertEqual(cg.rate_limit({"rateLimited": True}),
                         {"limited": True, "seconds": 30})
        self.assertEqual(cg.rate_limit({"rateLimited": True, "retryIn": "soon"}),
                         {"limited": True, "seconds": 30})


class Criteria(unittest.TestCase):
    def params(self, **over):
        p = {"query": " retry logic ", "modeIndex": 0, "titleOnly": False,
             "spacesAll": True, "spaces": [], "types": "page,blogpost",
             "modified": "", "contributorsAll": True, "contributors": [],
             "sort": "relevance", "favorites": False}
        p.update(over)
        return p

    def test_defaults(self):
        self.assertEqual(cg.criteria(self.params()), {
            "query": "retry logic", "mode": "all", "titleOnly": False,
            "spaces": [], "types": ["page", "blogpost"],
            "modified": "", "contributors": [], "sort": "relevance"})

    def test_mode_index_maps_and_clamps(self):
        self.assertEqual(cg.criteria(self.params(modeIndex=1))["mode"], "phrase")
        self.assertEqual(cg.criteria(self.params(modeIndex=2))["mode"], "any")
        self.assertEqual(cg.criteria(self.params(modeIndex=9))["mode"], "all")

    def test_all_flags_empty_the_lists(self):
        out = cg.criteria(self.params(spacesAll=False, spaces=["DEV"],
                                      contributorsAll=False, contributors=["me"],
                                      titleOnly=True))
        self.assertEqual(out["spaces"], ["DEV"])
        self.assertEqual(out["contributors"], ["me"])
        self.assertTrue(out["titleOnly"])
        out = cg.criteria(self.params(spacesAll=True, spaces=["DEV"]))
        self.assertEqual(out["spaces"], [])

    def test_types_split_drops_empties(self):
        self.assertEqual(cg.criteria(self.params(types="page,,comment"))["types"],
                         ["page", "comment"])
        self.assertEqual(cg.criteria(self.params(types=""))["types"], [])

    def test_favorites_only_when_in_scope(self):
        self.assertIn("favorites", cg.criteria(self.params(favorites=True)))
        self.assertNotIn("favorites", cg.criteria(self.params()))

    def test_passthroughs(self):
        out = cg.criteria(self.params(modified="-2w", sort="modified"))
        self.assertEqual((out["modified"], out["sort"]), ("-2w", "modified"))


if __name__ == "__main__":
    unittest.main(verbosity=2)
