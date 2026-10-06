#!/usr/bin/env python3
"""Tests for the Hyper+S unread-count backend (notify/*.py).

  python3 Tests/test_notifications.py      (stdlib unittest; no network)
"""
import datetime as dt
import io
import json
import os
import sys
import tempfile
import time
import unittest
import urllib.error
import urllib.parse

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(ROOT, "notify"))

TMP = tempfile.mkdtemp(prefix="notify-test-")
os.environ["NOTIFY_WEBEX_JSON"] = os.path.join(TMP, "webex.json")
os.environ["NOTIFY_CACHE_DIR"] = os.path.join(TMP, "cache")

import notify_poll  # noqa: E402
import webex_api  # noqa: E402

NOW = dt.datetime(2026, 9, 30, 12, 0, tzinfo=dt.timezone.utc)


def iso(minutes_ago):
    return (NOW - dt.timedelta(minutes=minutes_ago)).strftime("%Y-%m-%dT%H:%M:%S.000Z")


class Resp(io.BytesIO):
    def __enter__(self):
        return self

    def __exit__(self, *a):
        return False


class FakeWebex:
    """Routes urllib requests to canned data; records every call."""

    def __init__(self, rooms, seen, mentions, fail=None):
        self.rooms, self.seen, self.mentions = rooms, seen, mentions
        self.fail = fail or {}      # path → list of status codes to return first
        self.calls = []
        self.tokens = 0

    def __call__(self, req, timeout=None):
        u = urllib.parse.urlparse(req.full_url)
        q = dict(urllib.parse.parse_qsl(u.query))
        path = u.path.replace("/v1", "", 1)
        self.calls.append((path, q))
        if self.fail.get(path):
            code = self.fail[path].pop(0)
            hdrs = {"Retry-After": "1"} if code == 429 else {}
            raise urllib.error.HTTPError(req.full_url, code, "x", hdrs, None)
        if path == "/access_token":
            self.tokens += 1
            return Resp(json.dumps({"access_token": f"tok{self.tokens}", "expires_in": 3600,
                                    "refresh_token": "ref2",
                                    "refresh_token_expires_in": 7776000}).encode())
        if path == "/people/me":
            body = {"id": "ME", "displayName": "Me"}
        elif path == "/rooms":
            body = {"items": self.rooms}
        elif path == "/memberships":
            s = self.seen.get(q["roomId"])
            body = {"items": [{"personId": "ME", **({"lastSeenDate": s} if s else {})}]}
        elif path == "/messages":
            assert q["mentionedPeople"] == "me"
            body = {"items": self.mentions.get(q["roomId"], [])}
        else:
            raise AssertionError(path)
        return Resp(json.dumps(body).encode())


