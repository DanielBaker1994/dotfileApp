"""VS Code parity: which standard keys each view's panes are missing.

data/parity.toml lists the pane kinds of every view and what a pane of
each kind is expected to offer (VS Code's default keymap as the
reference). An expectation is met when every one of its keys appears in
commands.toml [shortcuts] for that view, for "all", or for the pane kind
itself ("sidebar: …" rows = every view's sidebar; "A | B" = either);
a [[waive]] marks a gap that is fine on purpose. `$WS_PARITY` overrides
the data file (tests).
"""
from __future__ import annotations

import os
import tomllib
from dataclasses import dataclass

from . import chords

DATA = os.path.join(os.path.dirname(os.path.abspath(__file__)), "data", "parity.toml")


@dataclass
class Check:
    view: str
    kind: str
    concept: str
    vscode: str
    keys: list          # as written in parity.toml
    status: str         # have | missing | waived
    missing: list       # the key groups nobody lists (status missing)
    reason: str = ""    # the waiver's


def load(path: str | None = None) -> dict:
    with open(path or os.environ.get("WS_PARITY") or DATA, "rb") as fh:
        return tomllib.load(fh)


def _chords(text: str) -> set:
    cs, _ = chords.app_label(text)
    return set(cs)


def check(app_rows: list, data: dict | None = None, view: str | None = None) -> list:
    """`app_rows` = the catalog's app-layer KeyRows."""
    data = data if data is not None else load()
    views = data.get("views", {})
    expects = data.get("expect", [])
    waived = {(w.get("view", ""), w.get("concept", "")): w.get("reason", "") for w in data.get("waive", [])}
    have_by_view: dict = {}
    for r in app_rows:
        have_by_view.setdefault(r.view, set()).update(r.chords)
    out = []
    for v, kinds in views.items():
        if view and v != view:
            continue
        base = have_by_view.get(v, set()) | have_by_view.get("all", set())
        for kind in kinds:
            # "sidebar: …" / "preview: …" rows = that pane in every view
            have = base | have_by_view.get(kind, set())
            for e in expects:
                if e.get("kind") != kind:
                    continue
                keys = list(e.get("keys", []))
                missing = [k for k in keys
                           if not any(_chords(alt.strip()) & have for alt in k.split(" | "))]
                concept = e.get("concept", "")
                status, reason = "have", ""
                if missing:
                    if (v, concept) in waived:
                        status, reason = "waived", waived[(v, concept)]
                    else:
                        status = "missing"
                out.append(Check(view=v, kind=kind, concept=concept, vscode=e.get("vscode", ""),
                                 keys=keys, status=status, missing=missing, reason=reason))
    return out


def as_json(c: Check) -> dict:
    return {"view": c.view, "kind": c.kind, "concept": c.concept, "vscode": c.vscode,
            "keys": c.keys, "status": c.status, "missing": c.missing, "reason": c.reason}
