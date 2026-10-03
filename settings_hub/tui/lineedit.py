"""A one-line text field with the edit keys rule.md asks for: Cmd+A / C /
X / V / Z (+ Shift+Z), Ctrl+C / Ctrl+V, plus the readline basics."""
from __future__ import annotations

import subprocess

from .keys import Ev


def pbcopy(text: str) -> None:
    try:
        subprocess.run(["pbcopy"], input=text, text=True, timeout=2)
    except (OSError, subprocess.SubprocessError):
        pass


def pbpaste() -> str:
    try:
        return subprocess.run(["pbpaste"], capture_output=True, text=True, timeout=2).stdout
    except (OSError, subprocess.SubprocessError):
        return ""


def one_line(s: str) -> str:
    return " ".join(s.replace("\r", "\n").replace("\t", " ").split("\n")).strip("\n")


class LineEdit:
    def __init__(self, text: str = ""):
        self.text = text
        self.pos = len(text)
        self.anchor = None          # selection start (None = no selection)
        self.undo_stack, self.redo_stack = [], []

    # -- helpers
    def sel(self):
        if self.anchor is None or self.anchor == self.pos:
            return None
        return (min(self.anchor, self.pos), max(self.anchor, self.pos))

    def _snap(self):
        self.undo_stack.append((self.text, self.pos))
        self.undo_stack = self.undo_stack[-100:]
        self.redo_stack.clear()

    def set(self, text: str):
        self._snap()
        self.text, self.pos, self.anchor = text, len(text), None

    def insert(self, s: str):
        s = one_line(s)
        if not s:
            return
        self._snap()
        r = self.sel()
        if r:
            self.text = self.text[:r[0]] + self.text[r[1]:]
            self.pos = r[0]
        self.text = self.text[:self.pos] + s + self.text[self.pos:]
        self.pos += len(s)
        self.anchor = None

    def _delete_range(self, a: int, b: int):
        if a == b:
            return
        self._snap()
        self.text = self.text[:a] + self.text[b:]
        self.pos, self.anchor = a, None

    def _word_left(self) -> int:
        i = self.pos
        while i > 0 and self.text[i - 1] == " ":
            i -= 1
        while i > 0 and self.text[i - 1] != " ":
            i -= 1
        return i

    def _word_right(self) -> int:
        i = self.pos
        while i < len(self.text) and self.text[i] == " ":
            i += 1
        while i < len(self.text) and self.text[i] != " ":
            i += 1
        return i

    # -- events; True = handled
    def handle(self, ev: Ev) -> bool:
        m = ev.mods
        if ev.kind == "paste":
            self.insert(ev.text)
            return True
        if ev.kind == "char" and not (m - {"shift"}):
            self.insert(ev.name)
            return True
        if ev.kind == "char":
            c = ev.name.lower()
            if m == {"cmd"} and c == "a":
                self.anchor, self.pos = 0, len(self.text)
                return True
            if (m == {"cmd"} or m == {"ctrl"}) and c == "c":
                r = self.sel()
                pbcopy(self.text[r[0]:r[1]] if r else self.text)
                return True
            if m == {"cmd"} and c == "x":
                r = self.sel() or (0, len(self.text))
                pbcopy(self.text[r[0]:r[1]])
                self._delete_range(*r)
                return True
            if (m == {"cmd"} or m == {"ctrl"}) and c == "v":
                self.insert(pbpaste())
                return True
            if m == {"cmd"} and c == "z":
                if self.undo_stack:
                    self.redo_stack.append((self.text, self.pos))
                    self.text, self.pos = self.undo_stack.pop()
                    self.anchor = None
                return True
            if m == {"cmd", "shift"} and c == "z":
                if self.redo_stack:
                    self.undo_stack.append((self.text, self.pos))
                    self.text, self.pos = self.redo_stack.pop()
                    self.anchor = None
                return True
            if m == {"ctrl"}:
                if c == "a":
                    self.pos, self.anchor = 0, None
                elif c == "e":
                    self.pos, self.anchor = len(self.text), None
                elif c == "u":
                    self._delete_range(0, self.pos)
                elif c == "k":
                    self._delete_range(self.pos, len(self.text))
                elif c == "w":
                    self._delete_range(self._word_left(), self.pos)
                elif c == "b":
                    self.pos, self.anchor = max(0, self.pos - 1), None
                elif c == "f":
                    self.pos, self.anchor = min(len(self.text), self.pos + 1), None
                elif c == "d":
                    self._delete_range(self.pos, min(len(self.text), self.pos + 1))
                else:
                    return False
                return True
            if m == {"alt"} and c in "bf":
                self.pos = self._word_left() if c == "b" else self._word_right()
                self.anchor = None
                return True
            return False
        if ev.kind == "key":
            n = ev.name
            shift = "shift" in m
            if n in ("left", "right", "home", "end"):
                if shift and self.anchor is None:
                    self.anchor = self.pos
                elif not shift:
                    r = self.sel()
                    self.anchor = None
                    if r and n in ("left", "right"):
                        self.pos = r[0] if n == "left" else r[1]
                        return True
                if n == "left":
                    self.pos = self._word_left() if m & {"alt"} else (0 if "cmd" in m else max(0, self.pos - 1))
                elif n == "right":
                    self.pos = self._word_right() if m & {"alt"} else (len(self.text) if "cmd" in m else min(len(self.text), self.pos + 1))
                elif n == "home":
                    self.pos = 0
                else:
                    self.pos = len(self.text)
                return True
            if n == "backspace":
                r = self.sel()
                if r:
                    self._delete_range(*r)
                elif "alt" in m or "ctrl" in m:
                    self._delete_range(self._word_left(), self.pos)
                elif "cmd" in m:
                    self._delete_range(0, self.pos)
                elif self.pos > 0:
                    self._delete_range(self.pos - 1, self.pos)
                return True
            if n == "delete":
                r = self.sel()
                if r:
                    self._delete_range(*r)
                elif self.pos < len(self.text):
                    self._delete_range(self.pos, self.pos + 1)
                return True
        return False
