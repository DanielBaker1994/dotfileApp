"""Terminal input → events, decoded by hand (keypad off): CSI / SS3 keys,
xterm modifier params, kitty CSI-u (the launcher maps Cmd+A/C/X/Z to it),
bracketed paste, Alt+x as ESC x. Pure: `decode(chars)` is unit-tested."""
from __future__ import annotations

from dataclasses import dataclass, field


@dataclass
class Ev:
    kind: str                       # key | char | paste | mouse
    name: str = ""                  # key: up down left right home end pgup pgdn tab enter esc
                                    #      backspace delete; char: the character
    mods: frozenset = field(default_factory=frozenset)   # ctrl alt shift cmd
    text: str = ""                  # paste
    x: int = 0                      # mouse: 0-based cell
    y: int = 0

    def is_(self, name: str, *mods) -> bool:
        return self.kind in ("key", "char") and self.name == name and self.mods == frozenset(mods)


_FINAL = {"A": "up", "B": "down", "C": "right", "D": "left", "H": "home", "F": "end", "Z": "tab"}
_TILDE = {"1": "home", "2": "insert", "3": "delete", "4": "end", "5": "pgup", "6": "pgdn",
          "7": "home", "8": "end"}
_U = {9: "tab", 13: "enter", 27: "esc", 127: "backspace", 32: "space"}


def _xmods(n: int) -> frozenset:
    n = max(0, n - 1)
    out = set()
    if n & 1:
        out.add("shift")
    if n & 2:
        out.add("alt")
    if n & 4:
        out.add("ctrl")
    if n & 8:
        out.add("cmd")
    return frozenset(out)


def _control(c: str):
    o = ord(c)
    if c in "\r\n":
        return Ev("key", "enter")
    if c == "\t":
        return Ev("key", "tab")
    if o in (0x7F, 0x08):
        return Ev("key", "backspace")
    if o == 0:
        return Ev("char", " ", frozenset({"ctrl"}))
    if 1 <= o <= 26:
        return Ev("char", chr(o + 96), frozenset({"ctrl"}))
    if o == 0x1F:
        return Ev("char", "/", frozenset({"ctrl"}))
    return None


class Decoder:
    """Feed characters (str, from get_wch) one at a time; `pending()` tells
    the reader to wait a few ms for the rest of an escape sequence."""

    def __init__(self):
        self.buf = ""
        self.paste = None

    def pending(self) -> bool:
        return bool(self.buf) or self.paste is not None

    def feed(self, c: str) -> list:
        if self.paste is not None:
            self.paste += c
            end = self.paste.find("\x1b[201~")
            if end >= 0:
                text = self.paste[:end]
                rest = self.paste[end + 6:]
                self.paste = None
                out = [Ev("paste", text=text)]
                for ch in rest:
                    out += self.feed(ch)
                return out
            return []
        if not self.buf:
            if c == "\x1b":
                self.buf = c
                return []
            ev = _control(c)
            return [ev] if ev else [Ev("char", c)]
        self.buf += c
        return self._try()

    def timeout(self) -> list:
        """No more input arrived: a lone ESC is Esc; ESC x is Alt+x."""
        b, self.buf = self.buf, ""
        if not b:
            return []
        if b == "\x1b":
            return [Ev("key", "esc")]
        if len(b) == 2:
            ev = _control(b[1])
            if ev:
                return [Ev(ev.kind, ev.name, ev.mods | {"alt"})]
            return [Ev("char", b[1], frozenset({"alt"}))]
        return []          # an unfinished sequence: drop it

    def _try(self) -> list:
        b = self.buf
        if len(b) == 2:
            if b[1] in "[O":
                return []
            self.buf = ""
            if b[1] == "\x1b":
                self.buf = "\x1b"
                return [Ev("key", "esc")]
            ev = _control(b[1])
            if ev:
                return [Ev(ev.kind, ev.name, ev.mods | {"alt"})]
            return [Ev("char", b[1], frozenset({"alt"}))]
        if b[1] == "O" and len(b) == 3:
            self.buf = ""
            name = _FINAL.get(b[2]) or {"P": "f1", "Q": "f2", "R": "f3", "S": "f4"}.get(b[2])
            return [Ev("key", name)] if name else []
        if b[1] != "[":
            self.buf = ""
            return []
        last = b[-1]
        if not ("@" <= last <= "~") or len(b) < 3:
            if len(b) > 32:
                self.buf = ""
            return []
        self.buf = ""
        params = b[2:-1]
        if params.startswith("<") and last in "Mm":
            return _mouse(params[1:], last == "M")
        if last == "~" and params == "200":
            self.paste = ""
            return []
        parts = params.split(";") if params else []
        mods = _xmods(int(parts[1].split(":")[0])) if len(parts) > 1 and parts[1].split(":")[0].isdigit() else frozenset()
        if last == "u":
            try:
                code = int(parts[0].split(":")[0])
            except (ValueError, IndexError):
                return []
            if code in _U:
                name = _U[code]
                return [Ev("char", " ", mods) if name == "space" else Ev("key", name, mods)]
            ch = chr(code)
            return [Ev("char", ch, mods)]
        if last == "~":
            name = _TILDE.get(parts[0] if parts else "")
            return [Ev("key", name, mods)] if name else []
        if last == "Z":
            return [Ev("key", "tab", frozenset({"shift"}))]
        name = _FINAL.get(last)
        return [Ev("key", name, mods)] if name else []


def _mouse(params: str, press: bool) -> list:
    """SGR mouse (CSI < b ; x ; y M|m) → Ev("mouse", name=click | release |
    wheelup | wheeldown | drag, x, y) — 0-based cells."""
    try:
        b, x, y = (int(v) for v in params.split(";"))
    except ValueError:
        return []
    mods = set()
    if b & 4:
        mods.add("shift")
    if b & 8:
        mods.add("alt")
    if b & 16:
        mods.add("ctrl")
    base = b & ~(4 | 8 | 16)
    if base in (64, 65):
        name = "wheelup" if base == 64 else "wheeldown"
    elif base & 32:
        name = "drag"
    elif not press:
        name = "release"
    elif base == 0:
        name = "click"
    else:
        name = "button%d" % (base & 3)
    return [Ev("mouse", name, frozenset(mods), x=x - 1, y=y - 1)]


def decode(text: str) -> list:
    """Whole string → events (tests): a trailing partial ESC = timeout."""
    d, out = Decoder(), []
    for c in text:
        out += d.feed(c)
    return out + d.timeout()
