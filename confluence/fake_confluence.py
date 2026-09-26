#!/usr/bin/env python3
"""fake_confluence.py - a small, deterministic Confluence for development.

ONE implementation, two ways in:

  handle(method, url, headers) -> (status, headers, body_bytes)
      the whole fake: REST API + a browsable HTML site. The unit tests put a
      `curl` shim on PATH that calls it (Tests/test_confluence.py), the same
      trick as the fake Jira.

  python3 fake_confluence.py serve [--port 8765] [--context /wiki]
      a real HTTP server on 127.0.0.1 (bin/fake-confluence.sh start|stop)
      so the app can be used by hand: search, preview, images, "open in
      browser" (the /spaces/... pages render without auth).

REST (under the context path; `/wiki` = Cloud shape, `/confluence` = DC):
  GET /rest/api/search?cql=&start=&limit=&excerpt=highlight   (+ _links.next)
  GET /rest/api/content/search?cql=                           (no excerpts)
  GET /rest/api/content/ID?expand=body.view,space,version,ancestors,history
  GET /rest/api/space/KEY          404 when unknown (spaces are never listed)
  GET /rest/api/user/current       anonymous without auth (like DC)
  GET /download/attachments/ID/NAME.png   generated PNGs (auth required)

CQL: exactly what confluence_config.criteria_cql emits - text ~ / title ~
(words AND-ed, an escaped "phrase", trailing * prefix, light stemming,
always case-insensitive), space / type / label in|=, lastmodified >=
now("-Nd") | "YYYY-MM-DD", creator / contributor = currentUser(), AND / OR /
NOT / parentheses, ORDER BY field [ASC|DESC] (else relevance).

Auth: `Authorization: Bearer <token>` or basic `-u any:<token>`; token =
$FAKE_CONF_TOKEN (default "fake-token").
Failure switches (env): FAKE_CONF_429=N (first N searches answer 429),
FAKE_CONF_401=1 (every API call 401), FAKE_CONF_SLOW_MS=N (sleep per
request), FAKE_CONF_NO_SEARCH=1 (/rest/api/search 404s, like an old DC).
Counters live in $FAKE_DIR when set (they span curl invocations).
"""
from __future__ import annotations

import base64
import datetime as dt
import html
import json
import os
import random
import re
import struct
import sys
import time
import urllib.parse
import zlib

CURRENT_USER = {"displayName": "Fake User", "username": "fake", "accountId": "fake-0001"}
AUTHORS = ["Alex Chen", "Priya Natarajan", "Sam Okafor", "Jordan Lee", "Fake User", "Maria Rossi"]
SPACES = {
    "ENG": "Engineering",
    "OPS": "Operations",
    "TEAM": "Team Handbook",
    "HR": "People & HR",
}
TODAY = dt.datetime.now(dt.timezone.utc).replace(hour=9, minute=0, second=0, microsecond=0)


# ------------------------------------------------------------------ content

def _p(*paras):
    return "".join(f"<p>{x}</p>" for x in paras)


def _code(text, lang="bash"):
    return (f'<div class="code panel pdl conf-macro output-block" data-macro-name="code">'
            f'<div class="codeContent panelContent pdl"><pre class="syntaxhighlighter-pre" '
            f'data-syntaxhighlighter-params="brush: {lang}">{html.escape(text)}</pre></div></div>')


def _panel(kind, text):
    return (f'<div class="confluence-information-macro confluence-information-macro-{kind} conf-macro" '
            f'data-macro-name="{kind}"><div class="confluence-information-macro-body">{_p(text)}</div></div>')


def _table(head, rows):
    th = "".join(f"<th>{h}</th>" for h in head)
    trs = "".join("<tr>" + "".join(f"<td>{c}</td>" for c in r) + "</tr>" for r in rows)
    return f'<div class="table-wrap"><table class="confluenceTable"><tbody><tr>{th}</tr>{trs}</tbody></table></div>'


def _ul(*items):
    return "<ul>" + "".join(f"<li>{i}</li>" for i in items) + "</ul>"


