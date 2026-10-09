#!/usr/bin/env python3
"""fake_jira.py - a small, deterministic Jira Server/DC for development.

    python3 fake_jira.py serve [--port 8766] [--seed 7] [--issues 40]

A real HTTP server on 127.0.0.1 (bin: `./ws fake jira-site start|stop`) the
poller and the Jira window can talk to by hand. Faker makes the titles and
people when installed (seeded); without it a word list does.

Projects DEMO, WEB, OPS. Per project: one KANBAN board and one SCRUM board
(company-managed; the scrum board has PAST_SPRINTS closed sprints, one
active and one future). OPS also has a team-managed ("simple") board with
Sprints switched off - like a fresh Jira Cloud project: its /sprint answers
400 "The board does not support sprints". Real Jira's discovery order is the
one the app follows: project -> boards -> sprints.

REST (Bearer or basic auth with any token; token = $FAKE_JIRA_TOKEN, default
"fake-token"):
  /rest/api/2/myself  /project  /project/KEY  /project/KEY/versions
  /rest/api/2/status  /field  /priority  /issuetype  /user/assignable/search
  /rest/api/2/search?jql=&startAt=&maxResults=&fields=
  /rest/api/2/filter/ID               (a board's saved filter: its JQL)
  /rest/agile/1.0/board?projectKeyOrId=KEY
  /rest/agile/1.0/board/ID/configuration   (columns -> status ids, filter)
  /rest/agile/1.0/board/ID/quickfilter
  /rest/agile/1.0/board/ID/sprint?state=active,future,closed&startAt=&maxResults=
                         (scrum boards only, paged; kanban / sprints off -> 400)
  /rest/agile/1.0/board/ID/features   (jsw.agility.sprints ENABLED / DISABLED)
  /rest/agile/1.0/board/ID/issue  /sprint/ID  /sprint/ID/issue
JQL understood: project in/=, key in, sprint = N, sprint in openSprints() /
closedSprints() / futureSprints(),
updated/created >=, labels is not EMPTY, fixVersion =, assignee =, status =,
AND, (...) groups, ORDER BY.
"""
from __future__ import annotations

import base64
import datetime as dt
import json
import os
import random
import re
import sys
import urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

STATUSES = [  # id, name, category key
    (1, "To Do", "new"), (2, "In Progress", "indeterminate"), (3, "In Review", "indeterminate"),
    (4, "Blocked", "indeterminate"), (5, "Done", "done")]
PRIORITIES = ["Highest", "High", "Medium", "Low", "Lowest"]
TYPES = ["Story", "Bug", "Task", "Epic"]
LABELS = ["backend", "frontend", "api", "infra", "ux", "security", "perf", "docs"]
WORDS = "cache queue login export report import search sync upload webhook retry timeout index".split()
PROJECTS = {"DEMO": "Demo Platform", "WEB": "Web Frontend", "OPS": "Operations"}
NOW = dt.datetime(2026, 10, 1, 12, 0, 0)
PAST_SPRINTS = 6           # closed sprints per scrum board (newest = just before the active one)


def iso(t: dt.datetime) -> str:
    return t.strftime("%Y-%m-%dT%H:%M:%S.000+0000")


