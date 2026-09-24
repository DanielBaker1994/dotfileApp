#!/usr/bin/env python3
"""Tests for the python jira poller (jira/jira_*.py).

  python3 Tests/test_jira_poll.py        (stdlib unittest; no network except
                                          the optional env-token login test)

Parity: the python transforms are compared against the EXACT jq programs the
old bash scripts ran (jira-api.sh sync / jira-poll.sh publish), so the window
json stays byte-for-byte compatible for every existing consumer.
"""
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
JIRA = os.path.join(ROOT, "jira")
sys.path.insert(0, JIRA)

import jira_api  # noqa: E402
import jira_config  # noqa: E402

HAVE_JQ = shutil.which("jq") is not None

# the legacy jira-api.sh sync entry transform, verbatim
LEGACY_ENTRY_JQ = r'''{
    key: .key,
    title: (.fields.summary // ""),
    status: (.fields.status.name // ""),
    assignee: ((.fields.assignee.name // .fields.assignee.displayName) // ""),
    release: ([.fields.fixVersions[]?.name] | join(", ")),
    releaseLabel: ([.fields.fixVersions[]?.name as $n | ($v[.fields.project.key][$n].date // "") as $d | if $d == "" then $n else "\($n) (\($d))" end] | join(", ")),
    releaseDate: ([.fields.fixVersions[]?.name as $n | ($v[.fields.project.key][$n].date // "") | select(. != "")] | join(", ")),
    releaseStatus: ([.fields.fixVersions[]?.name as $n | $v[.fields.project.key][$n] | select(. != null) | .released] | if length == 0 then "" elif any(.[]; .) then "Released" else "Upcoming" end),
    priority: (.fields.priority.name // ""),
    labels: ([.fields.labels[]?] | join(", ")),
    description: ((.fields.description | if type == "object" then ([.. | objects | select(has("text")) | .text] | join("\n")) else . end) // ""),
    updated: (.fields.updated // ""),
    reporter: ((.fields.reporter.name // .fields.reporter.displayName) // ""),
    project: (.fields.project.key // ""),
    comments: $c
}'''

# the legacy jira-poll.sh per-project + releases transforms, verbatim
LEGACY_PER_PROJECT_JQ = '[ .[] | select(.project == $p) ] | sort_by(.updated // "") | reverse | map({key,title,status,assignee,release,releaseLabel,releaseDate,releaseStatus,priority,labels,description,reporter,project})'
LEGACY_RELEASES_JQ = '''[.[] | {
    key: (.project + "-" + .name), title: .name,
    status: (if .released then "Released" else "Upcoming" end),
    assignee: "", release: .name,
    releaseLabel: (if (.releaseDate // "") != "" then (.name + " (" + .releaseDate + ")") else .name end),
    releaseDate: (.releaseDate // ""),
    releaseStatus: (if .released then "Released" else "Upcoming" end),
    priority: "", labels: "", description: "", reporter: "", project: .project
}] | sort_by(.releaseDate // "", .name) | reverse'''

ADF = {"type": "doc", "version": 1, "content": [
    {"type": "paragraph", "content": [{"type": "text", "text": "Line one"},
                                      {"type": "text", "text": " bold", "marks": [{"type": "strong"}]}]},
    {"type": "bulletList", "content": [{"type": "listItem", "content": [
        {"type": "paragraph", "content": [{"type": "text", "text": "nested item"}]}]}]}]}

RAW_ISSUES = [
    {"key": "SAM1-1", "fields": {
        "summary": "First", "status": {"name": "To Do"},
        "assignee": {"displayName": "Ann Example"}, "reporter": {"name": "rep", "displayName": "R"},
        "fixVersions": [{"name": "13.1"}, {"name": "14.0"}, {"name": "ghost"}],
        "priority": {"name": "High"}, "labels": ["a", "b"], "description": ADF,
        "updated": "2026-09-20T10:00:00.000+0000", "project": {"key": "SAM1"}}},
    {"key": "SAM1-2", "fields": {
        "summary": None, "status": None, "assignee": None, "reporter": None,
        "fixVersions": [], "priority": None, "labels": [], "description": None,
        "updated": "2026-09-21T10:00:00.000+0000", "project": {"key": "SAM1"}}},
    {"key": "KAN-7", "fields": {
        "summary": "Plain desc", "status": {"name": "Done"}, "assignee": {"name": ""},
        "fixVersions": [{"name": "k1"}], "priority": {"name": "Low"}, "labels": None,
        "description": "plain text body", "updated": "2026-09-19T10:00:00.000+0000",
        "project": {"key": "KAN"}}},
]
VERS = {"SAM1": {"13.1": {"released": True, "date": "2026-10-15"},
                 "14.0": {"released": False, "date": ""}},
        "KAN": {"k1": {"released": False, "date": "2026-12-01"}}}
COMMENTS = [{"author": "A", "body": "hi", "created": "c", "updated": "u"}]