# (space, title, parent title | None, author index, days ago, labels, body, image names)
HAND = [
    ("ENG", "Engineering Home", None, 0, 200, ["home"],
     "<h1>Welcome to Engineering</h1>" + _p(
         "This space holds how we build, ship and operate our services. Start with the "
         "<a href=\"/spaces/ENG/pages/1002\">Blue green deploy runbook</a> if you are on call this week.",
         "Architecture decisions live under <strong>Architecture Decision Records</strong>; team rituals "
         "under the Team Handbook space.") + _ul("Deploys and releases", "Service catalog",
                                                 "Architecture Decision Records", "Incident reviews"), []),
    ("ENG", "Blue green deploy runbook", "Engineering Home", 1, 3, ["runbook", "deploy"],
     "<h2>Overview</h2>" + _p(
         "A blue green deploy keeps two identical production stacks. Traffic points at one (blue) while "
         "the new release is rolled out to the other (green). Once green passes its checks we flip the "
         "load balancer and blue becomes the rollback target.",
         "Use this runbook for every production release of the payments and checkout services.")
     + _panel("info", "Deploy windows are Tuesday and Thursday 10:00-15:00 UTC. Outside of them, get a "
                      "release manager to approve in #release.")
     + "<h2>Steps</h2><ol><li>Announce the deploy in <code>#release</code>.</li>"
       "<li>Run the pipeline stage <em>deploy-green</em>.</li><li>Watch the green dashboards for 10 minutes.</li>"
       "<li>Flip traffic with the command below.</li><li>Keep blue warm for 24 hours.</li></ol>"
     + _code("deployctl switch --service checkout --to green --confirm\ndeployctl status checkout")
     + '<p><img class="confluence-embedded-image" src="/download/attachments/1002/traffic-switch.png" '
       'alt="traffic switch diagram" width="480"></p>'
     + "<h2>Rollback</h2>" + _p("Rolling back is the same switch pointed at blue. Do it the moment error "
                                "rates rise above 2% - do not debug on green while customers see errors.")
     + _code("deployctl switch --service checkout --to blue --confirm"),
     ["traffic-switch.png"]),
    ("ENG", "Canary releases", "Engineering Home", 2, 12, ["deploy", "canary"],
     _p("Canary releases send a small share of traffic to the new version first. Our blue team deploys "
        "canaries at 5%, then 25%, then 100%, and every step needs a green light from the SLO dashboard.",
        "Canaries are slower than a blue green switch but catch problems that only real traffic shows.")
     + _table(["Step", "Traffic", "Wait"], [["1", "5%", "15 min"], ["2", "25%", "30 min"], ["3", "100%", "-"]]),
     []),
    ("ENG", "Service catalog", "Engineering Home", 3, 40, ["catalog"],
     _p("Every service we run, who owns it and where its dashboards are.")
     + _table(["Service", "Owner", "Language", "Tier"],
              [["checkout", "Payments", "Go", "1"], ["payments-api", "Payments", "Java", "1"],
               ["search", "Discovery", "Python", "2"], ["notifications", "Platform", "Go", "2"],
               ["reporting", "Data", "Python", "3"]]), []),
    ("ENG", "ADR-012 Use PostgreSQL for the ledger", "Architecture Decision Records", 1, 95, ["adr", "database"],
     "<h2>Context</h2>" + _p("The ledger needs strict transactions and a schema we can migrate safely.")
     + "<h2>Decision</h2>" + _p("We store the ledger in PostgreSQL 16 with logical replication to the "
                                "reporting cluster.")
     + "<h2>Consequences</h2>" + _ul("Migrations go through the schema review.",
                                     "Point-in-time recovery must be tested every quarter."), []),
    ("ENG", "Architecture Decision Records", "Engineering Home", 0, 150, ["adr"],
     _p("Short documents that capture an important decision, its context and its consequences. "
        "Copy the template, number it, and link it here."), []),
    ("ENG", "ADR-019 Feature flags for every risky change", "Architecture Decision Records", 3, 30,
     ["adr", "flags"],
     _p("Risky changes ship dark behind a feature flag and are enabled per customer cohort. Flags older "
        "than 90 days are reviewed at the monthly cleanup.",
        "This lets us decouple deploy from release: code can go out on a normal deploy and be released "
        "later with a flag flip."), []),
    ("ENG", "Local development setup", "Engineering Home", 4, 7, ["onboarding", "setup"],
     _p("Everything you need to run the stack on a laptop.")
     + _code("brew bundle\nmake bootstrap\nmake up   # starts postgres, redis and the services")
     + _panel("warning", "Do not point local services at production databases. Use the seeded fixtures.")
     + _p("If <code>make up</code> fails with a port conflict, stop other Postgres instances first."), []),
    ("ENG", "Incident review: checkout latency 2026-08-14", "Incident reviews", 2, 43,
     ["incident", "postmortem"],
     "<h2>Summary</h2>" + _p("Checkout p99 latency rose to 4s for 38 minutes after a connection pool "
                             "change. We rolled back with the blue green switch and latency recovered.")
     + "<h2>Timeline</h2>" + _table(["Time (UTC)", "Event"],
                                    [["10:02", "Deploy to green"], ["10:12", "Traffic flipped"],
                                     ["10:19", "Latency alert"], ["10:50", "Rolled back to blue"]])
     + '<p><img class="confluence-embedded-image" src="/download/attachments/1009/latency.png" '
       'alt="latency graph" width="480"></p>'
     + "<h2>Action items</h2>" + _ul("Load-test pool changes before deploy",
                                     "Alert on pool saturation, not only latency"), ["latency.png"]),
    ("ENG", "Incident reviews", "Engineering Home", 5, 120, ["incident"],
     _p("Blameless reviews of every customer-facing incident. Write one within 5 working days."), []),
    ("OPS", "Operations Home", None, 5, 210, ["home"],
     _p("On-call, monitoring, capacity and the runbooks the operations team maintains."), []),
    ("OPS", "On-call handbook", "Operations Home", 5, 16, ["oncall", "runbook"],
     _p("You are on call for one week, Monday to Monday. Acknowledge pages within 5 minutes.",
        "Escalate to the secondary after 15 minutes without progress. Nobody is expected to fix "
        "everything alone.")
     + _ul("Page: acknowledge, open an incident channel", "Mitigate first, investigate later",
           "Hand over with notes in the on-call log"), []),
    ("OPS", "Database failover runbook", "Operations Home", 1, 60, ["runbook", "database"],
     _p("Promote the replica when the primary is unreachable for more than 2 minutes.")
     + _code("pgctl promote --cluster ledger --replica ledger-2\npgctl status ledger", "bash")
     + _panel("note", "Failover resets connection pools; expect a short error spike in checkout."), []),
    ("OPS", "Capacity planning 2026", "Operations Home", 3, 75, ["capacity", "planning"],
     _p("Projected traffic grows 40% year over year. The green cluster is sized for peak plus 30% "
        "headroom so a blue green switch never runs out of capacity.")
     + '<p><img class="confluence-embedded-image" src="/download/attachments/1014/capacity.png" '
       'alt="capacity forecast" width="480"></p>', ["capacity.png"]),
    ("OPS", "Monitoring and alerting", "Operations Home", 2, 22, ["monitoring"],
     _p("Alerts page a human only when a customer is affected. Everything else is a ticket.",
        "Dashboards: service health, SLO burn rate, deploy markers.")
     + _table(["Alert", "Threshold", "Pages?"],
              [["Error rate", "> 2% for 5m", "yes"], ["Latency p99", "> 1.5s for 10m", "yes"],
               ["Disk", "> 85%", "no (ticket)"]]), []),
    ("OPS", "Blue team exercise notes", "Operations Home", 4, 5, ["security", "exercise"],
     _p("The blue team defended against a simulated credential leak. Detection took 22 minutes.",
        "Follow-up: rotate the shared deploy key and move it into the vault."), []),
    ("TEAM", "Team Handbook", None, 0, 300, ["home"],
     _p("How we work together: rituals, tools, communication and expectations."), []),
    ("TEAM", "Working agreements", "Team Handbook", 0, 33, ["process"],
     _ul("Core hours 10:00-15:00 local time", "Pull requests reviewed within one working day",
         "Default to written decisions in Confluence", "Camera optional in meetings")
     + _p("Agreements are revisited every quarter at the retrospective."), []),
    ("TEAM", "Sprint rituals", "Team Handbook", 3, 18, ["process", "agile"],
     _p("Two-week sprints. Planning Monday morning, demo and retrospective on the last Friday.",
        "Standups are async in Slack unless something is blocked.")
     + _table(["Ritual", "When", "Length"],
              [["Planning", "Mon wk1", "60m"], ["Refinement", "Wed wk1", "45m"], ["Demo", "Fri wk2", "30m"],
               ["Retrospective", "Fri wk2", "45m"]]), []),
    ("TEAM", "Code review guidelines", "Team Handbook", 1, 9, ["process", "review"],
     _p("Review for correctness first, then clarity. Nitpicks are marked <code>nit:</code> and never "
        "block a merge.", "Small pull requests get reviewed faster - aim for under 400 lines.")
     + _code("git fetch origin && git rebase origin/main", "bash"), []),
    ("TEAM", "Release calendar", "Team Handbook", 2, 2, ["release", "deploy"],
     _p("Planned releases for the quarter. Deploys follow the blue green runbook; freeze weeks are marked.")
     + _table(["Week", "Release", "Notes"],
              [["36", "2026.09.1", "checkout redesign"], ["38", "2026.09.2", "payments retry"],
               ["40", "freeze", "quarter end"], ["42", "2026.10.1", "search ranking"]]), []),
    ("HR", "People & HR", None, 5, 400, ["home"],
     _p("Policies, benefits and the people processes. Ask in #people for anything not covered here."), []),
    ("HR", "Onboarding checklist", "People & HR", 5, 25, ["onboarding"],
     _ul("Laptop and accounts on day one", "Meet your buddy in week one",
         "Complete security training", "Ship a small change in your first two weeks")
     + _p("Engineers also follow the Local development setup page in the Engineering space."), []),
    ("HR", "Leave policy", "People & HR", 5, 180, ["policy", "leave"],
     _p("Everyone gets 25 days of annual leave plus public holidays. Book leave in the HR system at "
        "least two weeks ahead for anything longer than three days."), []),
    ("HR", "Expense policy", "People & HR", 0, 140, ["policy", "expenses"],
     _p("Submit expenses within 30 days with a receipt. Home office equipment up to 500 per year is "
        "covered.")
     + _table(["Category", "Limit"], [["Travel", "actuals"], ["Meals while travelling", "60/day"],
                                      ["Home office", "500/year"]]), []),
]

