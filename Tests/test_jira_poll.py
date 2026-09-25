#!/usr/bin/env python3
"""Tests for the python jira poller (jira/jira_*.py).

  python3 Tests/test_jira_poll.py        (stdlib unittest; no network except
                                          the optional env-token login test)

Parity: the python transforms are compared against the EXACT jq programs the
old bash scripts ran (jira-api.sh sync / jira-poll.sh publish), so the window
json stays byte-for-byte compatible for every existing consumer.
"""
import json
import urllib.parse
import os
import re
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


FAKE_CURL = r'''#!/usr/bin/env python3
# fake curl: logs argv, serves a 5-issue v2 /search in startAt pages
import json, os, sys, urllib.parse
with open(os.environ["FAKE_CURL_LOG"], "a") as fh:
    fh.write(json.dumps(sys.argv[1:]) + "\n")
url = sys.argv[-1]
q = urllib.parse.parse_qs(urllib.parse.urlparse(url).query)
path = urllib.parse.urlparse(url).path
if url.endswith("/myself"):
    body = {"displayName": "Fake User", "name": "fake"}
elif path.endswith("/project"):
    body = [{"key": "P1", "name": "Proj One"}, {"key": "P2", "name": "Proj Two"}]
elif "/project/" in path and not path.endswith("/versions"):
    k = path.rsplit("/", 1)[-1]
    body = {"key": k, "name": {"P1": "Proj One", "P2": "Proj Two"}.get(k, k)}
elif path.endswith("/user/assignable/search"):
    # 3 users per project, served in pages; "bob" is in both projects
    proj, start, n = q["project"][0], int(q.get("startAt", ["0"])[0]), int(q["maxResults"][0])
    us = [{"name": f"{proj.lower()}u{i}", "displayName": f"{proj} User {i}"} for i in range(2)]
    us.append({"name": "bob", "displayName": "Bob", "emailAddress": "bob@x"})
    body = us[start:start + n]
elif path.endswith("/status") or path.endswith("/priority") or path.endswith("/issuetype"):
    body = [{"name": "B"}, {"name": "a"}, {"name": "B"}]
elif path.endswith("/versions"):
    body = [{"name": "1.0", "released": True, "releaseDate": "2026-01-01"},
            {"name": "2.0", "released": False, "releaseDate": "2026-12-01"},
            {"name": "old", "archived": True}]
elif path.endswith("/field"):
    body = [{"id": "summary", "name": "Summary", "custom": False, "schema": {"type": "string"}},
            {"id": "customfield_20214", "name": "Package", "custom": True,
             "schema": {"type": "option"}}]
elif q.get("fields") == ["labels"]:
    proj = "P1" if '"P1"' in q["jql"][0] else "P2"
    body = {"startAt": 0, "total": 2, "issues": [
        {"key": f"{proj}-1", "fields": {"labels": ["shared", proj.lower()]}},
        {"key": f"{proj}-2", "fields": {"labels": []}}]}
else:
    start, n = int(q.get("startAt", ["0"])[0]), int(q["maxResults"][0])
    body = {"startAt": start, "total": 5, "issues": [
        {"key": f"P-{i}", "fields": {"summary": f"s{i}", "customfield_20214": {"value": "pkg"}}}
        for i in range(start, min(start + n, 5))]}
sys.stdout.write(json.dumps(body) + "\n200")
'''

MESSY_TEAM = {
    "custom_fields": {
        "package_info": {"field id": "customfield 20214", "Label": "Package Information"},
        "itrs_info": {"Field_id": "customfield_15262", "label": "ITRS Information"},
    },
    "field mappings": {"customfield 10086": "Acceptance Criteria"},
    "project_keys": ["P1", "P2"],
    "jobs": [{"key": "partial_search", "name": "Partial Search",
              "args": [{"name": "query", "Label": "Search Text", "type": "text"}]}],
    "api_endpoints": {"board_issues": "/board/{board_id}/issue"},
    "boards": [{"id": 9148, "name": "TEAM", "type": "scrum"}],
    "search defaults": {"max_results_search": 2, "timeout_seconds": 7},
}


class BearerAndTeamTests(unittest.TestCase):
    def client(self, **kw):
        team = jira_config.load_team({})
        team.update(jira_config.load_team({**MESSY_TEAM, "teamConfig": "/nonexistent"}))
        kw.setdefault("auth", "bearer")
        return jira_api.Client("https://jira.example.com", "TOK", team=team, **kw)

    def test_bearer_curl_matches_sample(self):
        c = self.client()
        self.assertEqual(
            c.curl_cmd("https://jira.example.com/rest/api/2/myself"),
            "curl -X GET -H 'Authorization: Bearer TOK' -H 'Accept: application/json' "
            "'https://jira.example.com/rest/api/2/myself'")
        self.assertIn('"Authorization: Bearer $JIRA_TOKEN"',
                      c.curl_cmd("https://x/rest/api/2/myself", masked=True))
        self.assertNotIn("-u", c.curl_argv("https://x"))

    def test_auth_mode_derivation(self):
        mk = lambda d: jira_config.Config(d, "", [], "test")
        self.assertEqual(mk({"site": "https://j", "token": "t"}).auth, "bearer")
        self.assertEqual(mk({"email": "a@b", "token": "t"}).auth, "basic")
        self.assertEqual(mk({"email": "a@b", "auth": "bearer"}).auth, "bearer")
        # bearer needs no email
        self.assertEqual(mk({"site": "https://j", "token": "t"}).problems(), [])
        self.assertTrue(any("email" in p for p in mk({"site": "https://j", "token": "t",
                                                       "auth": "basic"}).problems()))

    def test_messy_team_keys_normalize(self):
        t = jira_config.load_team({**MESSY_TEAM, "teamConfig": "/nonexistent"})
        al = jira_config.custom_field_aliases(t)
        self.assertEqual(al["package_info"]["id"], "customfield_20214")
        self.assertEqual(al["package_info"]["label"], "Package Information")
        self.assertEqual(al["customfield_10086"]["label"], "Acceptance Criteria")
        self.assertEqual(t["search_defaults"]["timeout_seconds"], 7)
        self.assertEqual(jira_config.team_problems(t), [])

    def test_alias_columns_drive_api_fields_and_cache(self):
        t = jira_config.load_team({**MESSY_TEAM, "teamConfig": "/nonexistent"})
        sec = {"columns": "key:Key:80, package_info:Package:120, title:Title"}
        fields = jira_config.api_fields(sec, team=t)
        self.assertIn("customfield_20214", fields)
        self.assertNotIn("package_info", fields)
        e = jira_api.cache_entry({"key": "P-1", "fields": {"customfield_20214": {"value": "pkg"}}},
                                 {}, None, fields, jira_config.custom_field_aliases(t))
        self.assertEqual(e["package_info"], "pkg")

    def test_paths_and_templates(self):
        t = jira_config.load_team({**MESSY_TEAM, "teamConfig": "/nonexistent"})
        self.assertEqual(jira_config.resolve_path(t, "board_issues", board_id=9148),
                         "/rest/agile/1.0/board/9148/issue")
        self.assertEqual(jira_config.resolve_path(t, "search", "https://jira.example.com"),
                         "/rest/api/2/search")
        self.assertEqual(jira_config.resolve_path(t, "search", "https://x.atlassian.net"),
                         "/rest/api/3/search/jql")
        t2 = jira_config.load_team({"api_endpoints": {"search": "/search"}, "teamConfig": "/x"})
        self.assertEqual(jira_config.resolve_path(t2, "search", "https://x.atlassian.net"),
                         "/rest/api/2/search")   # explicit wins
        self.assertEqual(jira_config.job_jql(t, "partial_search", {"query": 'a "b"'}),
                         'project in ("P1", "P2") AND (summary ~ "a \\"b\\"" OR description ~ '
                         '"a \\"b\\"") ORDER BY updated DESC')
        with self.assertRaises(jira_config.ConfigError):
            jira_config.job_jql(t, "partial_search", {})
        self.assertEqual(jira_config.endpoint_jql({"job": "users_search"}, t), 'project in ("P1", "P2")')

    def test_v2_search_paginates_with_startat_via_curl(self):
        with tempfile.TemporaryDirectory() as tmp:
            fake = os.path.join(tmp, "curl")
            with open(fake, "w") as fh:
                fh.write(FAKE_CURL)
            os.chmod(fake, 0o755)
            log = os.path.join(tmp, "calls.jsonl")
            old = (os.environ.get("PATH", ""), jira_api.CURL_LOG)
            os.environ["PATH"] = tmp + os.pathsep + old[0]
            os.environ["FAKE_CURL_LOG"] = log
            jira_api.CURL_LOG = os.path.join(tmp, "curl.log")
            try:
                res = self.client().search("project = P", "summary")
            finally:
                os.environ["PATH"], jira_api.CURL_LOG = old
            self.assertEqual([i["key"] for i in res["issues"]], [f"P-{i}" for i in range(5)])
            with open(log) as fh:
                calls = [json.loads(x) for x in fh]
            self.assertEqual(len(calls), 3)   # page size 2 -> 2+2+1
            self.assertIn("Authorization: Bearer TOK", calls[0])
            self.assertIn("120", calls[0])    # search pages: timeout_search_seconds -> -m 120
            self.assertTrue(all("/rest/api/2/search?" in c[-1] for c in calls))
            self.assertIn("startAt=4", calls[2][-1])

    def test_detect_auth_tries_the_likely_mode_first(self):
        fake = r'''#!/usr/bin/env python3
import json, os, sys
with open(os.environ["FAKE_CURL_LOG"], "a") as fh:
    fh.write(json.dumps(sys.argv[1:]) + "\n")
a = " ".join(sys.argv[1:])
ok = ("Authorization: Bearer" in a) if os.environ["FAKE_AUTH"] == "bearer" else (" -u " in " " + a)
sys.stdout.write((json.dumps({"displayName": "Dee"}) if ok else "{}") + "\n" + ("200" if ok else "401"))
'''
        cases = [("https://jira.example.com", "", "bearer", "bearer", ["bearer"]),
                 ("https://jira.example.com", "me@x", "basic", "basic", ["bearer", "basic"]),
                 ("https://x.atlassian.net", "me@x", "basic", "basic", ["basic"]),
                 ("https://x.atlassian.net", "", "basic", None, ["bearer"])]
        for site, email, server, want, tried in cases:
            with tempfile.TemporaryDirectory() as tmp:
                env = isolated_env(tmp, token="")
                with open(os.path.join(tmp, "curl"), "w") as fh:
                    fh.write(fake)
                os.chmod(os.path.join(tmp, "curl"), 0o755)
                env.update({"PATH": tmp + os.pathsep + env["PATH"], "FAKE_AUTH": server,
                            "FAKE_CURL_LOG": os.path.join(tmp, "calls.jsonl")})
                p = subprocess.run([sys.executable, os.path.join(JIRA, "jira_api.py"), "--detect-auth",
                                    "--site", site, "--email", email, "--token-stdin"],
                                   env=env, input="TOK\n", capture_output=True, text=True, timeout=30)
                r = json.loads(p.stdout)
                self.assertEqual(r.get("auth"), want, (site, email, r))
                self.assertEqual([t["mode"] for t in r["tried"]], tried, (site, email))
                if want:
                    self.assertEqual(r["user"], "Dee")
                    self.assertEqual(p.returncode, 0)
                else:
                    self.assertIn("needs the account email", r["error"])

    def test_curl_flag_prints_without_running(self):
        with tempfile.TemporaryDirectory() as tmp:
            env = isolated_env(tmp, token="SECRET")
            env["PATH"] = tmp   # no curl at all: --curl must not execute
            p = subprocess.run([sys.executable, os.path.join(JIRA, "jira_api.py"), "--curl",
                                "--mask", "--auth", "bearer", "--site", "https://jira.example.com"],
                               env=env, capture_output=True, text=True, timeout=30)
            self.assertEqual(p.returncode, 0, p.stderr)
            self.assertEqual(p.stdout.strip(),
                             "curl -X GET -H \"Authorization: Bearer $JIRA_TOKEN\" -H 'Accept: "
                             "application/json' 'https://jira.example.com/rest/api/2/myself'")
            self.assertNotIn("SECRET", p.stdout)


