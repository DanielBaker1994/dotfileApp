"""config_text.py - THE python codec for commands.toml's one-line TOML.

Mirrors ConfigText.swift (configEntry / configLine / configSectionHeader /
configLines / configSectionEntries / configSetting). Every python reader or
writer of commands.toml goes through here: never split on '=' by hand.
jira_config.py re-exports config_entry / config_line for older callers.
Stdlib only, python 3.9+ (launchd may run the CLT /usr/bin/python3).
"""
from __future__ import annotations

import re

_TOML_SCALAR = re.compile(r"-?(0|[1-9][0-9]*)(\.[0-9]+)?")
_TOML_ESC = {"n": "\n", "t": "\t", "r": "\r", "b": "\b", "f": "\f", '"': '"', "\\": "\\"}


def _toml_bare_scalar(v: str) -> bool:
    return v in ("true", "false") or bool(_TOML_SCALAR.fullmatch(v))


def _toml_scan_string(s: str):
    """A "basic" or 'literal' string at the start of s → (text, rest), else None."""
    if not s or s[0] not in "\"'":
        return None
    if s[0] == "'":
        end = s.find("'", 1)
        return None if end < 0 else (s[1:end], s[end + 1:])
    out, i = [], 1
    while i < len(s):
        c = s[i]
        if c == '"':
            return "".join(out), s[i + 1:]
        if c == "\\":
            i += 1
            if i >= len(s):
                return None
            e = s[i]
            if e in "uU":
                n = 4 if e == "u" else 8
                try:
                    out.append(chr(int(s[i + 1:i + 1 + n], 16)))
                except ValueError:
                    return None
                if len(s[i + 1:i + 1 + n]) != n:
                    return None
                i += n
            elif e in _TOML_ESC:
                out.append(_TOML_ESC[e])
            else:
                return None
        else:
            out.append(c)
        i += 1
    return None


def _toml_value(raw: str) -> str:
    """The text of a value (same rules as the app's tomlValue): strings
    unquoted, one-line [arrays] joined ", ", bare values as written."""
    def only_comment(rest: str) -> bool:
        r = rest.strip()
        return not r or r.startswith("#")
    if raw[:1] in ("\"", "'"):
        got = _toml_scan_string(raw)
        return got[0] if got and only_comment(got[1]) else raw
    if raw.startswith("["):
        items, rest = [], raw[1:]
        while True:
            rest = rest.lstrip(" \t")
            if rest.startswith("]"):
                return ", ".join(items) if only_comment(rest[1:]) else raw
            got = _toml_scan_string(rest)
            if got:
                items.append(got[0])
                rest = got[1]
            else:
                m = re.match(r"[^,\]]*", rest)
                v = m.group(0).strip()
                if not _toml_bare_scalar(v):
                    return raw
                items.append(v)
                rest = rest[m.end():]
            rest = rest.lstrip(" \t")
            if rest.startswith(","):
                rest = rest[1:]
            elif not rest.startswith("]"):
                return raw
    head, sep, _ = raw.partition(" #")
    if sep and _toml_bare_scalar(head.strip()):
        return head.strip()
    return raw


def config_entry(line: str):
    """One `key = value` line of commands.toml → (key, value) or None
    (mirrors the app's configEntry: TOML strings / bare scalars / one-line
    arrays; old unquoted values read as written)."""
    s = line.strip()
    if not s or s[0] in "#[":
        return None
    if s[0] in "\"'":
        got = _toml_scan_string(s)
        if not got:
            return None
        key, rest = got[0], got[1].lstrip(" \t")
        if not rest.startswith("="):
            return None
        rest = rest[1:]
    else:
        if "=" not in s:
            return None
        key, rest = s.split("=", 1)
        key = key.strip()
    return key, _toml_value(rest.strip())


def _toml_quote(v: str) -> str:
    out = []
    for c in v:
        if c in ('"', "\\"):
            out.append("\\" + c)
        elif c in "\n\t\r":
            out.append({"\n": "\\n", "\t": "\\t", "\r": "\\r"}[c])
        elif ord(c) < 0x20 or ord(c) == 0x7F:
            out.append("\\u%04X" % ord(c))
        else:
            out.append(c)
    return '"' + "".join(out) + '"'


def config_line(key: str, value: str) -> str:
    """`key = value` as a TOML line (strings quoted, bools/numbers bare)."""
    k = key if re.fullmatch(r"[A-Za-z0-9_-]+", key) else _toml_quote(key)
    return f"{k} = {value if _toml_bare_scalar(value) else _toml_quote(value)}"


