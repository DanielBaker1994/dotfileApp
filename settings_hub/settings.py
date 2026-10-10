"""The settings catalog: every `[section]` key of commands.toml with its
current value, its documentation (the `#   key   text` comment blocks the
file already carries) and a best-guess type."""
from __future__ import annotations

from .tables import APPLY, APP_LAUNCH_ONLY, COLOR_KEYS, NOT_SETTINGS  # data/tables.json

import re

from . import paths
from .model import Catalog, SettingRow

from config_text import config_entry, config_section_header  # noqa: E402

# a doc entry: `#   key   text`, `#   a / b   text`, `#   key : text`
_DOC = re.compile(r"^#(\s+)([a-z0-9][a-z0-9_-]*(?:\s*[/,]\s*[a-z0-9][a-z0-9_-]*)*)(?:\s{2,}|\s*:\s+)(\S.*)$")
# a commented-out entry: `# key = value` (one space at most: doc lines use 3)
_OFF = re.compile(r"^#\s?([a-z0-9][a-z0-9_-]*)\s*=\s*(.+)$")
_ENUM = re.compile(r"\b([a-z0-9][\w.-]*)((?:\s*\|\s*[a-z0-9][\w.-]*)+)")
_HEX = re.compile(r"^(0x)?[0-9A-Fa-f]{6}([0-9A-Fa-f]{2})?$")


def parse_docs(comment_lines: list) -> dict:
    """[(text after '#'… as written)] → {key: doc text}. Continuation lines
    are the ones indented to (at least) the entry's text column."""
    docs, cur_keys, col = {}, [], 0
    for raw in comment_lines:
        line = raw.rstrip()
        m = _DOC.match(line)
        if m:
            cur_keys = [k.strip() for k in re.split(r"[/,]", m.group(2))]
            col = m.start(3)
            for k in cur_keys:
                docs[k] = m.group(3).strip()
            continue
        body = line[1:] if line.startswith("#") else line
        lead = len(body) - len(body.lstrip(" "))
        if cur_keys and body.strip() and lead + 1 >= col - 1 and lead >= 4:
            for k in cur_keys:
                docs[k] += " " + body.strip()
            continue
        cur_keys = []
    return docs


def allowed_values(doc: str) -> list:
    m = _ENUM.search(re.sub(r"\s*\([^)]*\)", "", doc))
    if not m:
        return []
    vals = [m.group(1)] + [v.strip() for v in m.group(2).split("|") if v.strip()]
    return vals if len(vals) >= 2 else []


def guess_type(section: str, key: str, value: str, doc: str, allowed: list) -> str:
    v = value.strip()
    if v in ("true", "false") or doc.startswith("true/false"):
        return "bool"
    if allowed:
        return "enum"
    if (section == "theme" and key in COLOR_KEYS) or key.endswith("-color") or \
            key.endswith("-background") or (key == "background" and _HEX.match(v or "x")):
        return "color"
    if re.fullmatch(r"-?\d+(\.\d+)?", v):
        return "number"
    if v.startswith(("~", "/")) or re.search(r"-(dir|path|bin|file|init|log)$", key):
        return "path"
    if ", " in v:
        return "list"
    return "text"




def apply_mode(section: str, key: str) -> str:
    if section == "jira" and key == "enabled":
        return "jira-switch"
    if section == "app" and key in APP_LAUNCH_ONLY:
        return "restart"
    return APPLY.get(section, "reload")


def read_settings(cat: Catalog, path: str | None = None) -> list:
    path = path or paths.commands_conf()
    try:
        with open(path, encoding="utf-8") as fh:
            lines = fh.read().split("\n")
    except OSError:
        cat.warn(path, 0, "commands.toml not found")
        return []
    # split the file into sections; comment blocks right above a header
    # belong to that header, other comments to the section they sit in
    headers = [(i, config_section_header(l)) for i, l in enumerate(lines)
               if config_section_header(l) is not None]
    owner = {}       # line index → section name for comment lines
    preamble_end = headers[0][0] if headers else len(lines)
    for n, (hi, name) in enumerate(headers):
        j = hi - 1
        while j >= 0 and lines[j].strip().startswith("#"):
            owner[j] = name
            j -= 1
    cur = None
    for i, l in enumerate(lines):
        h = config_section_header(l)
        if h is not None:
            cur = h
            continue
        if l.strip().startswith("#") and i not in owner and cur is not None:
            owner[i] = cur
    by_section: dict = {}
    for i, name in owner.items():
        by_section.setdefault(name, []).append(i)
    general = parse_docs([lines[i].strip() for i in range(preamble_end) if lines[i].strip().startswith("#")])
    docs = {name: parse_docs([lines[i].strip() for i in sorted(idx)]) for name, idx in by_section.items()}

    rows, seen = [], set()
    cur = None
    for i, l in enumerate(lines):
        h = config_section_header(l)
        if h is not None:
            cur = h
            continue
        if cur is None or cur in NOT_SETTINGS:
            continue
        e = config_entry(l)
        off = None
        if not e:
            s = l.strip()
            m = _OFF.match(s) if s.startswith("#") else None
            if m and not _DOC.match(s):
                off = (m.group(1), m.group(2).strip().strip('"'))
            else:
                continue
        key, value = e if e else off
        if (cur, key) in seen and off:
            continue
        doc = docs.get(cur, {}).get(key) or general.get(key, "")
        line_doc = ""
        if e:
            j, above = i - 1, []
            while j >= 0 and lines[j].strip().startswith("#") and not _DOC.match(lines[j].strip()):
                above.append(lines[j].strip().lstrip("#").strip())
                j -= 1
            line_doc = " ".join(reversed(above))
        allowed = allowed_values(doc)
        typ = guess_type(cur, key, value, doc, allowed)
        if typ == "bool":
            allowed = ["true", "false"]
        row = SettingRow(section=cur, key=key, value=value, set=bool(e), type=typ, doc=doc,
                         line_doc=line_doc, allowed=allowed, source_file=path, line=i + 1,
                         apply=apply_mode(cur, key))
        if (cur, key) in seen:
            # a duplicate key: the app reads the LAST one
            rows = [r for r in rows if not (r.section == cur and r.key == key)]
            cat.warn(path, i + 1, f"[{cur}] {key} is set twice; the last one wins")
        seen.add((cur, key))
        rows.append(row)
    # drop a commented-out row when the key is also set for real
    real = {(r.section, r.key) for r in rows if r.set}
    return [r for r in rows if r.set or (r.section, r.key) not in real]
