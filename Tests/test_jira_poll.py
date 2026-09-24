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


FAKE_CURL = r'''#!/usr/bin/env python3
# fake curl: logs argv, serves a 5-issue v2 /search in startAt pages
import json, os, sys, urllib.parse
with open(os.environ["FAKE_CURL_LOG"], "a") as fh:
    fh.write(json.dumps(sys.argv[1:]) + "\n")
url = sys.argv[-1]
q = urllib.parse.parse_qs(urllib.parse.urlparse(url).query)
if url.endswith("/myself"):
    body = {"displayName": "Fake User", "name": "fake"}
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
            "curl -X GET -H 'Content-Type: application/json' -H 'Authorization: Bearer TOK' "
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
            self.assertIn("7", calls[0])      # timeout_seconds -> -m 7
            self.assertTrue(all("/rest/api/2/search?" in c[-1] for c in calls))
            self.assertIn("startAt=4", calls[2][-1])

    def test_curl_flag_prints_without_running(self):
        with tempfile.TemporaryDirectory() as tmp:
            env = isolated_env(tmp, token="SECRET")
            env["PATH"] = tmp   # no curl at all: --curl must not execute
            p = subprocess.run([sys.executable, os.path.join(JIRA, "jira_api.py"), "--curl",
                                "--mask", "--auth", "bearer", "--site", "https://jira.example.com"],
                               env=env, capture_output=True, text=True, timeout=30)
            self.assertEqual(p.returncode, 0, p.stderr)
            self.assertEqual(p.stdout.strip(),
                             "curl -X GET -H 'Content-Type: application/json' -H \"Authorization: "
                             "Bearer $JIRA_TOKEN\" 'https://jira.example.com/rest/api/2/myself'")
            self.assertNotIn("SECRET", p.stdout)


class JobsAndSearchesTests(unittest.TestCase):
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
            self.assertIn("gave 1 job(s) their own copy of [jira] columns", p.stdout)
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
            self.assertFalse(up({"name": "s1", "kind": "reporter"}, "search")["ok"])   # empty arg
            r = up({"name": "s1", "kind": "reporter", "args": {"username": "bob"}}, "search")
            self.assertTrue(r["ok"], r)
            self.assertEqual(self.cfgjson(env)["searches"][0]["file"], "search-s1.json")
            self.assertEqual(self.cfgjson(env)["searches"][0]["columns"], self.COLS)   # template
            r = json.loads(self.run_py(env, "jira_config.py", "--set-columns", "endpoint", "mine",
                                       "key:Key:50").stdout)
            self.assertTrue(r["ok"])
            self.assertTrue(json.loads(self.run_py(env, "jira_config.py", "--delete-endpoint",
                                                   "mine").stdout)["ok"])
            last = json.loads(self.run_py(env, "jira_config.py", "--delete-endpoint", "all").stdout)
            self.assertFalse(last["ok"])   # never delete the last job

    def test_per_job_api_fields_differ(self):
        a = jira_config.api_fields({}, {}, columns="key:K, labels:L")
        b = jira_config.api_fields({}, {}, columns="key:K, duedate:Due")
        self.assertIn("labels", a)
        self.assertNotIn("labels", b)
        self.assertIn("duedate", b)
        self.assertIn("duedate", jira_config.publish_keys({}, columns="key:K, duedate:Due"))

    def test_search_jql_kinds(self):
        t = {"project_keys": ["A"]}
        sj = jira_config.search_jql
        self.assertEqual(sj({"kind": "reporter", "args": {"username": "bob"}}, t),
                         'project in ("A") AND reporter = "bob"')
        self.assertEqual(sj({"kind": "project", "args": {"project": "Z"}}, t), 'project = "Z"')
        self.assertEqual(sj({"kind": "text", "args": {"query": "x"}, "projects": ["B"]}, t),
                         'project in ("B") AND (summary ~ "x" OR description ~ "x")')
        self.assertEqual(sj({"kind": "jql", "args": {"jql": "a = 1 ORDER BY created"}}, t), "a = 1")
        with self.assertRaises(jira_config.ConfigError):
            sj({"kind": "assignee", "args": {}}, t)

    def test_run_search_publishes_its_own_tab(self):
        with tempfile.TemporaryDirectory() as tmp:
            env = self.env(tmp, searches=[{"name": "s1", "kind": "text", "args": {"query": "pkg"},
                                           "file": "search-s1.json",
                                           "columns": "key:Key, title:Title, customfield_20214:Pkg"}])
            fake = os.path.join(tmp, "curl")
            with open(fake, "w") as fh:
                fh.write(FAKE_CURL)
            os.chmod(fake, 0o755)
            env["PATH"] = tmp + os.pathsep + env["PATH"]
            env["FAKE_CURL_LOG"] = os.path.join(tmp, "calls.jsonl")
            p = self.run_py(env, "jira_poll.py", "--search", "s1")
            self.assertEqual(p.returncode, 0, p.stderr)
            with open(os.path.join(tmp, "out", "search-s1.json")) as fh:
                items = json.load(fh)
            self.assertEqual([i["key"] for i in items], [f"P-{i}" for i in range(5)])
            self.assertEqual(items[0]["customfield_20214"], "pkg")
            with open(env["FAKE_CURL_LOG"]) as fh:
                urls = [json.loads(x)[-1] for x in fh]
            self.assertTrue(any("summary%20~%20%22pkg%22" in u for u in urls), urls)
            self.assertTrue(all("customfield_20214" in u for u in urls if "/search?" in u))
            with open(os.path.join(tmp, "cache", "status.json")) as fh:
                st = json.load(fh)["searches"]["s1"]
            self.assertEqual((st["status"], st["items"]), ("ok", 5))
            with open(os.path.join(tmp, "cache", "fields_seen.json")) as fh:
                self.assertIn("search-s1.json", json.load(fh)["customfield_20214"])
            d = json.loads(self.run_py(env, "jira_poll.py", "--describe").stdout)
            self.assertEqual(d["searches"][0]["items"], 5)
            cat = {c["field"]: c for c in d["catalog"]}
            self.assertIn("search:s1", cat["customfield_20214"]["usedBy"])


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
            self.assertIn("/rest/api/2/search?", eps["all"]["requests"][0]["curl"])
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


if __name__ == "__main__":
    unittest.main(verbosity=2)