class World:
    def __init__(self, seed: int = 7, per_project: int = 40):
        rnd = random.Random(seed)
        try:
            from faker import Faker
            fk = Faker()
            Faker.seed(seed)
            people = [fk.name() for _ in range(8)]
            title = lambda: f"{rnd.choice(['Fix', 'Add', 'Improve', 'Investigate'])} {fk.bs()}"
        except ImportError:
            people = ["Alex Chen", "Priya Natarajan", "Sam Okafor", "Jordan Lee", "Maria Rossi", "Kim Park"]
            title = lambda: f"{rnd.choice(['Fix', 'Add', 'Improve', 'Investigate'])} {rnd.choice(WORDS)} {rnd.choice(WORDS)}"
        self.users = [{"name": p.lower().replace(" ", "."), "displayName": p} for p in people]
        self.boards, self.sprints, self.issues = [], {}, []
        bid = sid = 0
        for pk, pname in PROJECTS.items():
            bid += 1
            kanban = {"id": bid, "name": f"{pk} Kanban", "type": "kanban", "project": pk, "filter": 100 + bid}
            bid += 1
            scrum = {"id": bid, "name": f"{pk} Scrum", "type": "scrum", "project": pk, "filter": 100 + bid}
            self.boards += [kanban, scrum]
            sp = []
            states = ["closed"] * PAST_SPRINTS + ["active", "future"]
            for n, state in enumerate(states, 1):
                sid += 1
                start = NOW + dt.timedelta(days=14 * (n - PAST_SPRINTS - 1) - 3)
                s = {"id": sid, "name": f"{pk} Sprint {n}", "state": state, "originBoardId": scrum["id"],
                     "goal": f"Sprint {n} goal for {pname}"}
                if state != "future":
                    s.update({"startDate": iso(start), "endDate": iso(start + dt.timedelta(days=14))})
                if state == "closed":
                    s["completeDate"] = iso(start + dt.timedelta(days=14, hours=2))
                sp.append(s)
            self.sprints[scrum["id"]] = sp
            for i in range(1, per_project + 1):
                sprint = rnd.choice(sp + [None, None])
                st = rnd.choice(STATUSES)
                if sprint and sprint["state"] == "closed":
                    st = STATUSES[4]
                if sprint and sprint["state"] == "future":
                    st = STATUSES[0]
                ttl = title()
                created = NOW - dt.timedelta(hours=rnd.randint(1, 24 * 60))
                self.issues.append({
                    "key": f"{pk}-{i}", "id": str(len(self.issues) + 1000), "project": pk,
                    "sprint": sprint["id"] if sprint else None,
                    "fields": {
                        "summary": ttl[:1].upper() + ttl[1:],
                        "status": {"id": str(st[0]), "name": st[1], "statusCategory": {"key": st[2]}},
                        "priority": {"name": rnd.choice(PRIORITIES)},
                        "issuetype": {"name": rnd.choice(TYPES)},
                        "assignee": rnd.choice(self.users + [None]),
                        "reporter": rnd.choice(self.users),
                        "labels": rnd.sample(LABELS, rnd.randint(0, 2)),
                        "created": iso(created), "updated": iso(created + dt.timedelta(hours=rnd.randint(0, 200))),
                        "project": {"key": pk, "name": pname},
                        "description": f"Details for {pk}-{i}.",
                        "comment": {"total": 0, "comments": []},
                        "fixVersions": [],
                    }})
        # OPS: a team-managed board with Sprints off (Jira Cloud's default)
        bid += 1
        self.boards.append({"id": bid, "name": "OPS board", "type": "simple", "project": "OPS",
                            "filter": 100 + bid, "sprintsOff": True})

    # ---- JQL: parse into a predicate over an issue
    def predicate(self, jql: str):
        jql = re.sub(r"\s+ORDER\s+BY\s+.*$", "", jql or "", flags=re.I | re.S).strip()
        toks = re.findall(r'\(|\)|"(?:[^"\\]|\\.)*"|[^\s()]+', jql)
        pos = [0]

        def peek():
            return toks[pos[0]] if pos[0] < len(toks) else None

        def take():
            pos[0] += 1
            return toks[pos[0] - 1]

        def unq(s):
            return s[1:-1].replace('\\"', '"') if s.startswith('"') else s

        def listing():          # ( a, b, ... )
            take()
            out = []
            while peek() not in (")", None):
                t = take()
                if t != ",":
                    out += [unq(x) for x in t.split(",") if x]
            take()
            return out

        def atom():
            t = peek()
            if t == "(":
                take()
                p = orexpr()
                if peek() == ")":
                    take()
                return p
            field = take().lower()
            op = take().lower()
            if op == "is":                       # is [not] EMPTY
                neg = peek().lower() == "not"
                if neg:
                    take()
                take()
                return lambda i: bool(i["fields"].get(field)) == neg
            if op == "not":
                op = "not " + take().lower()
            if op in ("in", "not in"):
                fn = (peek() or "").lower()
                if field == "sprint" and fn.split("(")[0] in ("opensprints", "closedsprints", "futuresprints"):
                    take()
                    if peek() == "(":
                        take()
                        take()
                    want = {"opensprints": "active", "closedsprints": "closed", "futuresprints": "future"}[fn.split("(")[0]]
                    ids = {s["id"] for ss in self.sprints.values() for s in ss if s["state"] == want}
                    neg = op == "not in"
                    return lambda i: (i["sprint"] in ids) != neg
                vals = listing()
                ok = lambda i: self.value(i, field) in vals
            else:
                v = unq(take())
                if field in ("updated", "created") and op in (">=", ">", "<=", "<"):
                    cmp = {">=": lambda a, b: a >= b, ">": lambda a, b: a > b,
                           "<=": lambda a, b: a <= b, "<": lambda a, b: a < b}[op]
                    ok = lambda i: cmp(i["fields"][field][:16].replace("T", " "), v[:16])
                elif op == "~":
                    ok = lambda i: v.lower() in (i["fields"]["summary"] + i["fields"]["description"]).lower()
                elif op == "!=":
                    ok = lambda i: str(self.value(i, field)) != v
                else:
                    ok = lambda i: str(self.value(i, field)) == v
            return (lambda i: not ok(i)) if op == "not in" else ok

        def andexpr():
            p = atom()
            while (peek() or "").lower() == "and":
                take()
                p = (lambda a, b: lambda i: a(i) and b(i))(p, atom())
            return p

        def orexpr():
            p = andexpr()
            while (peek() or "").lower() == "or":
                take()
                p = (lambda a, b: lambda i: a(i) or b(i))(p, andexpr())
            return p

        return orexpr() if toks else (lambda i: True)

    @staticmethod
    def value(i, field):
        f = i["fields"]
        if field == "project":
            return i["project"]
        if field == "key":
            return i["key"]
        if field == "sprint":
            return i["sprint"]
        if field == "status":
            return f["status"]["name"]
        if field == "assignee":
            return (f["assignee"] or {}).get("name", "")
        if field == "fixversion":
            return (f["fixVersions"] or [{}])[0].get("name", "")
        if field == "labels":
            return f["labels"]
        return f.get(field, "")

    def search(self, jql: str, start: int, n: int, fields: str, scope=None):
        pred = self.predicate(jql)
        rows = [i for i in self.issues if pred(i) and (scope is None or scope(i))]
        order = re.search(r"ORDER\s+BY\s+(\w+)\s*(ASC|DESC)?", jql or "", re.I)
        if order and order.group(1).lower() in ("updated", "created"):
            rows.sort(key=lambda i: i["fields"][order.group(1).lower()], reverse=(order.group(2) or "").upper() == "DESC")
        else:
            rows.sort(key=lambda i: (i["project"], int(i["key"].split("-")[1])))
        want = {f.strip() for f in fields.split(",") if f.strip()} if fields else None
        page = []
        for i in rows[start:start + n]:
            fl = i["fields"] if not want or "*all" in want else {k: v for k, v in i["fields"].items() if k in want}
            page.append({"key": i["key"], "id": i["id"], "fields": fl})
        return {"startAt": start, "maxResults": n, "total": len(rows), "issues": page}