MEETING_TOPICS = ["checkout redesign", "payments retry", "search ranking", "on-call load",
                  "deploy pipeline speed", "database upgrade", "green cluster capacity", "flaky tests"]
RETRO_ITEMS = ["the deploy pipeline was slow", "reviews took too long", "on-call was quiet",
               "the blue green switch worked well", "we under-estimated the migration",
               "pairing helped new joiners", "too many meetings", "flaky tests blocked merges"]


def _generated():
    """Meeting notes, retros and blog posts - deterministic filler that still
    reads like a team's space (and gives search something to rank)."""
    r = random.Random(42)
    out = []
    for i in range(14):
        space = ["ENG", "OPS", "TEAM"][i % 3]
        topic = MEETING_TOPICS[i % len(MEETING_TOPICS)]
        day = 4 + i * 9
        date = (TODAY - dt.timedelta(days=day)).strftime("%Y-%m-%d")
        att = r.sample(AUTHORS, 3)
        body = (_p(f"Attendees: {', '.join(att)}", f"Goal: agree next steps on the {topic}.")
                + "<h2>Discussion</h2>" + _ul(*[f"{x.capitalize()}." for x in r.sample(RETRO_ITEMS, 3)])
                + "<h2>Action items</h2>" + _ul(f"{att[0]}: write up the {topic} proposal",
                                                f"{att[1]}: check the deploy calendar"))
        out.append((space, f"{date} Meeting notes: {topic}", "Meeting notes", i % len(AUTHORS), day,
                    ["meeting-notes"], body, []))
    for i in range(8):
        space = ["ENG", "TEAM"][i % 2]
        day = 14 * i + 6
        good, bad = r.sample(RETRO_ITEMS, 2)
        out.append((space, f"Sprint {60 - i} retrospective", "Retrospectives", (i + 2) % len(AUTHORS), day,
                    ["retro"], "<h2>What went well</h2>" + _p(good.capitalize() + ".")
                    + "<h2>What to improve</h2>" + _p(bad.capitalize() + ".")
                    + "<h2>Actions</h2>" + _ul("Timebox reviews to one day", "Track pipeline duration"), []))
    for space in SPACES:
        out.append((space, "Meeting notes", f"{SPACES[space]}" if space != "TEAM" else "Team Handbook",
                    0, 250, [], _p("All meeting notes, newest first."), []))
        if space in ("ENG", "TEAM"):
            out.append((space, "Retrospectives", "Engineering Home" if space == "ENG" else "Team Handbook",
                        1, 260, [], _p("Every sprint retrospective."), []))
    return out