class JobsAndLiveSearchTests(unittest.TestCase):
    COLS = "key:Key:10:left:filter+sort, title:Title:0:left:filter"

    def env(self, tmp, **cfg_extra):
        env = isolated_env(tmp, token="T")
        with open(env["WS_COMMANDS_CONF"], "a") as fh:
            fh.write(f"columns = {self.COLS}\n")
        with open(env["JIRA_CONFIG_JSON"]) as fh:
            cfg = json.load(fh)
        cfg.update({"site": "https://jira.example.com", "email": "", "auth": "bearer",
                    "outDir": os.path.join(tmp, "out"), "fetchComments": False, **cfg_extra})
        with open(env["JIRA_CONFIG_JSON"], "w") as fh:
            json.dump(cfg, fh)
        return env

    def cfgjson(self, env):
        with open(env["JIRA_CONFIG_JSON"]) as fh:
            return json.load(fh)

    def run_py(self, env, script, *args, stdin=None):
        return subprocess.run([sys.executable, os.path.join(JIRA, script), *args], env=env,
                              input=stdin, capture_output=True, text=True, timeout=60)

    def test_migration_copies_columns_once(self):
        with tempfile.TemporaryDirectory() as tmp:
            env = self.env(tmp)
            p = self.run_py(env, "jira_config.py", "--check")
            # all + the favorites job the upgrade adds
            self.assertIn("gave 2 job(s) their own copy of [jira] columns", p.stdout)
            self.assertEqual(self.cfgjson(env)["endpoints"][0]["columns"], self.COLS)
            p = self.run_py(env, "jira_config.py", "--check")
            self.assertNotIn("own copy", p.stdout)   # once only

    def test_upsert_validates_and_delete(self):
        with tempfile.TemporaryDirectory() as tmp:
            env = self.env(tmp)
            up = lambda o, kind="endpoint": json.loads(self.run_py(  # noqa: E731
                env, "jira_config.py", f"--upsert-{kind}", stdin=json.dumps(o)).stdout)
            self.assertFalse(up({"name": "bad name!"})["ok"])
            self.assertFalse(up({"name": "x", "window": "10s"})["ok"])
            self.assertFalse(up({"name": "x", "columns": "key:Key:abc"})["ok"])
            self.assertFalse(up({"name": "x", "file": "all.json"})["ok"])     # file clash
            r = up({"name": "mine", "window": "15m", "projects": ["P"], "columns": "key:K, labels:L"})
            self.assertTrue(r["ok"], r)
            ep = [e for e in self.cfgjson(env)["endpoints"] if e["name"] == "mine"][0]
            self.assertEqual((ep["file"], ep["type"], ep["enabled"]), ("mine.json", "issues", True))
            self.assertFalse(up({"name": "x", "maxResults": 5000})["ok"])      # page size 1-1000
            self.assertFalse(up({"name": "x", "file": "search.json"})["ok"])   # the live tab
            r = up({"name": "mine", "maxResults": 25, "maxTotal": 0})
            ep = [e for e in self.cfgjson(env)["endpoints"] if e["name"] == "mine"][0]
            self.assertEqual(ep["maxResults"], 25)
            self.assertNotIn("maxTotal", ep)        # 0 = no cap = key dropped
            r = json.loads(self.run_py(env, "jira_config.py", "--set-columns", "endpoint", "mine",
                                       "key:Key:50").stdout)
            self.assertTrue(r["ok"])
            self.assertTrue(json.loads(self.run_py(env, "jira_config.py", "--delete-endpoint",
                                                   "mine").stdout)["ok"])
            last = json.loads(self.run_py(env, "jira_config.py", "--delete-endpoint", "all").stdout)
            self.assertFalse(last["ok"])   # never delete the last job (directory doesn't count)

    def test_per_job_api_fields_differ(self):
        a = jira_config.api_fields({}, {}, columns="key:K, labels:L")
        b = jira_config.api_fields({}, {}, columns="key:K, duedate:Due")
        self.assertIn("labels", a)
        self.assertNotIn("labels", b)
        self.assertIn("duedate", b)
        self.assertIn("duedate", jira_config.publish_keys({}, columns="key:K, duedate:Due"))

    def test_criteria_jql(self):
        t = jira_config.load_team({**MESSY_TEAM, "teamConfig": "/nonexistent"})   # scope P1, P2
        cj = jira_config.criteria_jql
        S = 'project in ("P1", "P2") AND '    # no project picked -> the whole scope, never the site
        self.assertEqual(cj({"assignee": ["ann", "bob"], "projects": ["P1"]}, t),
                         'project = "P1" AND assignee in ("ann", "bob") ORDER BY updated DESC')
        self.assertEqual(cj({"text": 'a "b"', "reporter": ["currentUser()"], "updated": "7d"}, t),
                         S + 'reporter = currentUser() AND text ~ "a \\"b\\"" AND updated >= -7d '
                         "ORDER BY updated DESC")
        self.assertEqual(cj({"created": "today", "projects": "*"}, t),
                         S + "created >= startOfDay() ORDER BY updated DESC")
        # a picked project outside the scope is dropped
        self.assertEqual(cj({"created": "today", "projects": ["ELSEWHERE"]}, t),
                         S + "created >= startOfDay() ORDER BY updated DESC")
        # custom alias -> cf[]; text type (or unknown) uses ~, options use =
        self.assertEqual(cj({"fields": {"package_info": "pkg"}}, t),
                         S + 'cf[20214] ~ "pkg" ORDER BY updated DESC')
        self.assertEqual(cj({"fields": {"package_info": "pkg"}}, t, {"customfield_20214": "option"}),
                         S + 'cf[20214] = "pkg" ORDER BY updated DESC')
        self.assertEqual(cj({"fields": {"title": "x"}, "jql": "a = 1 ORDER BY created"}, t),
                         S + 'summary ~ "x" AND (a = 1) ORDER BY updated DESC')
        # labels / releases come from the pickers as lists
        self.assertEqual(cj({"labels": ["a b", "c"], "fixVersion": ["1.0"]}, t),
                         S + 'labels in ("a b", "c") AND fixVersion = "1.0" ORDER BY updated DESC')
        for bad in ({}, {"projects": []}, {"updated": "soon"}):
            with self.assertRaises(jira_config.ConfigError):
                cj(bad, t)
        with self.assertRaises(jira_config.ConfigError):     # no scope -> no search at all
            cj({"text": "x"}, {**t, "project_keys": []})

    def fake_curl(self, tmp, env):
        fake = os.path.join(tmp, "curl")
        with open(fake, "w") as fh:
            fh.write(FAKE_CURL)
        os.chmod(fake, 0o755)
        env["PATH"] = tmp + os.pathsep + env["PATH"]
        env["FAKE_CURL_LOG"] = os.path.join(tmp, "calls.jsonl")

    def urls(self, env):
        with open(env["FAKE_CURL_LOG"]) as fh:
            return [json.loads(x)[-1] for x in fh]

    def test_live_search_publishes_search_tab(self):
        with tempfile.TemporaryDirectory() as tmp:
            env = self.env(tmp, liveSearch={"columns": "key:Key, title:Title, customfield_20214:Pkg",
                                            "maxResults": 3})
            self.fake_curl(tmp, env)
            crit = json.dumps({"text": "pkg", "projects": ["P"]})
            p = self.run_py(env, "jira_poll.py", "--live-search", "--dry-run", stdin=crit)
            dry = json.loads(p.stdout)
            self.assertEqual(dry["jql"], 'project = "P" AND text ~ "pkg" ORDER BY updated DESC')
            self.assertIn("maxResults=3", dry["curl"])
            self.assertFalse(os.path.exists(env["FAKE_CURL_LOG"]))   # dry: no request
            p = self.run_py(env, "jira_poll.py", "--live-search", stdin=crit)
            r = json.loads(p.stdout)
            self.assertTrue(r["ok"], p.stdout + p.stderr)
            self.assertEqual((r["count"], r["total"], r["more"]), (3, 5, True))
            with open(os.path.join(tmp, "out", "search.json")) as fh:
                items = json.load(fh)
            self.assertEqual([i["key"] for i in items], ["P-0", "P-1", "P-2"])
            self.assertEqual(items[0]["customfield_20214"], "pkg")
            self.assertTrue(all("customfield_20214" in u for u in self.urls(env)))
            self.assertFalse(os.path.exists(os.path.join(tmp, "cache", "jiras.json")))  # no merge
            bad = json.loads(self.run_py(env, "jira_poll.py", "--live-search", stdin="{}").stdout)
            self.assertFalse(bad["ok"])
            self.assertIn("at least one criterion", bad["error"])
            d = json.loads(self.run_py(env, "jira_poll.py", "--describe").stdout)
            self.assertEqual(d["liveSearch"]["maxResults"], 3)
            cat = {c["field"]: c for c in d["catalog"]}
            self.assertIn("live:search", cat["customfield_20214"]["usedBy"])
            r = json.loads(self.run_py(env, "jira_config.py", "--set-columns", "live", "search",
                                       "key:K").stdout)
            self.assertTrue(r["ok"])
            self.assertEqual(self.cfgjson(env)["liveSearch"]["columns"], "key:K")

    def test_field_labels_one_per_field(self):
        with tempfile.TemporaryDirectory() as tmp:
            env = self.env(tmp, endpoints=[
                {"name": "all", "window": "10m", "projects": "*", "type": "issues", "file": "all.json",
                 "columns": "key:Key:8, title:Summary:40:left:sort, status:State"},
                {"name": "B", "window": "10m", "projects": ["B"], "type": "issues", "file": "B.json",
                 "columns": "key:Issue, title:Summary, status:Stage"}],
                liveSearch={"columns": "key:Key, status:State:10"})
            p = self.run_py(env, "jira_config.py", "--check")
            self.assertIn("column titles -> team.json field_labels", p.stdout)
            with open(env["JIRA_TEAM_JSON"]) as fh:
                labels = json.load(fh)["field_labels"]
            # most used title wins; titles equal to the default are not pinned
            self.assertEqual(labels, {"title": "Summary", "status": "State"})
            cfg = self.cfgjson(env)
            self.assertEqual(cfg["endpoints"][0]["columns"], "key::8, title::40:left:sort, status")
            self.assertEqual(cfg["liveSearch"]["columns"], "key, status::10")
            self.assertTrue(cfg["fieldLabels"])
            p = self.run_py(env, "jira_config.py", "--check")
            self.assertNotIn("field_labels", p.stdout)             # once only
            d = json.loads(self.run_py(env, "jira_poll.py", "--describe").stdout)
            cat = {c["field"]: c for c in d["catalog"]}
            self.assertEqual((cat["status"]["label"], cat["status"]["defaultLabel"], cat["status"]["renamed"]),
                             ("State", "Status", True))
            self.assertEqual((cat["key"]["label"], cat["key"]["renamed"], cat["key"]["base"]), ("Key", False, True))
            r = json.loads(self.run_py(env, "jira_config.py", "--team-set", "field_labels",
                                       stdin='{"status": ""}').stdout)
            self.assertFalse(r["ok"])
            r = json.loads(self.run_py(env, "jira_config.py", "--team-set", "field_labels",
                                       stdin='{"status": "Workflow"}').stdout)
            self.assertTrue(r["ok"], r)
            d = json.loads(self.run_py(env, "jira_poll.py", "--describe").stdout)
            self.assertEqual({c["field"]: c["label"] for c in d["catalog"]}["status"], "Workflow")

    def test_directory_job_caches_users(self):
        with tempfile.TemporaryDirectory() as tmp:
            env = self.env(tmp)
            self.fake_curl(tmp, env)
            with open(env["JIRA_TEAM_JSON"], "w") as fh:
                json.dump({"project_keys": ["P1", "P2"], "search_defaults": {"max_results_users": 2}}, fh)
            p = self.run_py(env, "jira_poll.py", "--directory")
            self.assertEqual(p.returncode, 0, p.stderr)
            with open(os.path.join(tmp, "cache", "directory.json")) as fh:
                d = json.load(fh)
            self.assertEqual([x["key"] for x in d["projects"]], ["P1", "P2"])
            ids = [u["id"] for u in d["users"]]
            self.assertEqual(len(ids), 5)                 # bob merged across projects
            bob = [u for u in d["users"] if u["id"] == "bob"][0]
            self.assertEqual((bob["projects"], bob["email"]), (["P1", "P2"], "bob@x"))
            self.assertEqual(d["statuses"], ["a", "B"])    # de-duplicated, sorted
            self.assertEqual(d["fields"][0]["id"], "customfield_20214")   # custom first
            pages = [u for u in self.urls(env) if "assignable" in u]
            self.assertEqual(len(pages), 4)               # page size 2: 2 + 1, per project
            d = json.loads(self.run_py(env, "jira_poll.py", "--describe").stdout)
            self.assertEqual(d["directory"]["counts"]["users"], 5)
            ep = [e for e in d["endpoints"] if e["type"] == "directory"][0]
            self.assertEqual((ep["window"], ep["items"], ep["file"]), ("1w", 5, ""))
            self.assertTrue(any("assignable users of P2" in r["purpose"] for r in ep["requests"]))
            self.assertTrue(any("labels of P1" in r["purpose"] for r in ep["requests"]))
            with open(os.path.join(tmp, "cache", "directory.json")) as fh:
                d = json.load(fh)
            # releases: archived dropped, unreleased first; per project
            self.assertEqual([(v["name"], v["project"]) for v in d["versions"]],
                             [("2.0", "P1"), ("2.0", "P2"), ("1.0", "P1"), ("1.0", "P2")])
            self.assertEqual(d["labels"], [{"name": "p1", "projects": ["P1"]},
                                           {"name": "p2", "projects": ["P2"]},
                                           {"name": "shared", "projects": ["P1", "P2"]}])

    def test_upgrade_drops_searches_adds_directory_once(self):
        with tempfile.TemporaryDirectory() as tmp:
            env = self.env(tmp, searches=[{"name": "s1", "kind": "text", "file": "search-s1.json"}])
            os.makedirs(os.path.join(tmp, "out"))
            stale = os.path.join(tmp, "out", "search-s1.json")
            open(stale, "w").close()
            p = self.run_py(env, "jira_config.py", "--check")
            self.assertIn("removed 1 saved search", p.stdout)
            cfg = self.cfgjson(env)
            self.assertNotIn("searches", cfg)
            self.assertFalse(os.path.exists(stale))
            self.assertEqual([e["type"] for e in cfg["endpoints"]], ["issues", "directory", "favorites"])
            self.assertNotIn("columns", cfg["endpoints"][1])
            r = json.loads(self.run_py(env, "jira_config.py", "--delete-endpoint", "directory").stdout)
            self.assertTrue(r["ok"])
            self.run_py(env, "jira_config.py", "--check")
            self.assertEqual([e["type"] for e in self.cfgjson(env)["endpoints"]],
                             ["issues", "favorites"])   # directory not re-added

    def test_page_size_reaches_the_curl(self):
        with tempfile.TemporaryDirectory() as tmp:
            env = self.env(tmp)
            up = json.loads(self.run_py(env, "jira_config.py", "--upsert-endpoint",
                                        stdin=json.dumps({"name": "all", "maxResults": 7})).stdout)
            self.assertTrue(up["ok"], up)
            d = json.loads(self.run_py(env, "jira_poll.py", "--describe").stdout)
            ep = [e for e in d["endpoints"] if e["name"] == "all"][0]
            self.assertIn("maxResults=7", ep["requests"][0]["curl"])
            self.assertEqual(ep["maxResults"], 7)
            self.assertEqual(d["searchDefaults"]["max_results_search"], 500)

    def test_team_set_validates_and_keeps_other_keys(self):
        with tempfile.TemporaryDirectory() as tmp:
            env = self.env(tmp)
            with open(env["JIRA_TEAM_JSON"], "w") as fh:
                json.dump({"Project Keys": ["OLD"], "boards": [{"id": 1}], "extra": 1}, fh)
            ts = lambda k, v: json.loads(self.run_py(  # noqa: E731
                env, "jira_config.py", "--team-set", k, stdin=json.dumps(v)).stdout)
            self.assertFalse(ts("project_keys", ["bad key"])["ok"])
            self.assertFalse(ts("project_keys", "P1")["ok"])                    # not a list
            self.assertFalse(ts("custom_fields", {"pkg": {"field_id": "summary"}})["ok"])
            self.assertFalse(ts("custom_fields", {"pkg": {"label": "no id"}})["ok"])
            self.assertFalse(ts("site", "x")["ok"])                             # not editable
            self.assertTrue(ts("project_keys", ["P1", "P2"])["ok"])
            r = ts("custom_fields", {"pkg": {"field_id": "customfield_1", "label": "Pkg"}})
            self.assertTrue(r["ok"], r)
            with open(env["JIRA_TEAM_JSON"]) as fh:
                t = json.load(fh)
            self.assertEqual(t["project_keys"], ["P1", "P2"])
            self.assertNotIn("Project Keys", t)        # the old spelling is replaced
            self.assertEqual((t["boards"], t["extra"]), ([{"id": 1}], 1))
            self.assertEqual(t["custom_fields"]["pkg"]["field_id"], "customfield_1")