def jq(program, data, *args):
    p = subprocess.run(["jq", "-c", *args, program], input=json.dumps(data),
                       capture_output=True, text=True, check=True)
    return json.loads(p.stdout)


@unittest.skipUnless(HAVE_JQ, "jq not installed")
class ParityTests(unittest.TestCase):
    def test_cache_entry_matches_legacy_jq(self):
        fields = jira_config.api_fields({})   # empty section -> legacy base
        for raw in RAW_ISSUES:
            want = jq(LEGACY_ENTRY_JQ, raw, "--argjson", "v", json.dumps(VERS),
                      "--argjson", "c", json.dumps(COMMENTS))
            got = jira_api.cache_entry(raw, VERS, COMMENTS, fields)
            self.assertEqual(got, want, raw["key"])

    def test_per_project_publish_matches_legacy(self):
        import jira_poll
        fields = jira_config.api_fields({})
        cache = {r["key"]: jira_api.cache_entry(r, VERS, COMMENTS, fields) for r in RAW_ISSUES}
        for proj in ("SAM1", "KAN"):
            want = jq(LEGACY_PER_PROJECT_JQ, cache, "--arg", "p", proj)
            ep = {"name": proj, "projects": [proj]}
            got = jira_poll.shape(jira_poll.issues_for(ep, cache), jira_config.BASE_WINDOW_KEYS)
            self.assertEqual(got, want, proj)

    def test_all_publish_same_rows_as_legacy(self):
        # the legacy all.json sorted AFTER dropping `updated` (a no-op sort);
        # python sorts by updated properly, so compare the row set
        import jira_poll
        fields = jira_config.api_fields({})
        cache = {r["key"]: jira_api.cache_entry(r, VERS, COMMENTS, fields) for r in RAW_ISSUES}
        want = jq('[ .[] | {key,title,status,assignee,release,releaseLabel,releaseDate,'
                  'releaseStatus,priority,labels,description,reporter,project} ]', cache)
        got = jira_poll.shape(jira_poll.issues_for({"name": "all", "projects": "*"}, cache),
                              jira_config.BASE_WINDOW_KEYS)
        key = lambda r: r["key"]  # noqa: E731
        self.assertEqual(sorted(got, key=key), sorted(want, key=key))
        self.assertEqual([r["key"] for r in got], ["SAM1-2", "SAM1-1", "KAN-7"])

    def test_releases_publish_matches_legacy(self):
        import jira_poll
        rels = [{"project": "SAM1", "name": "13.1", "released": True, "releaseDate": "2026-10-15"},
                {"project": "SAM1", "name": "seed", "released": False, "releaseDate": ""},
                {"project": "KAN", "name": "k1", "released": False, "releaseDate": "2026-12-01"}]
        self.assertEqual(jira_poll.release_items(rels), jq(LEGACY_RELEASES_JQ, rels))

    def test_real_cache_per_project_parity(self):
        """Same window -> same published json as today, on the live cache."""
        import jira_poll
        cache = jira_api.read_json(jira_api.CACHE_FILE, None)
        if not cache:
            self.skipTest("no live cache")
        for proj in sorted({v.get("project") for v in cache.values()} - {None, ""}):
            want = jq(LEGACY_PER_PROJECT_JQ, cache, "--arg", "p", proj)
            got = jira_poll.shape(jira_poll.issues_for({"name": proj, "projects": [proj]}, cache),
                                  jira_config.BASE_WINDOW_KEYS)
            self.assertEqual([r["key"] for r in got if r["key"]],
                             [r["key"] for r in want if r["key"]], proj)
            self.assertEqual({r["key"]: r for r in got}, {r["key"]: r for r in want}, proj)


class ConfigTests(unittest.TestCase):
    def test_columns_drive_api_fields(self):
        sec = {"columns": "key:Key:6:left:filter, title:Title:40, created:Created:10:right:sort, "
                          "releaseLabel:Release:14"}
        self.assertEqual(jira_config.api_fields(sec),
                         ["summary", "created", "fixVersions", "updated", "project"])
        cols = jira_config.parse_columns(sec["columns"])
        self.assertEqual([c["field"] for c in cols], ["key", "title", "created", "releaseLabel"])
        self.assertTrue(cols[0]["filterable"] and not cols[0]["sortable"])
        self.assertTrue(cols[2]["sortable"] and cols[2]["align"] == "right")
        self.assertIn("created", jira_config.publish_keys(sec))

    def test_flags_combined(self):
        c = jira_config.parse_columns("status:Status:10:left:filter+sort")[0]
        self.assertTrue(c["filterable"] and c["sortable"])

    def test_window_parse(self):
        self.assertEqual(jira_config.parse_window("10m"), 600)
        self.assertEqual(jira_config.parse_window("1h"), 3600)
        with self.assertRaises(jira_config.ConfigError):
            jira_config.parse_window("30s")
        with self.assertRaises(jira_config.ConfigError):
            jira_config.parse_window("soon")

    def test_legacy_migration(self):
        with tempfile.TemporaryDirectory() as d:
            p = os.path.join(d, "config")
            with open(p, "w") as fh:
                fh.write("JIRA_SITE='https://x.atlassian.net'\nJIRA_EMAIL='e@x'\n"
                         "JIRA_TOKEN='tok'\nJIRA_MAX='40'\nJIRA_POLL_PROJECTS='A,B'\n")
            cfg = jira_config.migrate_legacy(jira_config.parse_legacy(p))
            self.assertEqual(cfg["site"], "https://x.atlassian.net")
            self.assertEqual(cfg["defaultMax"], 40)
            self.assertEqual([e["name"] for e in cfg["endpoints"]], ["all", "A", "B", "releases"])