def config_section_header(line: str):
    """`[name]` → "name", else None (same as configSectionHeader)."""
    s = line.strip()
    if s.startswith("[") and s.endswith("]"):
        return s[1:-1].strip()
    return None


def config_lines(text: str) -> list:
    """The file as editable lines; "\n".join() restores it byte for byte.
    Never splitlines(): it also splits on \x1c, \u2028, ..."""
    return text.split("\n")


def config_section_entries(lines: list, section: str) -> list:
    """[(index, key, value)] of every entry in [section], file order."""
    out, cur = [], None
    for i, raw in enumerate(lines):
        h = config_section_header(raw)
        if h is not None:
            cur = h
            continue
        if cur != section:
            continue
        e = config_entry(raw)
        if e:
            out.append((i, e[0], e[1]))
    return out


def _value_end(rest: str) -> int:
    """Index in `rest` (the text after '=') where the value ends."""
    lead = len(rest) - len(rest.lstrip(" \t"))
    r = rest[lead:]
    if r[:1] in ("\"", "'"):
        got = _toml_scan_string(r)
        return lead + len(r) - len(got[1]) if got else len(rest)
    if r.startswith("["):
        depth, i, q = 0, 0, None
        while i < len(r):
            c = r[i]
            if q:
                if c == "\\" and q == '"':
                    i += 1
                elif c == q:
                    q = None
            elif c in "\"'":
                q = c
            elif c == "[":
                depth += 1
            elif c == "]":
                depth -= 1
                if depth == 0:
                    return lead + i + 1
            i += 1
        return len(rest)
    head, sep, _ = r.partition(" #")
    if sep and _toml_bare_scalar(head.strip()):
        return lead + len(head.rstrip())
    return len(rest)


def config_line_parts(line: str):
    """(indent, comment_tail) of an entry line: what to keep around a
    rewritten `key = value` (tail = text after the value, e.g. "  # note")."""
    indent = line[: len(line) - len(line.lstrip(" \t"))]
    s = line.strip()
    if s[:1] in ("\"", "'"):
        got = _toml_scan_string(s)
        if not got:
            return indent, ""
        after = got[1]
        rest = after.lstrip(" \t")[1:]
    elif "=" in s:
        rest = s.split("=", 1)[1]
    else:
        return indent, ""
    tail = rest[_value_end(rest):]
    return indent, (tail if tail.strip().startswith("#") else "")


def config_entry_span(line: str):
    """(start, end) of the KEY text in an entry line (rebinding swaps only
    the key), else None."""
    indent = len(line) - len(line.lstrip(" \t"))
    s = line[indent:]
    if s[:1] in ("\"", "'"):
        got = _toml_scan_string(s)
        if not got:
            return None
        return indent, indent + len(s) - len(got[1])
    if "=" not in s or s[:1] in "#[":
        return None
    k = s.split("=", 1)[0].rstrip(" \t")
    return indent, indent + len(k)


def config_set_line(line: str, key: str, value: str) -> str:
    """`line` rewritten to `key = value`, keeping its indent + trailing comment."""
    indent, tail = config_line_parts(line)
    return indent + config_line(key, value) + tail


def config_setting(lines: list, section: str, kv) -> list:
    """Mirror of ConfigText.swift configSetting: [(key, value | None)] →
    new lines. The LAST copy of a key is edited (it's the one read), None
    removes it, a new key goes after the section's last entry (else its
    header, else a new section at the end). Indent + trailing comments of an
    edited line are kept."""
    lines = list(lines)
    for key, value in kv:
        entries = config_section_entries(lines, section)
        found = next((i for i, k, _ in reversed(entries) if k == key), None)
        headers = [i for i, l in enumerate(lines) if config_section_header(l) == section]
        header = headers[-1] if headers else None
        last = max(entries[-1][0], header if header is not None else -1) if entries else header
        if found is not None and value is not None:
            lines[found] = config_set_line(lines[found], key, value)
        elif found is not None:
            del lines[found]
        elif value is not None:
            if last is not None:
                lines.insert(last + 1, config_line(key, value))
            else:
                lines += ["", f"[{section}]", config_line(key, value)]
    return lines


def toml_array(items) -> str:
    """["a", "b"] (real TOML arrays for aerospace / herdr values)."""
    return "[" + ", ".join(_toml_quote(str(i)) for i in items) + "]"


def toml_string(v: str) -> str:
    return _toml_quote(v)