def isolated_env(tmp, token=""):
    """Env pointing every path at `tmp` (never touches the real config)."""
    conf = os.path.join(tmp, "commands.conf")
    with open(conf, "w") as fh:
        fh.write("[jira]\nenabled = true\ntype = list\n")
    cfgj = os.path.join(tmp, "config.json")
    with open(cfgj, "w") as fh:
        json.dump({"site": "https://sudosignup.atlassian.net", "email": "sudosignup@proton.me",
                   "token": token, "setup": {"state": "done", "steps": {}}, "endpoints": [
                       {"name": "all", "window": "10m", "projects": "*", "type": "issues",
                        "file": "all.json", "enabled": True}]}, fh)
    with open(os.path.join(tmp, "team.json"), "w") as fh:
        json.dump({"project_keys": ["P"]}, fh)
    env = dict(os.environ)
    env.update({"WS_COMMANDS_CONF": conf, "JIRA_CONFIG_JSON": cfgj,
                "JIRA_CONFIG_FILE": os.path.join(tmp, "nolegacy"),
                "JIRA_CACHE_DIR": os.path.join(tmp, "cache"),
                "JIRA_TEAM_JSON": os.path.join(tmp, "team.json")})
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

    def test_describe_lists_full_curl_and_jql(self):
        with tempfile.TemporaryDirectory() as tmp:
            env = isolated_env(tmp, token="SECRET")
            with open(env["JIRA_CONFIG_JSON"]) as fh:
                cfg = json.load(fh)
            cfg.update({"site": "https://jira.example.com", "email": "", "auth": "bearer"})
            cfg["endpoints"].append({"name": "mine", "window": "30m", "projects": ["P"],
                                     "type": "issues", "file": "mine.json", "job": "users_search"})
            with open(env["JIRA_CONFIG_JSON"], "w") as fh:
                json.dump(cfg, fh)
            env["PATH"] = tmp   # describe must never run curl
            p = subprocess.run([sys.executable, os.path.join(JIRA, "jira_poll.py"), "--describe"],
                               env=env, capture_output=True, text=True, timeout=30)
            self.assertEqual(p.returncode, 0, p.stderr)
            d = json.loads(p.stdout)
            self.assertEqual(d["auth"], "bearer")
            self.assertIn("-H 'Authorization: Bearer SECRET'", d["loginCurl"])
            eps = {e["name"]: e for e in d["endpoints"]}
            self.assertEqual(eps["all"]["nextWindow"], "full")      # no cache yet
            # readable: the JQL is shown as written, curl url-encodes it
            self.assertIn("-G", eps["all"]["requests"][0]["curl"])
            self.assertIn("'https://jira.example.com/rest/api/2/search' --data-urlencode 'jql=",
                          eps["all"]["requests"][0]["curl"])
            self.assertIn('project in ("P")', eps["mine"]["jql"])   # job -> jql
            self.assertEqual(eps["mine"]["job"], "users_search")

    def test_cancel_without_running_poll(self):
        with tempfile.TemporaryDirectory() as tmp:
            env = isolated_env(tmp, token="x")
            p = subprocess.run([sys.executable, os.path.join(JIRA, "jira_poll.py"), "--cancel"],
                               env=env, capture_output=True, text=True, timeout=30)
            self.assertEqual(p.returncode, 0, p.stderr)
            self.assertIn("no poll is running", p.stdout)

    def test_poll_when_disabled_keeps_poll_active(self):
        with tempfile.TemporaryDirectory() as tmp:
            conf = os.path.join(tmp, "commands.conf")
            with open(conf, "w") as fh:
                fh.write("[jira]\nenabled = false\npoll-when-disabled = true\n")
            self.assertFalse(jira_config.jira_enabled(conf))
            self.assertTrue(jira_config.poll_active(conf))
            with open(conf, "w") as fh:
                fh.write("[jira]\nenabled = false\n")
            self.assertFalse(jira_config.poll_active(conf))

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