def board_columns(b):
    if b["type"] == "kanban":
        groups = [("Backlog", [1]), ("In Progress", [2, 3, 4]), ("Done", [5])]
    else:
        groups = [("To Do", [1]), ("In Progress", [2, 3]), ("Blocked", [4]), ("Done", [5])]
    return [{"name": n, "statuses": [{"id": str(s)} for s in ids]} for n, ids in groups]


class Handler(BaseHTTPRequestHandler):
    world: World
    token = os.environ.get("FAKE_JIRA_TOKEN", "fake-token")

    def log_message(self, *a):
        pass

    def send(self, code, body):
        data = json.dumps(body).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def authed(self):
        a = self.headers.get("Authorization", "")
        if a.startswith("Bearer "):
            return a[7:] == self.token
        if a.startswith("Basic "):
            try:
                return base64.b64decode(a[6:]).decode().split(":", 1)[1] == self.token
            except Exception:
                return False
        return False

    def do_GET(self):
        u = urllib.parse.urlparse(self.path)
        q = {k: v[0] for k, v in urllib.parse.parse_qs(u.query).items()}
        w = self.world
        if not self.authed():
            return self.send(401, {"message": "Client must be authenticated to access this resource."})
        p = u.path
        start, n = int(q.get("startAt", 0)), int(q.get("maxResults", 50))
        if p.startswith("/rest/agile/1.0"):
            return self.agile(p[len("/rest/agile/1.0"):], q, start, n)
        if not p.startswith("/rest/api/2"):
            return self.send(404, {})
        p = p[len("/rest/api/2"):]
        if p == "/myself":
            return self.send(200, {"name": "fake", "displayName": "Fake User", "timeZone": "UTC"})
        if p == "/project":
            return self.send(200, [{"key": k, "name": v} for k, v in PROJECTS.items()])
        m = re.fullmatch(r"/project/(\w+)(/versions)?", p)
        if m:
            if m.group(1) not in PROJECTS:
                return self.send(404, {"errorMessages": ["No project could be found."]})
            return self.send(200, [] if m.group(2) else {"key": m.group(1), "name": PROJECTS[m.group(1)]})
        if p == "/status":
            return self.send(200, [{"id": str(i), "name": nme, "statusCategory": {"key": c}} for i, nme, c in STATUSES])
        if p == "/field":
            return self.send(200, [{"id": "summary", "name": "Summary", "custom": False, "schema": {"type": "string"}}])
        if p == "/priority":
            return self.send(200, [{"name": x} for x in PRIORITIES])
        if p == "/issuetype":
            return self.send(200, [{"name": x} for x in TYPES])
        if p == "/user/assignable/search":
            return self.send(200, w.users[start:start + n])
        if p == "/search":
            return self.send(200, w.search(q.get("jql", ""), start, n, q.get("fields", "")))
        m = re.fullmatch(r"/filter/(\d+)", p)
        if m:
            b = next((b for b in w.boards if b["filter"] == int(m.group(1))), None)
            if not b:
                return self.send(404, {})
            return self.send(200, {"id": m.group(1), "name": b["name"] + " filter",
                                   "jql": f'project = "{b["project"]}" ORDER BY Rank ASC'})
        m = re.fullmatch(r"/issue/([\w-]+)", p)
        if m:
            i = next((i for i in w.issues if i["key"] == m.group(1)), None)
            return self.send(200, {"key": i["key"], "fields": i["fields"]}) if i else self.send(404, {})
        return self.send(404, {"errorMessages": [f"no fake for {p}"]})

    def agile(self, p, q, start, n):
        w = self.world
        if p == "/board":
            pk = q.get("projectKeyOrId")
            vals = [{"id": b["id"], "name": b["name"], "type": b["type"],
                     "location": {"projectKey": b["project"], "displayName": PROJECTS[b["project"]]}}
                    for b in w.boards if not pk or b["project"] == pk]
            return self.send(200, {"startAt": start, "maxResults": n, "total": len(vals),
                                   "isLast": start + n >= len(vals), "values": vals[start:start + n]})
        m = re.fullmatch(r"/board/(\d+)/(\w+)", p)
        if m:
            b = next((b for b in w.boards if b["id"] == int(m.group(1))), None)
            if not b:
                return self.send(404, {"errorMessages": ["Board does not exist or you do not have permission to see it."]})
            what = m.group(2)
            if what == "configuration":
                return self.send(200, {"id": b["id"], "name": b["name"], "type": b["type"],
                                       "filter": {"id": str(b["filter"])},
                                       "columnConfig": {"columns": board_columns(b)}})
            if what == "quickfilter":
                return self.send(200, {"values": [
                    {"id": 1, "name": "Only My Issues", "jql": "assignee = fake"},
                    {"id": 2, "name": "Recently Updated", "jql": 'updated >= "2026-09-20 00:00"'}]})
            if what == "features":
                on = b["type"] == "scrum"
                return self.send(200, {"features": [
                    {"boardId": b["id"], "feature": "jsw.agility.sprints", "state": "ENABLED" if on else "DISABLED"},
                    {"boardId": b["id"], "feature": "jsw.agility.backlog", "state": "ENABLED" if on else "DISABLED"}]})
            if what == "sprint":
                if b["type"] != "scrum":
                    return self.send(400, {"errorMessages": ["The board does not support sprints"], "errors": {}})
                states = {s for s in q.get("state", "active,future,closed").split(",") if s}
                vals = [s for s in w.sprints[b["id"]] if s["state"] in states]
                page = vals[start:start + n]
                return self.send(200, {"startAt": start, "maxResults": n, "total": len(vals),
                                       "isLast": start + len(page) >= len(vals), "values": page})
            if what == "issue":
                return self.send(200, w.search(q.get("jql", ""), start, n, q.get("fields", ""),
                                               scope=lambda i: i["project"] == b["project"]))
        m = re.fullmatch(r"/sprint/(\d+)", p)
        if m:
            sp = next((s for ss in w.sprints.values() for s in ss if s["id"] == int(m.group(1))), None)
            return self.send(200, sp) if sp else self.send(404, {"errorMessages": ["Sprint does not exist"]})
        m = re.fullmatch(r"/sprint/(\d+)/issue", p)
        if m:
            sid = int(m.group(1))
            return self.send(200, w.search(q.get("jql", ""), start, n, q.get("fields", ""),
                                           scope=lambda i: i["sprint"] == sid))
        return self.send(404, {"errorMessages": [f"no fake for {p}"]})


def main(argv) -> int:
    import argparse
    ap = argparse.ArgumentParser()
    ap.add_argument("cmd", choices=["serve"])
    ap.add_argument("--port", type=int, default=8766)
    ap.add_argument("--seed", type=int, default=7)
    ap.add_argument("--issues", type=int, default=40)
    a = ap.parse_args(argv)
    Handler.world = World(a.seed, a.issues)
    srv = ThreadingHTTPServer(("127.0.0.1", a.port), Handler)
    print(f"fake Jira on http://127.0.0.1:{a.port}", flush=True)
    srv.serve_forever()
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
