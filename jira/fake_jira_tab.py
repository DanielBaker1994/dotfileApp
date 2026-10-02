#!/usr/bin/env python3
"""fake_jira_tab.py — write N fake Jira issues as one Jira window tab.

    fake_jira_tab.py OUT.json [--count N] [--seed S]

Same row shape as the poller's tabs (<outDir>/all.json): key, title,
description, status, assignee, reporter, priority, labels, project, release,
releaseLabel, releaseStatus, releaseDate, updated. Seeded, so the same N gives
the same file. Used by bin/fake-jira-tab.sh to reproduce a big work-sized list
(typing lag in the filter box) without a Jira server. Needs `faker`.
"""
import argparse
import datetime as dt
import json
import os
import random
import sys

try:
    from faker import Faker
except ImportError:
    sys.exit("fake_jira_tab.py needs faker: python3 -m pip install --user faker")

STATUSES = ["To Do", "In Progress", "In Review", "Blocked", "Done", "Closed", "Backlog"]
PRIORITIES = ["Highest", "High", "Medium", "Low", "Lowest"]
LABELS = ["backend", "frontend", "regression", "customer", "tech-debt", "security",
          "flaky-test", "perf", "ux", "infra", "billing", "mobile", "api", "docs"]
VERBS = ["Fix", "Investigate", "Add", "Remove", "Refactor", "Migrate", "Update",
         "Document", "Spike:", "Support", "Improve", "Handle", "Retry"]


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("out")
    ap.add_argument("--count", type=int, default=20000)
    ap.add_argument("--seed", type=int, default=7)
    a = ap.parse_args()

    fake = Faker()
    Faker.seed(a.seed)
    rnd = random.Random(a.seed)
    projects = [w.upper()[:4] for w in fake.words(nb=12, unique=True)]
    people = [fake.name() for _ in range(150)]
    releases = {p: [(f"{rnd.randint(1, 20)}.{i}", fake.date_between("-1y", "+6m")) for i in range(8)]
                for p in projects}
    counters = {p: 0 for p in projects}
    now = dt.datetime(2026, 10, 1, 12, 0, 0)

    rows = []
    for _ in range(a.count):
        p = rnd.choice(projects)
        counters[p] += 1
        rel, rdate = rnd.choice(releases[p])
        if rnd.random() >= 0.6:
            rel = ""   # no fixVersion
        title = f"{rnd.choice(VERBS)} {fake.bs()} in {fake.word()} {rnd.choice(['service', 'page', 'job', 'report', 'API', 'flow'])}"
        # descriptions of a few hundred to a few thousand chars, like real tickets
        desc = " ".join(fake.paragraphs(nb=rnd.randint(1, 8))) if rnd.random() < 0.85 else ""
        updated = now - dt.timedelta(minutes=rnd.randint(0, 60 * 24 * 365))
        rows.append({
            "key": f"{p}-{counters[p]}",
            "title": title[:1].upper() + title[1:],
            "status": rnd.choice(STATUSES),
            "assignee": rnd.choice(people) if rnd.random() < 0.8 else "",
            "release": rel,
            "releaseLabel": f"{rel} ({rdate.isoformat()})" if rel else "",
            "releaseDate": rdate.isoformat() if rel else "",
            "releaseStatus": ("Released" if rdate <= now.date() else "Unreleased") if rel else "",
            "priority": rnd.choice(PRIORITIES),
            "labels": ", ".join(rnd.sample(LABELS, rnd.randint(0, 3))),
            "description": desc,
            "reporter": rnd.choice(people),
            "project": p,
            "updated": updated.strftime("%Y-%m-%dT%H:%M:%S.000-0400"),
        })

    os.makedirs(os.path.dirname(os.path.abspath(a.out)), exist_ok=True)
    tmp = a.out + ".tmp"
    with open(tmp, "w", encoding="utf-8") as fh:
        json.dump(rows, fh)
    os.replace(tmp, a.out)
    print(f"{a.count} issues -> {a.out} ({os.path.getsize(a.out) // 1024} KB)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