BLOGS = [
    ("ENG", "How we cut deploy time in half", 2, 11, ["blog", "deploy"],
     _p("Our deploy pipeline took 42 minutes. Caching build layers and running tests in parallel "
        "brought it to 19. The blue green switch itself takes under a minute.")),
    ("OPS", "A quieter on-call", 5, 20, ["blog", "oncall"],
     _p("Pages per week dropped from 31 to 9 after we moved non-customer alerts to tickets.")),
    ("TEAM", "Welcome our new joiners", 0, 8, ["blog"],
     _p("Please welcome three new engineers to the payments team this month.")),
    ("HR", "Benefits update for next year", 5, 15, ["blog", "benefits"],
     _p("Health cover now includes dental, and the home office budget rises to 500.")),
]


def _slug(t):
    return urllib.parse.quote_plus(re.sub(r"\s+", " ", t).strip())


class Page:
    __slots__ = ("id", "type", "space", "title", "parent", "author", "modified", "created", "labels",
                 "body", "images", "text", "contributors")

    def webui(self):
        if self.type == "blogpost":
            return f"/spaces/{self.space}/blog/{self.modified:%Y/%m/%d}/{self.id}/{_slug(self.title)}"
        if self.type == "attachment":
            return f"/download/attachments/{self.parent}/{urllib.parse.quote(self.title)}"
        return f"/spaces/{self.space}/pages/{self.id}/{_slug(self.title)}"


def _build():
    pages: dict[str, Page] = {}
    by_title = {}
    rows = [(s, t, par, a, d, lb, b, im, "page") for s, t, par, a, d, lb, b, im in HAND + _generated()]
    rows += [(s, t, None, a, d, lb, b, [], "blogpost") for s, t, a, d, lb, b in BLOGS]
    nid = 1001
    for s, t, par, a, d, lb, b, im, typ in rows:
        p = Page()
        p.id, p.type, p.space, p.title, p.parent = str(nid), typ, s, t, par
        p.author = AUTHORS[a % len(AUTHORS)]
        p.modified = TODAY - dt.timedelta(days=d, hours=nid % 7)
        p.created = p.modified - dt.timedelta(days=30 + nid % 50)
        p.labels, p.body, p.images = lb, b, im
        p.contributors = {p.author, AUTHORS[(a + 1) % len(AUTHORS)]}
        pages[p.id] = p
        by_title[(s, t)] = p
        nid += 1
    # image srcs name their page by position in HAND; point them at the real ids
    for p in list(pages.values()):
        p.body = re.sub(r"/download/attachments/\d+/", f"/download/attachments/{p.id}/", p.body)
        for name in p.images:
            a = Page()
            a.id, a.type, a.space, a.title, a.parent = str(nid), "attachment", p.space, name, p.id
            a.author, a.modified, a.created, a.labels = p.author, p.modified, p.modified, []
            a.body, a.images, a.contributors = "", [], {p.author}
            pages[a.id] = a
            nid += 1
    for p in pages.values():
        if p.type == "page" and p.parent:
            par = by_title.get((p.space, p.parent))
            p.parent = par.id if par else None
        p.text = _plain(p.body) if p.type != "attachment" else p.title
    return pages


def _plain(h):
    h = re.sub(r"<(br|/p|/li|/h\d|/tr|/pre)[^>]*>", "\n", h)
    return re.sub(r"[ \t]+", " ", html.unescape(re.sub(r"<[^>]+>", " ", h))).strip()


PAGES = _build()
# what "Fake User" saved for later in Confluence itself (favourite = currentUser())
SAVED = {p.id for p in PAGES.values() if p.title in (
    "Blue green deploy runbook", "On-call handbook", "Working agreements", "Leave policy")}


def ancestors(p):
    out, seen = [], set()
    q = PAGES.get(p.parent) if p.type == "page" and p.parent else None
    while q and q.id not in seen:
        seen.add(q.id)
        out.insert(0, q)
        q = PAGES.get(q.parent) if q.parent else None
    return out


# ------------------------------------------------------------------- CQL

class CQLError(Exception):
    pass


TOKEN_RE = re.compile(r'\s*(?:(?P<str>"(?:\\.|[^"\\])*")|(?P<op>!~|!=|>=|<=|~|=|>|<|\(|\)|,)|'
                      r'(?P<word>[^\s()~=!<>,"]+))')


def tokenize(s):
    out, i = [], 0
    s = s.strip()
    while i < len(s):
        m = TOKEN_RE.match(s, i)
        if not m or m.end() == i:
            raise CQLError(f"could not parse CQL near: {s[i:i + 20]!r}")
        i = m.end()
        if m.group("str") is not None:
            raw = m.group("str")[1:-1]
            out.append(("str", re.sub(r'\\(.)', r"\1", raw)))
        elif m.group("op"):
            out.append(("op", m.group("op")))
        elif m.group("word"):
            out.append(("word", m.group("word")))
    return out


