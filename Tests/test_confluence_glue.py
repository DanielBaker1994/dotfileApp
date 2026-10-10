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


if __name__ == "__main__":
    unittest.main(verbosity=2)
