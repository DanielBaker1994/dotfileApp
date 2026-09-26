#!/usr/bin/env python3
"""Confluence search: CQL building, the API CLI against the fake Confluence
(confluence/fake_confluence.py, reached through a `curl` shim on PATH - the
same implementation `bin/fake-confluence.sh` serves), favorites.

    python3 Tests/test_confluence.py
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CONF = os.path.join(ROOT, "confluence")
sys.path.insert(0, CONF)
import confluence_config as cc  # noqa: E402
import fake_confluence as fake  # noqa: E402

SHIM = f"""#!/bin/sh
exec {sys.executable} {os.path.join(CONF, "fake_confluence.py")} curl "$@"
"""


def cfg(**kw) -> cc.Config:
    return cc.Config({"site": "https://conf.example.com/confluence", "token": "fake-token", **kw}, "/dev/null")


class CQLTests(unittest.TestCase):
    def cql(self, **crit):
        return cc.criteria_cql(crit, cfg())

    def test_modes(self):
        self.assertEqual(self.cql(query="blue green", types=["page"]),
                         '(text ~ "blue" AND text ~ "green") AND type in (page)')
        self.assertEqual(self.cql(query="blue green", mode="any", types=["page"]),
                         '(text ~ "blue" OR text ~ "green") AND type in (page)')
        self.assertEqual(self.cql(query="blue green deploy", mode="phrase", types=["page"]),
                         'text ~ "\\"blue green deploy\\"" AND type in (page)')

    def test_quoted_parts_are_phrases_in_any_mode(self):
        self.assertIn('text ~ "\\"blue green\\""', self.cql(query='"blue green" rollback'))
        self.assertIn('text ~ "rollback"', self.cql(query='"blue green" rollback'))

    def test_wildcard_title_and_junk(self):
        self.assertTrue(self.cql(query="deploy*", titleOnly=True).startswith('title ~ "deploy*"'))
        # lucene operators and leading wildcards never reach the server
        self.assertTrue(self.cql(query="*foo (bar) a:b").startswith(
            '(text ~ "foo" AND text ~ "bar" AND text ~ "ab")'))
        self.assertEqual(cc.parse_query('say "hi" AND x'), [("say", False), ("hi", False), ("x", False)])

    def test_escaping(self):
        self.assertEqual(cc.cql_str('a"b\\c'), '"a\\"b\\\\c"')

    def test_scope_clamp(self):
        c = cfg(spaces=[{"key": "ENG"}, {"key": "OPS"}])
        self.assertIn('space in ("ENG", "OPS")', cc.criteria_cql({"query": "x"}, c))
        self.assertIn('space in ("OPS")', cc.criteria_cql({"query": "x", "spaces": ["OPS", "HR"]}, c))
        # a picked space outside the scope falls back to the whole scope
        self.assertIn('space in ("ENG", "OPS")', cc.criteria_cql({"query": "x", "spaces": ["HR"]}, c))
        # no scope: the whole site
        self.assertNotIn("space", cc.criteria_cql({"query": "x"}, cfg()))

    def test_filters_and_sort(self):
        q = self.cql(query="x", modified="30d", mine=True, sort="recent", types=["page", "bogus"])
        self.assertIn('lastmodified >= now("-30d")', q)
        self.assertIn("contributor = currentUser()", q)
        self.assertIn("type in (page)", q)
        self.assertTrue(q.endswith("ORDER BY lastmodified DESC"))

    def test_contributors(self):
        self.assertIn("contributor = currentUser()", self.cql(query="x", contributors=["me"]))
        self.assertIn('(contributor = currentUser() OR contributor in ("acc-1", "bob"))',
                      self.cql(query="x", contributors=["me", "acc-1", "bob"]))
        self.assertIn('contributor in ("bob")', self.cql(query="x", contributors=["bob"]))
        # the old Mine toggle still means me; people alone make a query
        self.assertIn("contributor = currentUser()", self.cql(query="x", mine=True))
        self.assertTrue(self.cql(contributors=["bob"]).endswith("ORDER BY lastmodified DESC"))

    def test_empty(self):
        with self.assertRaises(cc.ConfigError):
            self.cql(query="  ")
        self.assertIn("ORDER BY lastmodified DESC", self.cql(modified="7d"))
        with self.assertRaises(cc.ConfigError):
            self.cql(query="x", ids=[])

    def test_site_normalized(self):
        self.assertEqual(cc.norm_site("acme.atlassian.net/"), "https://acme.atlassian.net/wiki")
        self.assertEqual(cc.norm_site("https://c.corp/confluence/"), "https://c.corp/confluence")

    def test_terms(self):
        self.assertEqual(cc.terms({"query": "deploy* green"}),
                         [{"text": "deploy", "phrase": False, "prefix": True},
                          {"text": "green", "phrase": False, "prefix": False}])
        self.assertEqual(cc.terms({"query": "blue green", "mode": "phrase"}),
                         [{"text": "blue green", "phrase": True, "prefix": False}])


class FakeTests(unittest.TestCase):
    """The fake itself answers the CQL criteria_cql emits the way the modes promise."""

    def count(self, **crit):
        return len(fake.search(cc.criteria_cql({"types": ["page", "blogpost"], **crit}, cfg()))[0])

    def test_modes_differ(self):
        allw = self.count(query="blue green deploy")
        phrase = self.count(query="blue green deploy", mode="phrase")
        anyw = self.count(query="blue green deploy", mode="any")
        self.assertTrue(0 < phrase < allw < anyw, (phrase, allw, anyw))

    def test_case_stem_prefix(self):
        self.assertEqual(self.count(query="DEPLOY"), self.count(query="deploy"))
        self.assertGreater(self.count(query="deploying"), 0)
        self.assertEqual(self.count(query="canar"), 0)
        self.assertGreater(self.count(query="canar*"), 0)

    def test_title_only_narrows(self):
        self.assertLess(self.count(query="deploy", titleOnly=True), self.count(query="deploy"))

    def test_parser_rejects_garbage(self):
        with self.assertRaises(fake.CQLError):
            fake.search('text ~ "a" AND AND')


class CLITests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp()
        bindir = os.path.join(self.tmp, "bin")
        os.makedirs(bindir)
        with open(os.path.join(bindir, "curl"), "w") as fh:
            fh.write(SHIM)
        os.chmod(os.path.join(bindir, "curl"), 0o755)
        self.config = os.path.join(self.tmp, "config.json")
        self.write({"site": "https://conf.example.com/wiki", "token": "fake-token", "auth": "bearer"})
        self.env = {**os.environ, "PATH": bindir + os.pathsep + os.environ["PATH"], "FAKE_DIR": self.tmp,
                    "CONFLUENCE_CONFIG_JSON": self.config, "CONFLUENCE_CACHE_DIR": os.path.join(self.tmp, "cache"),
                    "JIRA_CACHE_DIR": os.path.join(self.tmp, "jcache")}
        for k in ("CONFLUENCE_TOKEN", "CONFLUENCE_SITE", "FAKE_CONF_429", "FAKE_CONF_401", "FAKE_CONF_NO_SEARCH"):
            self.env.pop(k, None)

    def write(self, data):
        with open(self.config, "w") as fh:
            json.dump(data, fh)

    def read(self):
        with open(self.config) as fh:
            return json.load(fh)

    def run_api(self, *args, stdin=None, **env):
        p = subprocess.run([sys.executable, os.path.join(CONF, "confluence_api.py"), *args],
                           input=json.dumps(stdin) if stdin is not None else "", capture_output=True,
                           text=True, env={**self.env, **env}, timeout=60)
        try:
            return p.returncode, json.loads(p.stdout)
        except ValueError:
            self.fail(f"no JSON (exit {p.returncode}): {p.stdout!r} {p.stderr!r}")

    def test_search_highlights_and_paging(self):
        code, out = self.run_api("--search", stdin={"query": "deploy", "limit": 3})
        self.assertEqual(code, 0, out)
        self.assertEqual(len(out["results"]), 3)
        self.assertGreater(out["total"], 3)
        self.assertTrue(out["next"])
        r = out["results"][0]
        self.assertTrue(r["url"].startswith("https://conf.example.com/wiki/spaces/"))
        self.assertTrue(r["hits"])
        a, n = r["hits"][0]
        self.assertTrue(r["excerpt"][a:a + n].lower().startswith("deploy"))
        self.assertNotIn("@@@", r["excerpt"] + r["title"])
        self.assertIn("CONFLUENCE_TOKEN", out["curl"])
        code, more = self.run_api("--search", stdin={"query": "deploy", "next": out["next"]})
        self.assertEqual(code, 0, more)
        self.assertNotEqual(more["results"][0]["id"], r["id"])

    def test_scope_limits_results(self):
        self.write({**self.read(), "spaces": [{"key": "OPS", "name": "Operations"}]})
        code, out = self.run_api("--search", stdin={"query": "deploy", "limit": 50})
        self.assertEqual(code, 0, out)
        self.assertTrue(out["results"])
        self.assertEqual({r["space"] for r in out["results"]}, {"OPS"})

    def test_page(self):
        _, out = self.run_api("--search", stdin={"query": "runbook", "mode": "phrase", "titleOnly": True})
        pid = next(r["id"] for r in out["results"] if r["title"] == "Blue green deploy runbook")
        code, page = self.run_api("--page", pid)
        self.assertEqual(code, 0, page)
        self.assertIn("/download/attachments/", page["html"])
        self.assertEqual(page["path"], "Engineering Home")
        self.assertEqual(page["spaceName"], "Engineering")

    def test_retry_429(self):
        code, out = self.run_api("--search", stdin={"query": "deploy"}, FAKE_CONF_429="2")
        self.assertEqual(code, 0, out)

    def test_401_wording(self):
        code, out = self.run_api("--search", stdin={"query": "deploy"}, FAKE_CONF_401="1")
        self.assertEqual(code, 1)
        self.assertIn("Confluence view's Setup", out["error"])
        self.assertIn("$CONFLUENCE_TOKEN", out["curl"])

    def test_fallback_without_search_endpoint(self):
        code, out = self.run_api("--search", stdin={"query": "rollback"}, FAKE_CONF_NO_SEARCH="1")
        self.assertEqual(code, 0, out)
        self.assertTrue(out["fallback"])
        self.assertTrue(out["results"])

    def test_bad_input(self):
        code, out = self.run_api("--search", stdin={"query": ""})
        self.assertEqual(code, 2)
        self.assertIn("type something", out["error"])

    def test_not_set_up(self):
        self.write({})
        code, out = self.run_api("--search", stdin={"query": "x"})
        self.assertEqual(code, 2)
        self.assertTrue(out.get("setup"))

    def test_spaces(self):
        code, out = self.run_api("--add-space", "eng", "NOPE")
        self.assertEqual(code, 1)
        self.assertIn("NOPE", out["error"])
        self.assertEqual(self.read()["spaces"], [{"key": "ENG", "name": "Engineering"}])
        self.assertEqual(os.stat(self.config).st_mode & 0o777, 0o600)
        code, out = self.run_api("--remove-space", "ENG")
        self.assertEqual((code, self.read()["spaces"]), (0, []))

    def test_detect_auth(self):
        self.write({})
        code, out = self.run_api("--detect-auth", stdin={"site": "https://conf.example.com/wiki",
                                                         "token": "fake-token", "email": "me@x.com"})
        self.assertEqual(code, 0, out)
        self.assertEqual(out["user"], "Fake User")
        # a wrong token: DC answers "anonymous" 200 - that must not count
        code, out = self.run_api("--detect-auth", stdin={"site": "https://conf.example.com/wiki",
                                                         "token": "nope", "email": ""})
        self.assertEqual(code, 1, out)
        code, out = self.run_api("--save", stdin={"site": "conf.example.com/wiki", "token": "fake-token",
                                                  "auth": "bearer"})
        self.assertEqual(code, 0, out)
        self.assertEqual(self.read()["site"], "https://conf.example.com/wiki")
        self.assertNotIn("token", json.dumps(out).replace("hasToken", ""))

    def test_favorites(self):
        _, out = self.run_api("--search", stdin={"query": "handbook", "titleOnly": True})
        rows = out["results"][:2]
        ids = [r["id"] for r in rows]
        code, out = self.run_api("--favorite", "add", *ids, stdin=rows)
        self.assertEqual(code, 0, out)
        favs = self.read()["favorites"]
        self.assertEqual([f["id"] for f in favs], ids)
        self.assertEqual(favs[0]["title"], rows[0]["title"])
        # adding again is a no-op; the search marks them
        self.run_api("--favorite", "add", ids[0], stdin=rows)
        self.assertEqual(len(self.read()["favorites"]), 2)
        _, out = self.run_api("--search", stdin={"query": "handbook", "titleOnly": True})
        self.assertTrue(all(r["favorite"] for r in out["results"] if r["id"] in ids))
        # search within favorites
        _, out = self.run_api("--search", stdin={"query": "handbook", "favorites": True})
        self.assertTrue(set(r["id"] for r in out["results"]) <= set(ids))
        # a vanished page stays, flagged
        self.write({**self.read(), "favorites": self.read()["favorites"] + [{"id": "999999", "title": "Gone"}]})
        code, out = self.run_api("--favorites")
        self.assertEqual(code, 0, out)
        gone = [r for r in out["results"] if r["id"] == "999999"]
        self.assertTrue(gone and gone[0]["missing"])
        self.assertTrue(self.read()["favorites"][-1]["missing"])
        code, out = self.run_api("--favorite", "remove", "999999", ids[0])
        self.assertEqual([f["id"] for f in self.read()["favorites"]], ids[1:])

    def test_users_scan_cached_and_scoped(self):
        self.write({**self.read(), "spaces": [{"key": "HR", "name": "People & HR"}], "usersScanDelayMs": 0})
        code, out = self.run_api("--users")
        self.assertEqual(code, 0, out)
        self.assertFalse(out["fromCache"])
        names = {u["name"] for u in out["users"]}
        self.assertIn("Maria Rossi", names)
        self.assertTrue(all(u["id"] for u in out["users"]))
        # the ids work as contributor filters
        uid = next(u["id"] for u in out["users"] if u["name"] == "Maria Rossi")
        code, res = self.run_api("--search", stdin={"contributors": [uid]})
        self.assertEqual(code, 0, res)
        self.assertTrue(res["results"])
        n = len(self.calls())
        code, again = self.run_api("--users")
        self.assertTrue(again["fromCache"])
        self.assertEqual(len(self.calls()), n)          # no request
        # a scope change rebuilds
        self.write({**self.read(), "spaces": [{"key": "ENG", "name": "Engineering"}]})
        _, other = self.run_api("--users")
        self.assertFalse(other["fromCache"])

    def test_rate_limit_cooldown(self):
        # a Retry-After longer than rateLimitMaxWaitSeconds: give up, cool down
        self.write({**self.read(), "rateLimitMaxWaitSeconds": 5})
        code, out = self.run_api("--search", stdin={"query": "deploy"}, FAKE_CONF_429="9", FAKE_CONF_429_RA="90")
        self.assertEqual(code, 1, out)
        self.assertTrue(out["rateLimited"])
        self.assertEqual(out["retryIn"], 90)
        n = len(self.calls())
        # every call during the cooldown refuses WITHOUT a request
        for args in (["--search"], ["--page", "1002"]):
            code, out = self.run_api(*args, stdin={"query": "deploy"})
            self.assertTrue(out["rateLimited"], out)
            self.assertGreater(out["retryIn"], 80)
        code, out = self.run_api("--favorites")
        self.assertEqual(code, 0)
        self.assertFalse(out["refreshed"])
        self.assertEqual(len(self.calls()), n)
        # a short Retry-After is simply waited out
        os.unlink(os.path.join(self.tmp, "cache", "ratelimit.json"))
        os.unlink(os.path.join(self.tmp, "fakeconf-429"))
        code, out = self.run_api("--search", stdin={"query": "deploy"}, FAKE_CONF_429="1", FAKE_CONF_429_RA="1")
        self.assertEqual(code, 0, out)

    def calls(self):
        p = os.path.join(self.tmp, "calls.jsonl")
        if not os.path.exists(p):
            return []
        with open(p) as fh:
            return fh.read().splitlines()

    def test_import_saved(self):
        code, out = self.run_api("--import-saved")
        self.assertEqual(code, 0, out)
        self.assertEqual(out["added"], 4)
        code, out = self.run_api("--import-saved")
        self.assertEqual(out["added"], 0)
        self.assertEqual(len(self.read()["favorites"]), 4)


if __name__ == "__main__":
    unittest.main(verbosity=1)