class Parser:
    def __init__(self, cql):
        self.t = tokenize(cql)
        self.i = 0

    def peek(self, kind=None, val=None):
        if self.i >= len(self.t):
            return None
        k, v = self.t[self.i]
        if kind and k != kind:
            return None
        if val and (v.upper() if k == "word" else v) != val:
            return None
        return v

    def take(self, kind=None, val=None):
        v = self.peek(kind, val)
        if v is None:
            raise CQLError(f"expected {val or kind} at token {self.i}")
        self.i += 1
        return v

    def parse(self):
        expr = self.or_()
        order = None
        if self.peek("word", "ORDER"):
            self.take()
            self.take("word", "BY")
            field = self.take("word").lower()
            desc = True
            if self.peek("word", "ASC"):
                self.take()
                desc = False
            elif self.peek("word", "DESC"):
                self.take()
            order = (field, desc)
        if self.i != len(self.t):
            raise CQLError(f"unexpected token {self.t[self.i][1]!r}")
        return expr, order

    def or_(self):
        left = self.and_()
        while self.peek("word", "OR"):
            self.take()
            right = self.and_()
            left = ("or", left, right)
        return left

    def and_(self):
        left = self.not_()
        while self.peek("word", "AND"):
            self.take()
            right = self.not_()
            left = ("and", left, right)
        return left

    def not_(self):
        if self.peek("word", "NOT"):
            self.take()
            return ("not", self.not_())
        if self.peek("op", "("):
            self.take()
            e = self.or_()
            self.take("op", ")")
            return e
        return self.clause()

    def value(self):
        if self.peek("str") is not None:
            return self.take("str")
        w = self.take("word")
        if self.peek("op", "("):       # a function: now("-7d"), currentUser()
            self.take()
            args = []
            while not self.peek("op", ")"):
                args.append(self.value())
                if self.peek("op", ","):
                    self.take()
            self.take()
            return ("fn", w.lower(), args)
        return w

    def clause(self):
        field = self.take("word").lower()
        if self.peek("word", "NOT"):
            self.take()
            self.take("word", "IN")
            op = "not in"
        elif self.peek("word", "IN"):
            self.take()
            op = "in"
        else:
            op = self.take("op")
        if op in ("in", "not in"):
            self.take("op", "(")
            vals = []
            while not self.peek("op", ")"):
                vals.append(self.value())
                if self.peek("op", ","):
                    self.take()
            self.take()
            return ("clause", field, op, vals)
        return ("clause", field, op, self.value())


SUFFIXES = ("", "s", "es", "ed", "d", "ing", "ment", "ments", "er", "ers", "ly")


def _word_re(term):
    """One query word -> a regex over lowercase text: `deploy*` = prefix,
    else the word or a light stem of it (deploy ~ deploys/deployed/deploying)."""
    term = term.lower()
    if term.endswith("*"):
        return r"\b" + re.escape(term[:-1]) + r"\w*"
    base = term
    for suf in ("ments", "ment", "ing", "ers", "er", "ed", "es", "s"):
        if base.endswith(suf) and len(base) - len(suf) >= 3:
            base = base[:-len(suf)]
            break
    return r"\b" + re.escape(base) + r"(?:" + "|".join(SUFFIXES[1:]) + r")?\b"


def text_query(value):
    """A text ~ value -> [regex, ...] that must ALL match. An escaped inner
    "phrase" is one regex of consecutive words; bare words each match alone."""
    value = value.strip()
    if len(value) >= 2 and value[0] == '"' and value[-1] == '"':
        words = re.findall(r"[\w*'-]+", value[1:-1])
        return [r"\s+".join(_word_re(w).removeprefix(r"\b").removesuffix(r"\b") for w in words)
                .join([r"\b", r"\b"])] if words else []
    return [_word_re(w) for w in re.findall(r"[\w*'-]+", value)]


def _date(v):
    if isinstance(v, tuple) and v[0] == "fn" and v[1] == "now":
        arg = (v[2] or ["0d"])[0]
        m = re.fullmatch(r"([+-]?\d+)([dwmyh])", str(arg).strip())
        if not m:
            return TODAY
        n, unit = int(m.group(1)), m.group(2)
        days = {"d": 1, "w": 7, "m": 30, "y": 365, "h": 1 / 24}[unit] * n
        return dt.datetime.now(dt.timezone.utc) + dt.timedelta(days=days)
    return dt.datetime.fromisoformat(str(v)[:10]).replace(tzinfo=dt.timezone.utc)


def _is_me(v):
    return (isinstance(v, tuple) and v[1] == "currentuser") or \
        v in (CURRENT_USER["username"], CURRENT_USER["accountId"])


def matches(node, p):
    kind = node[0]
    if kind == "and":
        return matches(node[1], p) and matches(node[2], p)
    if kind == "or":
        return matches(node[1], p) or matches(node[2], p)
    if kind == "not":
        return not matches(node[1], p)
    _, field, op, val = node
    vals = val if isinstance(val, list) else [val]
    if field in ("text", "title", "siteSearch".lower()):
        hay = (p.title if field == "title" else p.title + "\n" + p.text).lower()
        hit = all(re.search(rx, hay) for rx in text_query(str(val)))
        return hit if op == "~" else not hit
    if field in ("space", "space.key", "type", "label", "id", "ancestor", "parent"):
        have = {"space": {p.space}, "space.key": {p.space}, "type": {p.type}, "label": set(p.labels),
                "id": {p.id}, "parent": {p.parent or ""},
                "ancestor": {a.id for a in ancestors(p)}}[field]
        want = {str(v).lower() if field in ("type", "label") else str(v) for v in vals}
        have = {h.lower() for h in have} if field in ("type", "label") else have
        hit = bool(have & want)
        return not hit if op in ("!=", "not in") else hit
    if field in ("favourite", "favorite"):
        hit = p.id in SAVED and any(_is_me(v) for v in vals)
        return not hit if op in ("!=", "not in") else hit
    if field in ("creator", "contributor"):
        who = {p.author} if field == "creator" else p.contributors
        hit = any((_is_me(v) and CURRENT_USER["displayName"] in who) or v in who for v in vals)
        return not hit if op in ("!=", "not in") else hit
    if field in ("lastmodified", "created"):
        d = p.modified if field == "lastmodified" else p.created
        t = _date(val)
        return {">=": d >= t, ">": d > t, "<=": d <= t, "<": d < t, "=": d.date() == t.date()}.get(op, False)
    raise CQLError(f"unsupported CQL field {field!r}")