# stateful fake Jira (v2): N issues across projects, oldest-first search with
# `updated >= / created >=` bounds, comments in the search, and injectable
# failures (counters live in FAKE_DIR so they span curl invocations)
FAKE_JIRA = r"""#!/usr/bin/env python3
import datetime, json, os, re, sys, urllib.parse
D = os.environ["FAKE_DIR"]
def bump(name):
    p = os.path.join(D, name)
    n = int(open(p).read()) if os.path.exists(p) else 0
    open(p, "w").write(str(n + 1))
    return n
with open(os.path.join(D, "calls.jsonl"), "a") as fh:
    fh.write(json.dumps(sys.argv[1:]) + "\n")
url = sys.argv[-1]
u = urllib.parse.urlparse(url)
q = urllib.parse.parse_qs(u.query)
N = int(os.environ.get("FAKE_N", "12"))
projs = os.environ.get("FAKE_PROJECTS", "P").split(",")
def iso(i):
    return (datetime.datetime(2026, 9, 1) + datetime.timedelta(hours=i)).strftime("%Y-%m-%dT%H:%M:00.000+0000")
def issue(i):
    p = projs[i % len(projs)]
    return {"key": f"{p}-{i}", "fields": {
        "summary": f"s{i}", "updated": iso(i), "created": iso(i), "project": {"key": p},
        "status": {"name": "Open"},
        "comment": {"total": 2 if (i == 0 and os.environ.get("FAKE_TRUNC")) else 1,
                    "comments": [{"author": {"displayName": "A"}, "body": f"c{i}"}]}}}
code, ra, body = 200, "", None
if u.path.endswith("/myself"):
    body = {"displayName": "Fake", "timeZone": "UTC"}
    if os.environ.get("FAKE_401") == "always":
        code, body = 401, {}
elif u.path.endswith("/project"):
    body = [{"key": p, "name": p} for p in projs]
elif u.path.endswith("/versions"):
    body = json.loads(os.environ.get("FAKE_VERSIONS", "[]"))
elif "/issue/" in u.path:
    body = {"fields": {"comment": {"comments": [{"author": {"displayName": "A"}, "body": "x"},
                                                {"author": {"displayName": "B"}, "body": "y"}]}}}
elif u.path.endswith("/search") and \
        int(q.get("maxResults", ["0"])[0]) > int(os.environ.get("FAKE_BREAK_ABOVE", "999999")):
    # a proxy that cuts big responses off mid-body (curl exit 18)
    sys.stdout.write('{"startAt": 0, "total": 12, "issues": [{"key": "P-')
    sys.stderr.write("curl: (18) transfer closed with outstanding read data remaining\n")
    sys.exit(18)
elif u.path.endswith("/user/assignable/search"):
    proj, start, n = q["project"][0], int(q.get("startAt", ["0"])[0]), int(q["maxResults"][0])
    us = [{"name": f"{proj.lower()}u{i}", "displayName": f"{proj} U{i}"} for i in range(3)]
    # FAKE_USERS_SAME: a server that ignores startAt (the same page forever)
    body = us[:n] if os.environ.get("FAKE_USERS_SAME") else us[start:start + n]
elif u.path.endswith("/search") and "labels is not EMPTY" in q.get("jql", [""])[0]:
    proj = re.search(r'project = "([^"]+)"', q["jql"][0]).group(1)
    if proj in os.environ.get("FAKE_LABELS_401", "").split(","):
        code, ra, body = 401, "0", {"message": "Client must be authenticated to access this resource."}
    else:
        body = {"startAt": 0, "total": 1, "issues": [{"key": f"{proj}-1", "fields": {"labels": [f"l{proj}"]}}]}
elif u.path.endswith("/search"):
    n429 = int(os.environ.get("FAKE_429", "0"))
    if n429 and bump("429") < n429:
        code, ra, body = 429, "0", {"errorMessages": ["slow down"]}
    elif os.environ.get("FAKE_401_ONCE") and bump("401") == 0:
        code, ra, body = 401, os.environ.get("FAKE_401_RA", ""), {}
    elif os.environ.get("FAKE_SEARCH_401"):
        code, ra, body = 401, "0", {"message": "shed"}
    else:
        jql = q["jql"][0]
        items = [issue(i) for i in range(N)]
        m = re.search(r'(updated|created) >= "(\d{4}-\d\d-\d\d \d\d:\d\d)"', jql)
        if m:
            items = [x for x in items if x["fields"][m.group(1)][:16].replace("T", " ") >= m.group(2)]
        m = re.search(r'project in \(([^)]*)\)', jql)
        if m:
            keep = re.findall(r'"([^"]+)"', m.group(1))
            items = [x for x in items if x["fields"]["project"]["key"] in keep]
        m = re.search(r'key in \(([^)]*)\)', jql)
        bad = [k for k in os.environ.get("FAKE_BAD_KEYS", "").split(",") if k and m and k in m.group(1)]
        if m:
            keys = [k.strip() for k in m.group(1).split(",")]
            items = [x for x in items if x["key"] in keys]
        start, n = int(q.get("startAt", ["0"])[0]), int(q["maxResults"][0])
        fail = os.environ.get("FAKE_FAIL_AT")
        if bad:
            code, body = 400, {"errorMessages": [f"An issue with key '{bad[0]}' does not exist for field 'key'."]}
        elif fail and start >= int(fail) and bump("fail") == 0:
            code, body = 400, {"errorMessages": ["boom"]}
        else:
            body = {"startAt": start, "total": len(items), "issues": items[start:start + n]}
else:
    body = {}
if "-D" in sys.argv:     # raw capture: the response headers
    with open(sys.argv[sys.argv.index("-D") + 1], "w") as fh:
        fh.write(f"HTTP/1.1 {code} X\r\nRetry-After: {ra}\r\nX-Fake: yes\r\n\r\n")
sys.stdout.write(json.dumps(body) + f"\n{code} {ra}")
"""