def isolated_env(tmp, token=""):
    """Env pointing every path at `tmp` (never touches the real config)."""
    conf = os.path.join(tmp, "commands.conf")
    with open(conf, "w") as fh:
        fh.write("[jira]\nenabled = true\ntype = list\n")
    cfgj = os.path.join(tmp, "config.json")
    with open(cfgj, "w") as fh:
        json.dump({"site": "https://sudosignup.atlassian.net", "email": "sudosignup@proton.me",
                   "token": token, "endpoints": [
                       {"name": "all", "window": "10m", "projects": "*", "type": "issues",
                        "file": "all.json", "enabled": True}]}, fh)
    env = dict(os.environ)
    env.update({"WS_COMMANDS_CONF": conf, "JIRA_CONFIG_JSON": cfgj,
                "JIRA_CONFIG_FILE": os.path.join(tmp, "nolegacy"),
                "JIRA_CACHE_DIR": os.path.join(tmp, "cache")})
    env.pop("JIRA_TOKEN", None)
    return env


class PollTests(unittest.TestCase):
    def test_second_invocation_skips_on_lock(self):
        import fcntl
        with tempfile.TemporaryDirectory() as tmp:
            env = isolated_env(tmp, token="x")
            os.makedirs(os.path.join(tmp, "cache"))
            lockp = os.path.join(tmp, "cache", "poll.lock")
            with open(lockp, "w") as fh:
                fcntl.flock(fh, fcntl.LOCK_EX | fcntl.LOCK_NB)
                fh.write(json.dumps({"pid": os.getpid(), "since": "2099-01-01 00:00:00"}))
                fh.flush()
                p = subprocess.run([sys.executable, os.path.join(JIRA, "jira_poll.py"), "--quiet"],
                                   env=env, capture_output=True, text=True, timeout=30)
            self.assertEqual(p.returncode, 3, p.stderr)
            with open(os.path.join(tmp, "cache", "status.json")) as fh:
                st = json.load(fh)
            self.assertEqual(st["lastSkipped"]["reason"], "skipped-locked")
            self.assertTrue(st["lock"]["held"])

    def test_disabled_is_noop_with_status(self):
        with tempfile.TemporaryDirectory() as tmp:
            env = isolated_env(tmp, token="x")
            with open(env["WS_COMMANDS_CONF"], "w") as fh:
                fh.write("[jira]\nenabled = false\n")
            p = subprocess.run([sys.executable, os.path.join(JIRA, "jira_poll.py")],
                               env=env, capture_output=True, text=True, timeout=30)
            self.assertEqual(p.returncode, 0)
            with open(os.path.join(tmp, "cache", "status.json")) as fh:
                st = json.load(fh)
            self.assertEqual(st["status"], "disabled")
            self.assertFalse(st["enabled"])
            self.assertFalse(os.path.exists(os.path.join(tmp, "cache", "curl.log")))

    @unittest.skipUnless(os.environ.get("TEST_TOKEN_NOT_REAL"), "TEST_TOKEN_NOT_REAL not exported")
    def test_env_token_override_is_visible_and_logs_in(self):
        with tempfile.TemporaryDirectory() as tmp:
            env = isolated_env(tmp, token="")
            env["JIRA_TOKEN"] = env["TEST_TOKEN_NOT_REAL"]
            p = subprocess.run([sys.executable, os.path.join(JIRA, "jira_config.py"), "--check"],
                               env=env, capture_output=True, text=True, timeout=30)
            chk = json.loads(p.stdout)
            self.assertIn("using env JIRA_TOKEN (config empty)", chk["notes"])
            p = subprocess.run([sys.executable, os.path.join(JIRA, "jira_api.py"), "--myself"],
                               env=env, capture_output=True, text=True, timeout=60)
            self.assertEqual(p.returncode, 0, p.stderr)
            self.assertIn("using env JIRA_TOKEN (config empty)", p.stderr)
            self.assertIn("displayName", p.stdout)
            log = os.path.join(tmp, "cache", "curl.log")
            self.assertEqual(os.stat(log).st_mode & 0o777, 0o600)
            with open(log) as fh:
                self.assertIn(" 200  curl ", fh.read())


if __name__ == "__main__":
    unittest.main(verbosity=2)
