"""Jira setup glue: the project-key grammar (Setup window + dashboard
scope/team edits share it)."""
from __future__ import annotations

import re

_KEY_RE = re.compile(r"^[A-Z][A-Z0-9_]*$")


def project_keys(raw: str) -> dict:
    """Comma/whitespace-separated keys, uppercased and de-duplicated; ones
    that cannot be Jira keys come back separately for the error message."""
    keys, bad = [], []
    for part in re.split(r"[,\s]+", (raw or "").upper()):
        if not part:
            continue
        if not _KEY_RE.match(part):
            bad.append(part)
        elif part not in keys:
            keys.append(part)
    return {"keys": keys, "bad": bad}