class ResilientSyncTests(unittest.TestCase):
    """Rate limits, streaming + resume, one request per page, shared sync,
    scope, setup gate, rebuild."""

    def env(self, tmp, endpoints=None, scope=("P",), setup="done", **extra):
        env = isolated_env(tmp, token="T")
        with open(env["JIRA_CONFIG_JSON"]) as fh:
            cfg = json.load(fh)
        cfg.update({"site": "https://jira.example.com", "email": "", "auth": "bearer",
                    "outDir": os.path.join(tmp, "out"), "fetchComments": True,
                    "checkpointEvery": 1, "setup": {"state": setup, "steps": {}},
                    "directoryJob": True, **extra})
        if endpoints is not None:
            cfg["endpoints"] = endpoints
        with open(env["JIRA_CONFIG_JSON"], "w") as fh:
            json.dump(cfg, fh)
        with open(env["JIRA_TEAM_JSON"], "w") as fh:
            json.dump({"project_keys": list(scope), "search_defaults": {"max_results_search": 5}}, fh)
        bindir = os.path.join(tmp, "bin")
        os.makedirs(bindir)
        with open(os.path.join(bindir, "curl"), "w") as fh:
            fh.write(FAKE_JIRA)
        os.chmod(os.path.join(bindir, "curl"), 0o755)
        env["PATH"] = bindir + os.pathsep + env["PATH"]
        env["FAKE_DIR"] = tmp
        return env

    def poll(self, env, *args):
        return subprocess.run([sys.executable, os.path.join(JIRA, "jira_poll.py"), *args], env=env,
                              capture_output=True, text=True, timeout=60)

    def calls(self, tmp):
        p = os.path.join(tmp, "calls.jsonl")
        if not os.path.exists(p):
            return []
        with open(p) as fh:
            return [json.loads(x)[-1] for x in fh]

    def cache(self, tmp):
        with open(os.path.join(tmp, "cache", "jiras.json")) as fh:
            return json.load(fh)

    # -- Client.get
    def client_env(self, tmp, **fake):
        env = self.env(tmp)
        old = {k: os.environ.get(k) for k in ["PATH", "FAKE_DIR", *fake]}
        os.environ.update({"PATH": env["PATH"], "FAKE_DIR": tmp, **fake})
        return old

    def restore(self, old):
        for k, v in old.items():
            if v is None:
                os.environ.pop(k, None)
            else:
                os.environ[k] = v

    def client(self):
        c = jira_api.Client("https://jira.example.com", "T", team=jira_config.load_team({"teamConfig": "/x"}))
        c.on_wait = lambda m, s: None
        return c

    def test_429_waits_retry_after_then_succeeds(self):
        with tempfile.TemporaryDirectory() as tmp:
            old, slept = self.client_env(tmp, FAKE_429="2"), []
            old_sleep, old_log = jira_api.SLEEP, jira_api.CURL_LOG
            jira_api.SLEEP, jira_api.CURL_LOG = slept.append, os.path.join(tmp, "curl.log")
            try:
                res = self.client().search("project = P", "summary")
            finally:
                jira_api.SLEEP, jira_api.CURL_LOG = old_sleep, old_log
                self.restore(old)
            self.assertEqual(len(res["issues"]), 12)
            self.assertEqual(slept, [0.0, 0.0])       # Retry-After: 0, twice
            self.assertEqual(len(self.calls(tmp)), 2 + 1)   # the same request re-sent, one page

    def test_401_first_request_fails_fast_but_mid_run_401_recovers(self):
        with tempfile.TemporaryDirectory() as tmp:
            old = self.client_env(tmp, FAKE_401="always")
            old_sleep, old_log = jira_api.SLEEP, jira_api.CURL_LOG
            jira_api.SLEEP, jira_api.CURL_LOG = (lambda s: None), os.path.join(tmp, "curl.log")
            try:
                with self.assertRaises(jira_api.ApiError) as cm:
                    self.client().get("/rest/api/2/myself")
                self.assertEqual(cm.exception.code, 401)
                self.assertEqual(len(self.calls(tmp)), 1)          # a bad token never loops
                os.environ.pop("FAKE_401")
                os.environ["FAKE_401_ONCE"] = "1"
                c = self.client()
                c.get("/rest/api/2/myself")                          # a 2xx first ...
                self.assertEqual(len(c.search("project = P", "summary")["issues"]), 12)
                self.assertEqual(c.retries, 1)                      # ... then a 401 is waited out
            finally:
                jira_api.SLEEP, jira_api.CURL_LOG = old_sleep, old_log
                os.environ.pop("FAKE_401_ONCE", None)
                self.restore(old)

    # -- sync
    def test_comments_ride_in_the_search_one_request_per_page(self):
        with tempfile.TemporaryDirectory() as tmp:
            env = self.env(tmp)
            p = self.poll(env, "--force", "--projects", "all")
            self.assertEqual(p.returncode, 0, p.stderr)
            urls = self.calls(tmp)
            self.assertFalse([u for u in urls if "/issue/" in u])     # no per-issue calls
            searches = [u for u in urls if "/search?" in u]
            self.assertEqual(len(searches), 3)                        # 12 issues, pages of 5 (+overlap 0)
            self.assertIn("comment", searches[0])
            self.assertIn("ORDER%20BY%20created%20ASC", searches[0])  # full sync: oldest first
            c = self.cache(tmp)
            self.assertEqual(len(c), 12)
            self.assertEqual(c["P-3"]["comments"][0]["body"], "c3")
            self.assertNotIn("comment", c["P-3"])
            with open(os.path.join(tmp, "out", "all.json")) as fh:
                self.assertEqual(len(json.load(fh)), 12)
            with open(os.path.join(tmp, "cache", "poll.log")) as fh:
                log = fh.read()
            self.assertIn("[sync] page 3: 12/12 (100%)", log)

    def test_truncated_comments_fall_back_to_the_issue(self):
        with tempfile.TemporaryDirectory() as tmp:
            env = self.env(tmp)
            env["FAKE_TRUNC"] = "1"
            self.assertEqual(self.poll(env, "--force", "--projects", "all").returncode, 0)
            self.assertEqual(len([u for u in self.calls(tmp) if "/issue/" in u]), 1)
            self.assertEqual(len(self.cache(tmp)["P-0"]["comments"]), 2)

    def test_failure_keeps_progress_and_the_next_run_resumes(self):
        with tempfile.TemporaryDirectory() as tmp:
            env = self.env(tmp)
            env["FAKE_FAIL_AT"] = "10"
            p = self.poll(env, "--force", "--projects", "all")
            self.assertEqual(p.returncode, 1)
            self.assertEqual(len(self.cache(tmp)), 10)               # pages 1-2 kept
            ck = os.path.join(tmp, "cache", "checkpoints", "sync.json")
            with open(ck) as fh:
                self.assertEqual(json.load(fh)["fetched"], 10)
            with open(os.path.join(tmp, "out", "all.json")) as fh:
                self.assertEqual(len(json.load(fh)), 10)             # partial tab published
            before = len(self.calls(tmp))
            p = self.poll(env, "--force", "--projects", "all")
            self.assertEqual(p.returncode, 0, p.stderr)
            again = [u for u in self.calls(tmp)[before:] if "/search?" in u]
            self.assertIn("created%20%3E%3D%20%222026-09-01%2008%3A", again[0])  # hwm P-9 - 1 min
            self.assertEqual(len(again), 1)
            self.assertEqual(len(self.cache(tmp)), 12)
            self.assertFalse(os.path.exists(ck))

    def out(self, tmp, name):
        with open(os.path.join(tmp, "out", name)) as fh:
            return json.load(fh)

    def test_favorites_job_requeries_the_pinned_keys(self):
        eps = [{"name": "all", "window": "10m", "projects": "*", "type": "issues", "file": "all.json"},
               {"name": "favorites", "window": "10m", "projects": "*", "type": "favorites",
                "file": "favorites.json"}]
        with tempfile.TemporaryDirectory() as tmp:
            env = self.env(tmp, endpoints=eps, favoritesJob=True)
            p = self.poll(env, "--force", "--projects", "favorites")      # nothing pinned
            self.assertEqual(p.returncode, 0, p.stderr)
            self.assertEqual(self.out(tmp, "favorites.json"), [])
            self.assertEqual([u for u in self.calls(tmp) if "/search?" in u], [])   # no request
            r = subprocess.run([sys.executable, os.path.join(JIRA, "jira_poll.py"), "--favorite",
                                "add", "P-3", "P-1"], env=env, input='[{"key": "P-3", "title": "seen"}]',
                               capture_output=True, text=True, timeout=60)
            self.assertTrue(json.loads(r.stdout)["ok"], r.stdout + r.stderr)
            # at once, from the row the app passed (the cache doesn't hold it yet)
            self.assertEqual([x["key"] for x in self.out(tmp, "favorites.json")], ["P-3"])
            p = self.poll(env, "--force", "--projects", "favorites")
            self.assertEqual(p.returncode, 0, p.stderr)
            rows = self.out(tmp, "favorites.json")
            self.assertEqual([(x["key"], x["title"]) for x in rows], [("P-3", "s3"), ("P-1", "s1")])
            jql = urllib.parse.unquote_plus([u for u in self.calls(tmp) if "/search?" in u][0])
            self.assertIn("key in (P-3, P-1)", jql)
            self.assertIn('project in ("P")', jql)                        # clamped to the scope
            r = subprocess.run([sys.executable, os.path.join(JIRA, "jira_poll.py"), "--favorite",
                                "remove", "P-3"], env=env, capture_output=True, text=True, timeout=60)
            self.assertEqual(json.loads(r.stdout)["keys"], ["P-1"])
            self.assertEqual([x["key"] for x in self.out(tmp, "favorites.json")], ["P-1"])

    def test_favorites_skip_a_key_jira_rejects(self):
        eps = [{"name": "favorites", "window": "10m", "projects": "*", "type": "favorites",
                "file": "favorites.json"}]
        with tempfile.TemporaryDirectory() as tmp:
            env = self.env(tmp, endpoints=eps, favoritesJob=True, favorites=["P-99", "P-2"])
            env["FAKE_BAD_KEYS"] = "P-99"
            p = self.poll(env, "--force", "--projects", "favorites")
            self.assertEqual(p.returncode, 0, p.stderr)
            self.assertEqual([x["key"] for x in self.out(tmp, "favorites.json")], ["P-2"])

    def test_release_blacklist_splits_the_tab_and_restores(self):
        eps = [{"name": "releases", "window": "1h", "projects": "*", "type": "releases",
                "file": "releases.json"}]
        vers = [{"id": "101", "name": "1.0", "released": True, "releaseDate": "2026-01-01"},
                {"id": "102", "name": "2.0", "released": False}]
        with tempfile.TemporaryDirectory() as tmp:
            env = self.env(tmp, endpoints=eps, favoritesJob=True, releaseBlacklist=["P-1.0"])
            env["FAKE_VERSIONS"] = json.dumps(vers)
            p = self.poll(env, "--force", "--projects", "releases")
            self.assertEqual(p.returncode, 0, p.stderr)
            shown = self.out(tmp, "releases.json")
            self.assertEqual([(x["key"], x["versionId"]) for x in shown], [("P-2.0", "102")])
            self.assertEqual([x["key"] for x in self.out(tmp, "blacklist_release.json")], ["P-1.0"])
            run = lambda *a: json.loads(subprocess.run(  # noqa: E731
                [sys.executable, os.path.join(JIRA, "jira_poll.py"), "--blacklist-release", *a],
                env=env, capture_output=True, text=True, timeout=60).stdout)
            self.assertEqual(run("remove", "P-1.0")["keys"], [])
            # release_items' order: dated newest first, undated last
            self.assertEqual([x["key"] for x in self.out(tmp, "releases.json")], ["P-1.0", "P-2.0"])
            self.assertEqual(self.out(tmp, "blacklist_release.json"), [])
            self.assertEqual(run("add", "P-2.0")["keys"], ["P-2.0"])
            self.assertEqual([x["key"] for x in self.out(tmp, "releases.json")], ["P-1.0"])
            self.assertEqual([x["key"] for x in self.out(tmp, "blacklist_release.json")], ["P-2.0"])
            with open(env["JIRA_CONFIG_JSON"]) as fh:
                self.assertEqual(json.load(fh)["releaseBlacklist"], ["P-2.0"])
            p = self.poll(env, "--force", "--projects", "releases")    # the poll keeps it hidden
            self.assertEqual([x["key"] for x in self.out(tmp, "releases.json")], ["P-1.0"])

    def test_overlapping_jobs_share_one_sync(self):
        eps = [{"name": n, "window": "10m", "projects": pr, "type": "issues", "file": f"{n}.json"}
               for n, pr in (("all", "*"), ("KAN", ["KAN"]), ("SAM1", ["SAM1"]))]
        with tempfile.TemporaryDirectory() as tmp:
            env = self.env(tmp, endpoints=eps, scope=("KAN", "SAM1"))
            env["FAKE_PROJECTS"] = "KAN,SAM1"
            p = self.poll(env, "--force", "--projects", "*")
            self.assertEqual(p.returncode, 0, p.stderr)
            self.assertEqual(len([u for u in self.calls(tmp) if "/search?" in u]), 3)   # one sequence
            rows = {}
            for n in ("all", "KAN", "SAM1"):
                with open(os.path.join(tmp, "out", f"{n}.json")) as fh:
                    rows[n] = len(json.load(fh))
            self.assertEqual(rows, {"all": 12, "KAN": 6, "SAM1": 6})

    def test_a_project_added_to_the_scope_gets_a_full_sync(self):
        with tempfile.TemporaryDirectory() as tmp:
            env = self.env(tmp, scope=("KAN",))
            env["FAKE_PROJECTS"] = "KAN,SAM1"
            self.assertEqual(self.poll(env, "--force", "--projects", "all").returncode, 0)
            self.assertEqual({v["project"] for v in self.cache(tmp).values()}, {"KAN"})
            with open(env["JIRA_TEAM_JSON"], "w") as fh:
                json.dump({"project_keys": ["KAN", "SAM1"], "search_defaults": {"max_results_search": 5}}, fh)
            before = len(self.calls(tmp))
            p = self.poll(env, "--force", "--projects", "all")
            self.assertEqual(p.returncode, 0, p.stderr)
            first = [u for u in self.calls(tmp)[before:] if "/search?" in u][0]
            self.assertIn("project%20in%20%28%22SAM1%22%29", first)      # only the new one, in full
            self.assertIn("ORDER%20BY%20created", first)
            self.assertEqual(len([v for v in self.cache(tmp).values() if v["project"] == "SAM1"]), 6)

    def test_scope_is_required_and_star_means_scope(self):
        with tempfile.TemporaryDirectory() as tmp:
            env = self.env(tmp, scope=())
            p = self.poll(env, "--force", "--projects", "all")
            self.assertEqual(p.returncode, 2)
            self.assertIn("no projects in scope", p.stderr)
            self.assertEqual(self.calls(tmp), [])
        with tempfile.TemporaryDirectory() as tmp:
            env = self.env(tmp)
            env["FAKE_PROJECTS"] = "P,OTHER"
            self.assertEqual(self.poll(env, "--force", "--projects", "all").returncode, 0)
            self.assertIn("project%20in%20%28%22P%22%29", self.calls(tmp)[0])
            self.assertEqual({v["project"] for v in self.cache(tmp).values()}, {"P"})

    def test_setup_gates_the_tick_and_runs_steps_one_at_a_time(self):
        eps = [{"name": "all", "window": "10m", "projects": "*", "type": "issues", "file": "all.json"},
               {"name": "releases", "window": "1h", "projects": "*", "type": "releases",
                "file": "releases.json"}]
        with tempfile.TemporaryDirectory() as tmp:
            env = self.env(tmp, endpoints=eps, setup="pending")
            p = self.poll(env)
            self.assertEqual(p.returncode, 0)
            self.assertEqual(self.calls(tmp), [])                     # no network while pending
            with open(os.path.join(tmp, "cache", "status.json")) as fh:
                self.assertEqual(json.load(fh)["status"], "setup pending")
            env["FAKE_FAIL_AT"] = "5"
            p = self.poll(env, "--setup")
            self.assertEqual(p.returncode, 1)
            with open(env["JIRA_CONFIG_JSON"]) as fh:
                st = json.load(fh)["setup"]
            self.assertEqual(st["state"], "pending")
            self.assertEqual({k: v["state"] for k, v in st["steps"].items()},
                             {"connection": "ok", "scope": "ok", "releases": "ok", "sync": "error"})
            before = len(self.calls(tmp))
            p = self.poll(env, "--setup")                              # only what is not ok
            self.assertEqual(p.returncode, 0, p.stderr)
            done_again = [u for u in self.calls(tmp)[before:]
                          if u.endswith(("/myself", "/project"))]   # connection / scope: not rerun
            self.assertFalse(done_again)
            with open(env["JIRA_CONFIG_JSON"]) as fh:
                st = json.load(fh)["setup"]
            self.assertEqual(st["state"], "done")
            self.assertEqual(st["steps"]["all"]["items"], 12)
            p = self.poll(env, "--setup", "--step", "releases")        # rerun one
            self.assertEqual(p.returncode, 0, p.stderr)
            self.assertEqual(self.poll(env, "--setup", "--step", "nope").returncode, 2)
            # projects are never looked up: only /project/KEY for the keys the user entered
            self.assertFalse([u for u in self.calls(tmp) if u.endswith("/project")])
            self.assertTrue([u for u in self.calls(tmp) if u.endswith("/project/P")])

    def test_setup_migration_keeps_working_installs_running(self):
        with tempfile.TemporaryDirectory() as tmp:
            env = self.env(tmp)
            with open(env["JIRA_CONFIG_JSON"]) as fh:
                cfg = json.load(fh)
            cfg.pop("setup")
            with open(env["JIRA_CONFIG_JSON"], "w") as fh:
                json.dump(cfg, fh)
            os.makedirs(os.path.join(tmp, "cache"), exist_ok=True)
            with open(os.path.join(tmp, "cache", "status.json"), "w") as fh:
                json.dump({"endpoints": [{"name": "all", "lastSuccess": "2026-09-01 00:00:00"}]}, fh)
            chk = json.loads(subprocess.run([sys.executable, os.path.join(JIRA, "jira_config.py"),
                                             "--check"], env=env, capture_output=True, text=True).stdout)
            self.assertEqual(chk["setup"]["state"], "done")
            self.assertEqual(chk["projectKeys"], ["P"])

    def test_rebuild_wipes_and_repopulates(self):
        with tempfile.TemporaryDirectory() as tmp:
            env = self.env(tmp)
            self.assertEqual(self.poll(env, "--force", "--projects", "all").returncode, 0)
            c = self.cache(tmp)
            c["GONE-1"] = {"key": "GONE-1", "project": "P"}
            with open(os.path.join(tmp, "cache", "jiras.json"), "w") as fh:
                json.dump(c, fh)
            env["FAKE_FAIL_AT"] = "5"
            self.assertEqual(self.poll(env, "--rebuild").returncode, 1)
            with open(env["JIRA_CONFIG_JSON"]) as fh:
                self.assertEqual(json.load(fh)["rebuildOnNextPoll"], "resume")   # not re-wiped
            self.assertEqual(len(self.cache(tmp)), 5)
            p = self.poll(env)                                        # the next tick finishes it
            self.assertEqual(p.returncode, 0, p.stderr)
            self.assertEqual(sorted(self.cache(tmp)), sorted(f"P-{i}" for i in range(12)))
            with open(env["JIRA_CONFIG_JSON"]) as fh:
                self.assertFalse(json.load(fh)["rebuildOnNextPoll"])

    def test_mid_run_401_really_waits_and_names_the_request(self):
        with tempfile.TemporaryDirectory() as tmp:
            old, slept = self.client_env(tmp, FAKE_401_ONCE="1", FAKE_401_RA="0"), []
            old_sleep, old_log = jira_api.SLEEP, jira_api.CURL_LOG
            jira_api.SLEEP, jira_api.CURL_LOG = slept.append, os.path.join(tmp, "curl.log")
            try:
                c, waits = self.client(), []
                c.on_wait = lambda m, s: waits.append(m)
                c.get("/rest/api/2/myself")
                self.assertEqual(len(c.search("project = P", "summary")["issues"]), 12)
                self.assertEqual(slept, [5.0])          # Retry-After: 0 is not a wait for a 401
                self.assertIn("GET /rest/api/2/search", waits[0])
                self.assertIn("Retry-After 0s ignored", waits[0])
                os.environ.pop("FAKE_401_ONCE")
                os.environ["FAKE_SEARCH_401"] = "1"
                slept.clear()
                with self.assertRaises(jira_api.ApiError) as cm:
                    c.search("project = P", "summary")
                self.assertEqual(slept, [5.0, 10.0, 20.0, 40.0])
                msg = str(cm.exception)
                self.assertIn("token worked earlier", msg)
                self.assertIn("/rest/api/2/search", msg)
                self.assertIn("shed", msg)               # the server's own words
                self.assertNotIn("fix it in the setup sheet", msg)
            finally:
                jira_api.SLEEP, jira_api.CURL_LOG = old_sleep, old_log
                os.environ.pop("FAKE_SEARCH_401", None)
                self.restore(old)

    DIR_EP = [{"name": "directory", "window": "1w", "projects": "*", "type": "directory", "enabled": True}]

    def raw_runs(self, tmp):
        root = os.path.join(tmp, "cache", "raw")
        return sorted(os.path.join(root, d) for d in os.listdir(root))

    def test_directory_late_401_is_a_warning_raw_and_debug_log_show_it_rerun_retries_only_it(self):
        with tempfile.TemporaryDirectory() as tmp:
            env = self.env(tmp, endpoints=self.DIR_EP, scope=("P1", "P2"), token="SEKRETTOKEN42",
                           rateLimitMaxWaitMinutes=0)
            env["FAKE_LABELS_401"] = "P2"
            p = self.poll(env, "--directory")
            self.assertEqual(p.returncode, 0, p.stderr)
            with open(os.path.join(tmp, "cache", "directory.json")) as fh:
                d = json.load(fh)
            self.assertEqual(len(d["users"]), 6)
            self.assertEqual(d["labels"], [{"name": "lP1", "projects": ["P1"]}])
            self.assertEqual(len(d["warnings"]), 1)
            self.assertIn("labels of P2", d["warnings"][0])
            self.assertIn("token worked earlier", d["warnings"][0])
            ck = os.path.join(tmp, "cache", "checkpoints", "directory-directory.json")
            self.assertTrue(os.path.exists(ck))          # the failed part is kept to retry
            # raw: every request, the 401 with its body + headers
            run = self.raw_runs(tmp)[-1]
            with open(os.path.join(run, "manifest.jsonl")) as fh:
                man = [json.loads(x) for x in fh]
            self.assertEqual(len(man), len(self.calls(tmp)))
            bad = [m for m in man if m["httpCode"] == 401]
            self.assertEqual(len(bad), 1)
            self.assertEqual(bad[0]["stage"], "directory/labels/P2")
            with open(os.path.join(run, bad[0]["body"])) as fh:
                self.assertIn("Client must be authenticated", fh.read())
            with open(os.path.join(run, bad[0]["body"].replace(".json", ".meta.json"))) as fh:
                meta = json.load(fh)
            self.assertIn(["X-Fake", "yes"], meta["headers"])
            self.assertIn("$JIRA_TOKEN", meta["repro"])
            self.assertEqual(oct(os.stat(run).st_mode & 0o777), "0o700")
            with open(os.path.join(tmp, "cache", "debug.log")) as fh:
                log = fh.read()
            for want in ("▶ directory", "▶ users", "◀ users done", "▶ labels", "[directory/labels/P2]",
                         "HTTP 401 on GET /rest/api/2/search", "skipped labels of P2"):
                self.assertIn(want, log)
            for root, _, files in os.walk(os.path.join(tmp, "cache")):
                for f in files:
                    if f != "curl.log":                  # curl.log is the runnable one (0600)
                        with open(os.path.join(root, f), errors="replace") as fh:
                            self.assertNotIn("SEKRETTOKEN42", fh.read(), f)
            # rerun: only the failed part is fetched again
            os.unlink(os.path.join(tmp, "calls.jsonl"))
            env.pop("FAKE_LABELS_401")
            p = self.poll(env, "--directory")
            self.assertEqual(p.returncode, 0, p.stderr)
            self.assertEqual(len(self.calls(tmp)), 1, self.calls(tmp))
            self.assertIn("P2", self.calls(tmp)[0])
            with open(os.path.join(tmp, "cache", "directory.json")) as fh:
                d = json.load(fh)
            self.assertEqual((len(d["users"]), d["warnings"], len(d["labels"])), (6, [], 2))
            self.assertFalse(os.path.exists(ck))

    def test_directory_aborts_on_401s_in_a_row_and_resumes(self):
        with tempfile.TemporaryDirectory() as tmp:
            env = self.env(tmp, endpoints=self.DIR_EP, scope=("P1", "P2", "P3"), rateLimitMaxWaitMinutes=0)
            env["FAKE_LABELS_401"] = "P1,P2,P3"
            p = self.poll(env, "--directory")
            self.assertEqual(p.returncode, 1, p.stderr)
            self.assertFalse(os.path.exists(os.path.join(tmp, "cache", "directory.json")))
            with open(os.path.join(tmp, "cache", "debug.log")) as fh:
                log = fh.read()
            self.assertIn("✗ P3 failed", log)
            self.assertIn("Traceback", log)
            os.unlink(os.path.join(tmp, "calls.jsonl"))
            env.pop("FAKE_LABELS_401")
            p = self.poll(env, "--directory")
            self.assertEqual(p.returncode, 0, p.stderr)
            calls = self.calls(tmp)
            self.assertEqual(len(calls), 3)                   # labels of P1..P3 only
            self.assertTrue(all("labels" in u for u in calls))
            with open(os.path.join(tmp, "cache", "directory.json")) as fh:
                self.assertEqual(len(json.load(fh)["users"]), 9)

    def test_directory_users_stop_when_the_server_ignores_startat(self):
        with tempfile.TemporaryDirectory() as tmp:
            env = self.env(tmp, endpoints=self.DIR_EP, scope=("P1",))
            with open(env["JIRA_TEAM_JSON"], "w") as fh:
                json.dump({"project_keys": ["P1"], "search_defaults": {"max_results_users": 2}}, fh)
            env["FAKE_USERS_SAME"] = "1"
            p = self.poll(env, "--directory")
            self.assertEqual(p.returncode, 0, p.stderr)
            self.assertEqual(len([u for u in self.calls(tmp) if "assignable" in u]), 2)
            with open(os.path.join(tmp, "cache", "directory.json")) as fh:
                d = json.load(fh)
            self.assertEqual(len(d["users"]), 2)
            self.assertIn("ignores startAt", d["warnings"][0])

    def test_raw_runs_are_pruned(self):
        with tempfile.TemporaryDirectory() as tmp:
            env = self.env(tmp, endpoints=self.DIR_EP, scope=("P1",), rawKeepDays=1)
            root = os.path.join(tmp, "cache", "raw")
            os.makedirs(os.path.join(root, "old"))
            os.utime(os.path.join(root, "old"), (0, 0))
            os.makedirs(os.path.join(root, "recent"))
            self.assertEqual(self.poll(env, "--directory").returncode, 0)
            names = [os.path.basename(r) for r in self.raw_runs(tmp)]
            self.assertNotIn("old", names)
            self.assertIn("recent", names)
            self.assertEqual(len(names), 2)
        with tempfile.TemporaryDirectory() as tmp:     # rawCapture false: no folder at all
            env = self.env(tmp, endpoints=self.DIR_EP, scope=("P1",), rawCapture=False)
            self.assertEqual(self.poll(env, "--directory").returncode, 0)
            self.assertFalse(os.path.exists(os.path.join(tmp, "cache", "raw")))
            self.assertTrue(os.path.exists(os.path.join(tmp, "cache", "debug.log")))

    def test_a_page_that_breaks_off_is_shrunk_and_the_size_remembered(self):
        with tempfile.TemporaryDirectory() as tmp:
            env = self.env(tmp, rateLimitMaxWaitMinutes=0)
            with open(env["JIRA_TEAM_JSON"], "w") as fh:
                json.dump({"project_keys": ["P"], "search_defaults": {"max_results_search": 40}}, fh)
            env["FAKE_BREAK_ABOVE"] = "20"
            p = self.poll(env, "--init")
            self.assertEqual(p.returncode, 0, p.stderr)
            self.assertEqual(len(self.cache(tmp)), 12)            # everything, in a page of 20
            sizes = [int(re.search(r"maxResults=(\d+)", u).group(1)) for u in self.calls(tmp)
                     if "/search" in u]
            self.assertEqual(sizes, [40, 20])                     # 40 broke off -> halved, same position
            self.assertIn("re-reading the same position with 20", p.stderr)
            with open(os.path.join(tmp, "cache", "search_page_cap.json")) as fh:
                cap = json.load(fh)
            self.assertEqual((cap["pageSize"], cap["why"]), (20, "curl exit 18 on a page of 40"))
            raw = [f for r in self.raw_runs(tmp) for f in os.listdir(r) if "curl18" in f]
            self.assertTrue(raw)                                  # the cut-off body is kept
            os.unlink(os.path.join(tmp, "calls.jsonl"))
            self.assertEqual(self.poll(env, "--init").returncode, 0)
            self.assertFalse([u for u in self.calls(tmp) if "maxResults=40" in u])   # no re-probing

    def test_describe_shows_setup_scope_and_shared_sync(self):
        with tempfile.TemporaryDirectory() as tmp:
            env = self.env(tmp, setup="pending")
            env["PATH"] = tmp
            p = self.poll(env, "--describe")
            d = json.loads(p.stdout)
            self.assertEqual(d["scope"], ["P"])
            self.assertEqual(d["setup"]["state"], "pending")
            self.assertEqual([s["name"] for s in d["setup"]["steps"]],
                             ["connection", "scope", "sync", "all"])
            self.assertIn("comment", d["issueCache"]["requests"][0]["curl"])
            self.assertFalse([r for e in d["endpoints"] for r in e["requests"]
                              if "fields=comment" in r["curl"]])



if __name__ == "__main__":
    unittest.main(verbosity=2)