def text_terms(node):
    """Every text/title ~ regex in the query (for ranking + highlights)."""
    if node[0] in ("and", "or"):
        return text_terms(node[1]) + text_terms(node[2])
    if node[0] == "not":
        return []
    if node[1] in ("text", "title", "sitesearch") and node[2] == "~":
        return text_query(str(node[3]))
    return []


def highlight(text, terms, width=None):
    """Wrap matches in @@@hl@@@...@@@endhl@@@; width = an excerpt window
    around the first match (Confluence's excerpt=highlight)."""
    spans = []
    for rx in terms:
        spans += [m.span() for m in re.finditer(rx, text, re.I)]
    spans.sort()
    if width:
        start = max(0, spans[0][0] - width // 3) if spans else 0
        end = min(len(text), start + width)
        # snap to word boundaries
        if start:
            start = text.find(" ", start) + 1 or start
        text_w = text[start:end]
        spans = [(a - start, b - start) for a, b in spans if a >= start and b <= end]
        text = ("…" if start else "") + text_w + ("…" if end < len(text) else "")
        off = 1 if start else 0
        spans = [(a + off, b + off) for a, b in spans]
    out, last = [], 0
    for a, b in spans:
        if a < last:
            continue
        out.append(text[last:a] + "@@@hl@@@" + text[a:b] + "@@@endhl@@@")
        last = b
    out.append(text[last:])
    return "".join(out).replace("\n", " ")


def search(cql):
    expr, order = Parser(cql).parse()
    hits = [p for p in PAGES.values() if matches(expr, p)]
    terms = text_terms(expr)
    if order:
        key = {"lastmodified": lambda p: p.modified, "created": lambda p: p.created,
               "title": lambda p: p.title.lower()}.get(order[0], lambda p: p.modified)
        hits.sort(key=key, reverse=order[1])
    else:
        def score(p):
            return (sum(5 * len(re.findall(rx, p.title, re.I)) + len(re.findall(rx, p.text, re.I))
                        for rx in terms), p.modified)
        hits.sort(key=score, reverse=True)
    return hits, terms


# ----------------------------------------------------------------- JSON

def iso(d):
    return d.strftime("%Y-%m-%dT%H:%M:%S.000Z")


def user_json(name):
    return {"type": "known", "displayName": name, "username": name.split()[0].lower(),
            "accountId": "acc-" + str(abs(hash(name)) % 100000)}


def content_json(p, base, expand=()):
    j = {"id": p.id, "type": p.type, "status": "current", "title": p.title,
         "space": {"key": p.space, "name": SPACES[p.space], "_links": {"webui": f"/spaces/{p.space}"}},
         "version": {"number": 1 + int(p.id) % 5, "when": iso(p.modified), "by": user_json(p.author)},
         "history": {"createdDate": iso(p.created), "createdBy": user_json(p.author)},
         "metadata": {"labels": {"results": [{"name": x} for x in p.labels]}},
         "_links": {"webui": p.webui(), "self": f"{base}/rest/api/content/{p.id}"}}
    if p.type == "attachment":
        j["_links"]["download"] = p.webui()
        j["metadata"]["mediaType"] = "image/png"
        j["container"] = {"id": p.parent, "title": PAGES[p.parent].title}
    if "ancestors" in expand:
        j["ancestors"] = [{"id": a.id, "type": "page", "title": a.title,
                           "_links": {"webui": a.webui()}} for a in ancestors(p)]
    if "body.view" in expand:
        j["body"] = {"view": {"value": p.body, "representation": "view"}}
    if "body.storage" in expand:
        j["body"] = {"storage": {"value": p.body, "representation": "storage"}}
    return j


def search_json(cql, qs, base, context, excerpts=True):
    hits, terms = search(cql)
    start = int(qs.get("start", ["0"])[0] or 0)
    limit = max(1, min(100, int(qs.get("limit", ["25"])[0] or 25)))
    page = hits[start:start + limit]
    expand = qs.get("expand", [""])[0]
    results = []
    for p in page:
        c = content_json(p, base, [x.replace("content.", "") for x in expand.split(",")])
        if not excerpts:
            results.append(c)
            continue
        results.append({
            "content": c,
            "title": highlight(p.title, terms),
            "excerpt": highlight(p.text, terms, width=220),
            "url": p.webui(),
            "resultGlobalContainer": {"title": SPACES[p.space], "displayUrl": f"/spaces/{p.space}"},
            "breadcrumbs": [], "entityType": "content", "iconCssClass": "aui-icon content-type-" + p.type,
            "lastModified": iso(p.modified), "friendlyLastModified": p.modified.strftime("%b %d, %Y"),
            "score": 1.0})
    out = {"results": results, "start": start, "limit": limit, "size": len(results),
           "totalSize": len(hits), "cqlQuery": cql, "searchDuration": 7,
           "_links": {"base": base, "context": context}}
    if start + limit < len(hits):
        nq = urllib.parse.urlencode({"cql": cql, "start": start + limit, "limit": limit,
                                     **({"expand": expand} if expand else {}),
                                     **({"excerpt": "highlight"} if excerpts else {})})
        path = "/rest/api/search" if excerpts else "/rest/api/content/search"
        out["_links"]["next"] = f"{path}?{nq}"
    return out


# ------------------------------------------------------------------- PNGs

def png(w, h, pixel):
    raw = b"".join(b"\x00" + b"".join(bytes(pixel(x, y)) for x in range(w)) for y in range(h))

    def chunk(t, d):
        return struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xffffffff)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw, 6)) + chunk(b"IEND", b""))


