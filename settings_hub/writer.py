"""Line-preserving edits. Every write changes ONE hunk, keeps every other
byte, goes through the codec, writes atomically into the REAL file (a
symlinked config stays a link) and is recorded for `undo`."""
from __future__ import annotations

import hashlib
import os
import re
import tempfile

from . import undo

from config_text import (config_line, config_section_entries,  # noqa: E402
                         config_section_header, config_set_line)


class WriteError(Exception):
    pass


def read(path: str) -> tuple:
    real = os.path.realpath(path)
    with open(real, "rb") as fh:
        data = fh.read()
    return real, data.decode("utf-8"), hashlib.sha256(data).hexdigest()


def write_atomic(real: str, text: str) -> None:
    st = os.stat(real)
    fd, tmp = tempfile.mkstemp(dir=os.path.dirname(real), prefix="." + os.path.basename(real) + ".")
    try:
        with os.fdopen(fd, "wb") as fh:
            fh.write(text.encode("utf-8"))
            fh.flush()
            os.fsync(fh.fileno())
        os.chmod(tmp, st.st_mode & 0o7777)
        os.replace(tmp, real)
    except BaseException:
        try:
            os.unlink(tmp)
        except OSError:
            pass
        raise


def _hunk_ok(old: list, new: list) -> bool:
    """Exactly one line replaced, or a few lines inserted in one place."""
    if len(new) == len(old):
        return sum(1 for a, b in zip(old, new) if a != b) == 1
    if len(new) < len(old):
        return False
    i = 0
    while i < len(old) and old[i] == new[i]:
        i += 1
    k = len(new) - len(old)
    return new[i + k:] == old[i:]


def plan_setting(lines: list, section: str, key: str, value: str) -> tuple:
    """→ (new_lines, line_no 1-based, op). Last duplicate wins (as the app
    reads it); a commented-out `# key = …` in the section is un-commented in
    place; else the key goes after the section's last entry."""
    entries = config_section_entries(lines, section)
    hits = [e for e in entries if e[1] == key]
    new = list(lines)
    if hits:
        i = hits[-1][0]
        new[i] = config_set_line(lines[i], key, value)
        return new, i + 1, "replace"
    # find the section's span
    start = end = None
    for i, l in enumerate(lines):
        h = config_section_header(l)
        if h is not None:
            if start is not None:
                end = i
                break
            if h == section:
                start = i
    if start is not None and end is None:
        end = len(lines)
    if start is not None:
        off = re.compile(r"^(\s*)#\s?" + re.escape(key) + r"\s*=")
        for i in range(start + 1, end):
            m = off.match(lines[i])
            if m:
                new[i] = m.group(1) + config_line(key, value)
                return new, i + 1, "uncomment"
        last = entries[-1][0] if entries else start
        new.insert(last + 1, config_line(key, value))
        return new, last + 2, "insert"
    # a new section at the end (before a final empty line)
    tail = len(new)
    while tail > 0 and new[tail - 1] == "":
        tail -= 1
    add = ["", f"[{section}]", config_line(key, value)]
    new[tail:tail] = add
    return new, tail + 3, "insert"


def apply_edit(path: str, make, label: dict) -> dict:
    """Read → make(lines) → (new_lines, line_no, op) → check → write.
    Retries once when the file changed between read and write."""
    for attempt in (1, 2):
        real, text, sha = read(path)
        lines = text.split("\n")
        new, line_no, op = make(lines)
        if new == lines:
            return {"changed": False, "file": real, "line": line_no, "op": "none"}
        if not _hunk_ok(lines, new):
            raise WriteError("internal: the edit would change more than one place")
        out = "\n".join(new)
        _, _, sha_now = read(path)
        if sha_now != sha:
            if attempt == 2:
                raise WriteError(f"{real} keeps changing; try again")
            continue
        write_atomic(real, out)
        rec = undo.record(real, lines, new, label)
        return {"changed": True, "file": real, "line": line_no, "op": op, "undo": rec}
    raise WriteError("unreachable")


def set_setting(path: str, section: str, key: str, value: str, check_file=None) -> dict:
    def make(lines):
        return plan_setting(lines, section, key, value)

    if check_file is not None:
        real, text, _ = read(path)
        new, _, _ = make(text.split("\n"))
        issues = check_file("\n".join(new))
        fatal = [i for i in (issues or []) if i.get("fatal")]
        if fatal:
            raise WriteError("the edited file would be invalid: " + "; ".join(i["message"] for i in fatal))
    return apply_edit(path, make, {"kind": "setting", "section": section, "key": key, "value": value})


def replace_line(path: str, line_no: int, expect: str, new_line: str, label: dict) -> dict:
    """Swap one known line (rebinding); refuses when it no longer matches."""
    def make(lines):
        i = line_no - 1
        if not (0 <= i < len(lines)) or lines[i] != expect:
            raise WriteError(f"line {line_no} changed since it was read")
        new = list(lines)
        new[i] = new_line
        return new, line_no, "replace"
    return apply_edit(path, make, label)
