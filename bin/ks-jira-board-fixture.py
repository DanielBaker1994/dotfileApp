#!/usr/bin/env python3
"""Seed a throwaway sandbox home with a fake Jira board so `board:*` works.

    bin/ks-jira-board-fixture.py <home>

The sandbox has no Jira credentials, so the poller never writes a board; this
drops the poller's output in by hand (like `jira/fake_jira_tab.py` fakes a tab).
Every file is written only when it is absent, so a real home is never clobbered.
Nothing here touches the network; `bin/ks-mcp-sandbox.sh start` calls it.

Writes under <home>:
  jira/cache/board_catalog.json                  boards + sprints
  jira/cache/boards.json                         board 1's column spec
  .cache/kitchen-sink/jira_boards/board-1.json   board 1's issues
  jira/config.json                               pinnedBoards/pinnedBoardViews
"""
from __future__ import annotations

import json
import os
import sys

CATALOG = {
    "projects": [
        {
            "key": "DEMO",
            "name": "Demo",
            "boards": [
                {
                    "id": "1",
                    "name": "Demo board",
                    "type": "scrum",
                    "sprints": [
                        {"id": "7", "name": "Sprint 7", "state": "active",
                         "startDate": "2026-10-01", "endDate": "2026-10-14"},
                        {"id": "6", "name": "Sprint 6", "state": "closed",
                         "startDate": "2026-09-17", "endDate": "2026-09-30",
                         "completeDate": "2026-09-30"},
                    ],
                }
            ],
        }
    ]
}

BOARDS = {
    "1": {
        "name": "Demo board",
        "type": "scrum",
        "columns": [
            {"name": "To Do", "statuses": ["To Do", "Backlog"]},
            {"name": "In Progress", "statuses": ["In Progress", "In Review"]},
            {"name": "Done", "statuses": ["Done"]},
        ],
    }
}

ISSUES = [
    {"key": "DEMO-1", "title": "Fix the fake board", "status": "To Do",
     "type": "Bug", "priority": "High", "assignee": "Ada Lovelace",
     "project": "DEMO", "reporter": "Ada Lovelace"},
    {"key": "DEMO-2", "title": "Add a column", "status": "Backlog",
     "type": "Task", "priority": "Medium", "assignee": "",
     "project": "DEMO", "reporter": "Grace Hopper"},
    {"key": "DEMO-3", "title": "Review the pull request", "status": "In Review",
     "type": "Task", "priority": "Low", "assignee": "Grace Hopper",
     "project": "DEMO", "reporter": "Ada Lovelace"},
    {"key": "DEMO-4", "title": "Ship it", "status": "Done",
     "type": "Story", "priority": "Lowest", "assignee": "Ada Lovelace",
     "project": "DEMO", "reporter": "Grace Hopper"},
    {"key": "DEMO-5", "title": "Something with no column", "status": "Blocked",
     "type": "Bug", "priority": "Highest", "assignee": "",
     "project": "DEMO", "reporter": "Ada Lovelace"},
]


def write(path: str, data) -> bool:
    """Write JSON only when `path` does not exist. True when it wrote."""
    if os.path.exists(path):
        return False
    os.makedirs(os.path.dirname(path), exist_ok=True)
    tmp = path + ".tmp"
    with open(tmp, "w", encoding="utf-8") as fh:
        json.dump(data, fh, indent=2)
    os.replace(tmp, path)
    return True


def main() -> int:
    # The sandbox home is required: never fall back to the real $HOME.
    if len(sys.argv) != 2 or not sys.argv[1]:
        print("usage: ks-jira-board-fixture.py <sandbox-home>", file=sys.stderr)
        return 2
    arg = sys.argv[1]
    home = os.path.abspath(os.path.expanduser(arg))
    cache = os.path.join(home, "jira", "cache")
    write(os.path.join(cache, "board_catalog.json"), CATALOG)
    write(os.path.join(cache, "boards.json"), BOARDS)
    write(
        os.path.join(home, ".cache", "kitchen-sink", "jira_boards", "board-1.json"),
        ISSUES,
    )
    write(
        os.path.join(home, "jira", "config.json"),
        {"pinnedBoards": [], "pinnedBoardViews": []},
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