def image(name):
    """A small chart per attachment name (bars / a line / two boxes)."""
    seed = sum(map(ord, name))
    r = random.Random(seed)
    w, h = 480, 220
    bg, grid = (246, 247, 251), (225, 228, 238)
    blue, green, red = (66, 120, 230), (70, 180, 110), (225, 90, 90)
    if "switch" in name:
        def boxes(x, y):
            if 40 <= y <= 180 and (60 <= x <= 200 or 280 <= x <= 420):
                edge = y in (40, 180) or x in (60, 200, 280, 420)
                return (30, 40, 60) if edge else (blue if x < 240 else green)
            if 105 <= y <= 115 and 200 < x < 280:
                return (30, 40, 60)
            return bg
        return png(w, h, boxes)
    vals = [r.randint(40, 180) for _ in range(12)]
    if "latency" in name:
        vals[6], vals[7] = 205, 200

    def px(x, y):
        if y % 40 == 0:
            return grid
        i = x * len(vals) // w
        top = h - vals[i]
        if y >= top and x % (w // len(vals)) > 6:
            return red if vals[i] > 190 else (green if "capacity" in name and i > 7 else blue)
        return bg
    return png(w, h, px)


# --------------------------------------------------------------- handle()

_COUNTS: dict = {}


def _counter(name):
    d = os.environ.get("FAKE_DIR")
    if not d:
        n = _COUNTS.get(name, 0)
        _COUNTS[name] = n + 1
        return n
    p = os.path.join(d, "fakeconf-" + name)
    n = int(open(p).read()) if os.path.exists(p) else 0
    with open(p, "w") as fh:
        fh.write(str(n + 1))
    return n


def _authed(headers):
    tok = os.environ.get("FAKE_CONF_TOKEN", "fake-token")
    a = next((v for k, v in headers.items() if k.lower() == "authorization"), "")
    if a.startswith("Bearer "):
        return a[7:] == tok
    if a.startswith("Basic "):
        try:
            return base64.b64decode(a[6:]).decode().partition(":")[2] == tok
        except ValueError:
            return False
    return False


def _json(code, body, extra=None):
    return code, {"Content-Type": "application/json", **(extra or {})}, json.dumps(body).encode()


def _html(code, body):
    return code, {"Content-Type": "text/html; charset=utf-8"}, body.encode()


def handle(method: str, url: str, headers: dict, context: str | None = None):
    """-> (status, headers, body bytes). `context` = the served prefix
    (/wiki); None accepts /wiki, /confluence or none (the curl shim)."""
    if os.environ.get("FAKE_CONF_SLOW_MS"):
        time.sleep(int(os.environ["FAKE_CONF_SLOW_MS"]) / 1000)
    u = urllib.parse.urlsplit(url)
    path = urllib.parse.unquote(u.path)
    ctx = context
    if ctx is None:
        ctx = next((c for c in ("/wiki", "/confluence") if path == c or path.startswith(c + "/")), "")
    if ctx and not (path == ctx or path.startswith(ctx + "/")):
        return _json(404, {"message": f"not under {ctx}"})
    path = path[len(ctx):] or "/"
    host = headers.get("Host") or u.netloc or "127.0.0.1"
    base = f"{u.scheme or 'http'}://{host}{ctx}"
    qs = urllib.parse.parse_qs(u.query)
    if method not in ("GET", "HEAD"):
        return _json(405, {"message": "the fake is read-only"})

    # the browsable site (no auth: it's what "open in browser" lands on)
    if path in ("/", "") or path.startswith("/spaces/"):
        return _html(200, site_page(path, ctx))

    authed = _authed(headers)
    if os.environ.get("FAKE_CONF_401"):
        authed = False
    if path.startswith("/download/attachments/"):
        if not authed:
            return _json(401, {"message": "Unauthorized"})
        name = path.rsplit("/", 1)[-1]
        return 200, {"Content-Type": "image/png"}, image(name)
    if path == "/rest/api/user/current":
        if not authed:
            if os.environ.get("FAKE_CONF_401"):
                return _json(401, {"message": "Client must be authenticated"})
            return _json(200, {"type": "anonymous", "displayName": "Anonymous"})
        return _json(200, {"type": "known", **CURRENT_USER})
    if not authed:
        return _json(401, {"message": "Client must be authenticated to access this resource.",
                           "statusCode": 401})
    m = re.fullmatch(r"/rest/api/space/([^/]+)", path)
    if m:
        k = m.group(1)
        if k not in SPACES:
            return _json(404, {"statusCode": 404, "message": f"No space with key : {k}"})
        return _json(200, {"id": 100 + list(SPACES).index(k), "key": k, "name": SPACES[k], "type": "global",
                           "_links": {"webui": f"/spaces/{k}", "base": base}})
    m = re.fullmatch(r"/rest/api/content/(\d+)", path)
    if m:
        p = PAGES.get(m.group(1))
        if not p:
            return _json(404, {"statusCode": 404, "message": "No content found with id"})
        exp = qs.get("expand", [""])[0].split(",")
        j = content_json(p, base, exp)
        j["_links"]["base"] = base
        if ctx and "body" in j:
            # like the real thing: links + images carry the context path
            for rep in j["body"].values():
                rep["value"] = re.sub(r'(src|href)="/(download|spaces)/', rf'\1="{ctx}/\2/', rep["value"])
        return _json(200, j)
    if path in ("/rest/api/search", "/rest/api/content/search"):
        if path == "/rest/api/search" and os.environ.get("FAKE_CONF_NO_SEARCH"):
            return _html(404, "<html><body>Page Not Found</body></html>")
        n429 = int(os.environ.get("FAKE_CONF_429", "0") or 0)
        if n429 and _counter("429") < n429:
            return _json(429, {"message": "Rate limit exceeded"}, {"Retry-After": "0"})
        cql = qs.get("cql", [""])[0]
        if not cql.strip():
            return _json(400, {"statusCode": 400, "message": "cql is required"})
        try:
            return _json(200, search_json(cql, qs, base, ctx, excerpts=path == "/rest/api/search"))
        except CQLError as e:
            return _json(400, {"statusCode": 400, "message": f"Could not parse cql : {cql} ({e})"})
    return _json(404, {"statusCode": 404, "message": f"no fake for {path}"})


# ------------------------------------------------------- the browsable site

CSS = """body{font:15px/1.55 -apple-system,Segoe UI,sans-serif;margin:0;color:#172b4d;background:#fff}
header{background:#0747a6;color:#fff;padding:10px 24px;font-weight:600}header a{color:#fff}
main{max-width:860px;margin:24px auto;padding:0 24px}.crumbs{color:#6b778c;font-size:13px}
table{border-collapse:collapse}td,th{border:1px solid #dfe1e6;padding:6px 10px}th{background:#f4f5f7}
pre{background:#f4f5f7;padding:12px;border-radius:4px;overflow:auto}
.confluence-information-macro{border-left:4px solid #0065ff;background:#deebff;padding:4px 14px;margin:12px 0}
.confluence-information-macro-warning{border-color:#ff991f;background:#fffae6}
.meta{color:#6b778c;font-size:13px}img{max-width:100%}.banner{background:#fffae6;padding:6px 24px;
font-size:13px;color:#172b4d}"""


def site_page(path, ctx):
    head = (f"<!doctype html><meta charset=utf-8><style>{CSS}</style><header><a href='{ctx}/'>Fake "
            f"Confluence</a></header><div class=banner>A local fake (confluence/fake_confluence.py) - not a "
            f"real site.</div><main>")
    m = re.match(r"/spaces/([^/]+)/(?:pages|blog/\d+/\d+/\d+)/(\d+)", path)
    if m and m.group(2) in PAGES:
        p = PAGES[m.group(2)]
        crumbs = " / ".join([f"<a href='{ctx}/spaces/{p.space}'>{SPACES[p.space]}</a>"]
                            + [f"<a href='{ctx}{a.webui()}'>{html.escape(a.title)}</a>" for a in ancestors(p)])
        body = re.sub(r'src="/download/', f'src="{ctx}/download/', p.body)
        # images need auth on the API; the browsable site inlines them
        body = re.sub(r'src="[^"]*/download/attachments/\d+/([^"]+)"',
                      lambda mm: 'src="data:image/png;base64,' + base64.b64encode(image(mm.group(1))).decode()
                      + '"', body)
        body = re.sub(r'href="/spaces/', f'href="{ctx}/spaces/', body)
        return (head + f"<div class=crumbs>{crumbs}</div><h1>{html.escape(p.title)}</h1>"
                f"<div class=meta>{p.author} · updated {p.modified:%b %d, %Y}</div>{body}</main>")
    m = re.match(r"/spaces/([^/]+)", path)
    keys = [m.group(1)] if m and m.group(1) in SPACES else list(SPACES)
    out = [head]
    for k in keys:
        out.append(f"<h2>{SPACES[k]} <span class=meta>({k})</span></h2><ul>")
        for p in sorted((p for p in PAGES.values() if p.space == k and p.type != "attachment"),
                        key=lambda p: p.title):
            out.append(f"<li><a href='{ctx}{p.webui()}'>{html.escape(p.title)}</a></li>")
        out.append("</ul>")
    return "".join(out) + "</main>"


# ------------------------------------------------------------------ server

def serve(port: int, context: str):
    from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

    class H(BaseHTTPRequestHandler):
        def do_GET(self):
            code, hdrs, body = handle("GET", self.path, dict(self.headers.items()), context)
            self.send_response(code)
            for k, v in hdrs.items():
                self.send_header(k, v)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, format, *args):  # noqa: A002 (the base's name)
            sys.stderr.write("fake-confluence: " + (format % args) + "\n")

    srv = ThreadingHTTPServer(("127.0.0.1", port), H)
    print(f"fake Confluence: http://127.0.0.1:{port}{context}  (token: "
          f"{os.environ.get('FAKE_CONF_TOKEN', 'fake-token')}; {len(PAGES)} items in "
          f"{', '.join(SPACES)})", file=sys.stderr, flush=True)
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        pass


