#!/usr/bin/env python3
"""Tests for the Hyper+S unread-count backend (notify/*.py).

  python3 Tests/test_notifications.py      (stdlib unittest; no network)
"""
import os
import sys
import tempfile
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(ROOT, "notify"))

TMP = tempfile.mkdtemp(prefix="notify-test-")
os.environ["NOTIFY_CACHE_DIR"] = os.path.join(TMP, "cache")

import notify_poll  # noqa: E402


class TickTests(unittest.TestCase):
    CFG = dict(notify_poll.DEFAULTS, enabled="true", **{"outlook-enabled": "true"})

    def test_parse_badge(self):
        p = notify_poll.parse_badge
        self.assertEqual(p('"StatusLabel"={ "label"="3" }'), 3)
        self.assertEqual(p('"StatusLabel"={ "label"="" }'), 0)
        self.assertEqual(p('"StatusLabel"={ "label"="•" }'), "•")
        self.assertEqual(p('[ NULL ]  ASN:0x0-0x2e72e7: \n    bundleID=[ NULL ]'), 0)

    def test_badge_live(self):
        self.assertIsNone(notify_poll.badge("com.example.not-running"))
        self.assertEqual(notify_poll.badge("com.apple.finder"), 0)

    def chip(self, count, api):
        return notify_poll.chip("outlook", self.CFG, count, api)

    def test_chip_count_and_mentions(self):
        c = self.chip(3, {"ok": True, "mentions": 2, "unread": 1})
        self.assertEqual((c["count"], c["mentions"], c["warn"], c["shown"]), (3, 2, False, True))
        self.assertEqual(c["app"], "com.microsoft.Outlook")

    def test_chip_zero_is_hidden(self):
        self.assertFalse(self.chip(0, None)["shown"])

    def test_badge_zero_clears_mentions(self):
        c = self.chip(0, {"ok": True, "mentions": 2})
        self.assertEqual((c["mentions"], c["shown"]), (0, False))

    def test_app_not_running_falls_back_to_api(self):
        self.assertEqual(self.chip(None, {"ok": True, "mentions": 0, "unread": 4})["count"], 4)

    def test_api_error_warns(self):
        c = self.chip(None, {"ok": False, "error": "x"})
        self.assertTrue(c["warn"] and c["shown"])

    def test_sources_respect_enabled(self):
        self.assertEqual(notify_poll.sources(self.CFG), ["outlook"])
        cfg = dict(self.CFG, **{"imessage-enabled": "true", "sources": "outlook, imessage"})
        self.assertEqual(notify_poll.sources(cfg), ["outlook", "imessage"])

    def test_config_reads_section(self):
        conf = os.path.join(TMP, "commands.toml")
        with open(conf, "w") as fh:
            fh.write('[app]\nenabled = false\n[notifications]\nenabled = true\n'
                     'outlook-tag = "MAIL"  # comment\n[other]\noutlook-tag = "no"\n')
        sec = notify_poll.jira_config.read_section("notifications", conf)
        self.assertEqual(sec, {"enabled": "true", "outlook-tag": "MAIL"})


if __name__ == "__main__":
    unittest.main()