class WebexPollTests(unittest.TestCase):
    def setUp(self):
        webex_api.NOW = lambda: NOW
        webex_api.SLEEP = lambda s: None
        webex_api.save_creds({"client_id": "c", "client_secret": "s", "access_token": "tok0",
                              "expires_at": time.time() + 3600, "refresh_token": "ref"})

    def run_poll(self, fake, **kw):
        webex_api.URLOPEN = fake
        return webex_api.poll(**kw)

    def test_unread_and_mentions(self):
        fake = FakeWebex(
            rooms=[{"id": "g1", "type": "group", "title": "Team", "lastActivity": iso(1)},
                   {"id": "d1", "type": "direct", "title": "Bob", "lastActivity": iso(5)},
                   {"id": "g2", "type": "group", "title": "Read", "lastActivity": iso(10)},
                   {"id": "old", "type": "group", "title": "Old", "lastActivity": iso(60 * 48)}],
            seen={"g1": iso(30), "d1": iso(30), "g2": iso(2)},
            mentions={"g1": [{"created": iso(1), "text": "@Me look"},
                             {"created": iso(40), "text": "already seen"}]})
        res = self.run_poll(fake)
        self.assertTrue(res["ok"], res)
        self.assertEqual(res["unread"], 2)          # g1 + d1; g2 read; old outside window
        self.assertEqual(res["mentions"], 1)
        self.assertEqual(res["items"][0]["space"], "Team")
        paths = [p for p, _ in fake.calls]
        self.assertNotIn(("/messages", {"roomId": "d1"}), fake.calls)  # no mentions in DMs
        self.assertEqual(paths.count("/messages"), 1)

    def test_no_read_status_uses_age_window(self):
        fake = FakeWebex(
            rooms=[{"id": "g1", "type": "group", "title": "T", "lastActivity": iso(1)}],
            seen={}, mentions={"g1": [{"created": iso(10)}, {"created": iso(60 * 30)}]})
        res = self.run_poll(fake, max_age_hours=24)
        self.assertEqual((res["unread"], res["mentions"]), (0, 1))

    def test_401_refreshes_once_then_retries(self):
        fake = FakeWebex(rooms=[], seen={}, mentions={}, fail={"/people/me": [401]})
        res = self.run_poll(fake)
        self.assertTrue(res["ok"], res)
        self.assertEqual(fake.tokens, 1)
        self.assertEqual(webex_api.load_creds()["refresh_token"], "ref2")

    def test_refresh_failure_is_auth_error(self):
        fake = FakeWebex(rooms=[], seen={}, mentions={},
                         fail={"/people/me": [401], "/access_token": [400]})
        res = self.run_poll(fake)
        self.assertFalse(res["ok"])
        self.assertTrue(res["auth"])
        self.assertIn("--login", res["error"])

    def test_not_signed_in(self):
        webex_api.save_creds({})
        res = self.run_poll(FakeWebex([], {}, {}))
        self.assertEqual((res["ok"], res["auth"]), (False, True))

    def test_429_waits_and_retries(self):
        waits = []
        webex_api.SLEEP = waits.append
        fake = FakeWebex(rooms=[], seen={}, mentions={}, fail={"/rooms": [429]})
        res = self.run_poll(fake)
        self.assertTrue(res["ok"], res)
        self.assertEqual(waits, [1])


class TickTests(unittest.TestCase):
    CFG = dict(notify_poll.DEFAULTS, enabled="true")

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
        return notify_poll.chip("webex", self.CFG, count, api)

    def test_chip_count_and_mentions(self):
        c = self.chip(3, {"ok": True, "mentions": 2, "unread": 1})
        self.assertEqual((c["count"], c["mentions"], c["warn"], c["shown"]), (3, 2, False, True))
        self.assertEqual(c["app"], "Cisco-Systems.Spark")

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

    def test_parse_window(self):
        self.assertEqual(notify_poll.parse_window("count\t2\nspace\tTeam\nspace\tBob\n"),
                         (2, ["Team", "Bob"]))
        self.assertEqual(notify_poll.parse_window("count\t0\n"), (0, []))
        self.assertEqual(notify_poll.parse_window("count\t99+\n"), ("•", []))

    def test_space_link(self):
        import base64
        rid = base64.b64encode(b"ciscospark://us/ROOM/abc-123").decode().rstrip("=")
        self.assertEqual(webex_api.space_link(rid), "webexteams://im?space=abc-123")

    def test_sources_respect_enabled(self):
        self.assertEqual(notify_poll.sources(self.CFG), ["webex"])
        cfg = dict(self.CFG, **{"outlook-enabled": "true", "sources": "outlook, webex"})
        self.assertEqual(notify_poll.sources(cfg), ["outlook", "webex"])

    def test_config_reads_section(self):
        conf = os.path.join(TMP, "commands.toml")
        with open(conf, "w") as fh:
            fh.write('[app]\nenabled = false\n[notifications]\nenabled = true\n'
                     'webex-tag = "WX"  # comment\n[other]\nwebex-tag = "no"\n')
        sec = notify_poll.jira_config.read_section("notifications", conf)
        self.assertEqual(sec, {"enabled": "true", "webex-tag": "WX"})


if __name__ == "__main__":
    unittest.main()
