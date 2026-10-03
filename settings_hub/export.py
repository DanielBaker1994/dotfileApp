"""Cheat sheet (Markdown) and the full catalog (JSON)."""
from __future__ import annotations

import json

from .model import Catalog

LAYER_TITLE = {"aerospace": "Global hotkeys (AeroSpace)", "app": "workspace-switcher",
               "herdr": "herdr (terminal)", "ghostty": "Ghostty", "vim": "Notes vim pane"}


def key_json(r) -> dict:
    return {"id": r.id, "layer": r.layer, "view": r.view, "keys": r.display,
            "chords": [c.text for c in r.chords], "source": r.chord_text, "action": r.action,
            "doc": r.doc, "kind": r.kind, "editable": r.editable,
            "readonly": r.readonly_reason, "file": r.source_file, "line": r.line,
            "mirrorOf": r.mirror_of}


def setting_json(s) -> dict:
    return {"id": s.id, "section": s.section, "key": s.key, "value": s.value, "set": s.set,
            "type": s.type, "doc": s.doc, "lineDoc": s.line_doc, "allowed": s.allowed,
            "apply": s.apply, "file": s.source_file, "line": s.line}


def to_json(cat: Catalog) -> str:
    return json.dumps({"keys": [key_json(r) for r in cat.keys],
                       "settings": [setting_json(s) for s in cat.settings],
                       "sources": cat.sources,
                       "warnings": [{"file": f, "line": l, "message": m} for f, l, m in cat.warnings]},
                      indent=2, ensure_ascii=False)


def _cell(t: str) -> str:
    return t.replace("|", "\\|").replace("\n", " ")


def to_markdown(cat: Catalog, settings: bool = True) -> str:
    out = ["# Shortcuts & settings", ""]
    for layer in LAYER_TITLE:
        rows = [r for r in cat.keys if r.layer == layer and not r.mirror_of]
        if not rows:
            continue
        out += [f"## {LAYER_TITLE[layer]}", ""]
        views = []
        for r in rows:
            if r.view not in views:
                views.append(r.view)
        for v in views:
            out += [f"### {v}", "", "| Keys | Does |", "|---|---|"]
            out += [f"| `{_cell(r.display)}` | {_cell(r.action)} |" for r in rows if r.view == v]
            out.append("")
    if settings and cat.settings:
        out += ["## Settings (commands.toml)", ""]
        sections = []
        for s in cat.settings:
            if s.section not in sections:
                sections.append(s.section)
        for sec in sections:
            out += [f"### [{sec}]", "", "| Key | Value | What |", "|---|---|---|"]
            for s in cat.settings:
                if s.section == sec:
                    val = s.value if s.set else f"*({s.value})*"
                    out.append(f"| `{s.key}` | `{_cell(val)}` | {_cell(s.doc or s.line_doc)} |")
            out.append("")
    return "\n".join(out)