def curl_shim(argv):
    """Behave like the `curl` Client.get runs: -H / -u / -D / -w, URL last;
    prints the body + "\\n<code> <retry-after>"."""
    headers, url, dump = {}, argv[-1], None
    i = 0
    while i < len(argv) - 1:
        a = argv[i]
        if a == "-H":
            k, _, v = argv[i + 1].partition(":")
            headers[k.strip()] = v.strip()
            i += 1
        elif a == "-u":
            headers["Authorization"] = "Basic " + base64.b64encode(argv[i + 1].encode()).decode()
            i += 1
        elif a == "-D":
            dump = argv[i + 1]
            i += 1
        elif a in ("-X", "-m", "-w", "--data-raw"):
            i += 1
        i += 1
    if d := os.environ.get("FAKE_DIR"):
        with open(os.path.join(d, "calls.jsonl"), "a") as fh:
            fh.write(json.dumps(argv) + "\n")
    code, hdrs, body = handle("GET", url, headers)
    if dump:
        with open(dump, "w") as fh:
            fh.write(f"HTTP/1.1 {code} X\r\n" + "".join(f"{k}: {v}\r\n" for k, v in hdrs.items()) + "\r\n")
    sys.stdout.write(body.decode("utf-8", "replace") + f"\n{code} {hdrs.get('Retry-After', '')}")


def main(argv):
    if argv[:1] == ["serve"]:
        port, context = 8765, "/wiki"
        if "--port" in argv:
            port = int(argv[argv.index("--port") + 1])
        if "--context" in argv:
            context = argv[argv.index("--context") + 1].rstrip("/")
        serve(port, context)
    elif argv[:1] == ["curl"]:
        curl_shim(argv[1:])
    else:
        print(__doc__, file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
