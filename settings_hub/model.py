"""The catalog's rows: keys (every layer) and settings (commands.toml)."""
from __future__ import annotations

from dataclasses import dataclass, field

MODS = ("ctrl", "alt", "shift", "cmd")          # display order ⌃ ⌥ ⇧ ⌘
MOD_NAME = {"ctrl": "Ctrl", "alt": "Opt", "shift": "Shift", "cmd": "Cmd"}
MOD_SYMBOL = {"ctrl": "⌃", "alt": "⌥", "shift": "⇧", "cmd": "⌘"}


@dataclass(frozen=True)
class Stroke:
    """One key press: modifiers + a key name ("K", "Return", "Up", "/")."""
    mods: frozenset
    key: str

    @property
    def text(self) -> str:
        if set(MODS) <= self.mods:
            return "Hyper+" + self.key
        return "+".join([MOD_NAME[m] for m in MODS if m in self.mods] + [self.key])


@dataclass(frozen=True)
class Chord:
    """A key sequence: usually one stroke; herdr prefix keys and "Esc Esc"
    are two."""
    strokes: tuple

    @property
    def text(self) -> str:
        return ", ".join(s.text for s in self.strokes)

    @property
    def first(self) -> Stroke:
        return self.strokes[0]

    @property
    def mods(self) -> frozenset:
        return self.strokes[-1].mods


@dataclass
class KeyRow:
    layer: str                 # aerospace | app | herdr | vim | ghostty
    view: str                  # main, service, files, jira, global, normal…
    chords: list               # [Chord]; empty for gestures ("Mouse drag")
    chord_text: str            # as written in the source (display + write-back)
    action: str
    source_file: str = ""
    line: int = 0              # 1-based; 0 = unknown
    doc: str = ""
    editable: bool = False
    readonly_reason: str = ""
    kind: str = "key"          # key | gesture
    mirror_of: str = ""        # an aerospace binding that runs the app: its [shortcuts] row
    raw: str = ""
    id: str = ""               # stable id (favorites): layer:view:chord_text

    def __post_init__(self):
        if not self.id:
            self.id = f"{self.layer}:{self.view}:{self.chord_text}"

    @property
    def display(self) -> str:
        """The chord as people read it: the app's own label / vim's lhs as
        written, the normalized form for aerospace / herdr / ghostty."""
        if self.layer in ("app", "vim") or not self.chords:
            return self.chord_text
        return " / ".join(c.text for c in self.chords)


@dataclass
class SettingRow:
    section: str
    key: str
    value: str
    set: bool = True           # False = a commented-out default (`# key = value`)
    type: str = "text"         # bool | number | color | path | list | enum | text
    doc: str = ""
    line_doc: str = ""
    allowed: list = field(default_factory=list)
    range: tuple | None = None
    source_file: str = ""
    line: int = 0
    apply: str = ""            # reload | trigger | open | restart | sketchybar | jira-switch
    id: str = ""

    def __post_init__(self):
        if not self.id:
            self.id = f"setting:{self.section}.{self.key}"


@dataclass
class Catalog:
    keys: list = field(default_factory=list)
    settings: list = field(default_factory=list)
    warnings: list = field(default_factory=list)   # (file, line, message)
    sources: dict = field(default_factory=dict)    # layer -> path (or "" = missing)

    def warn(self, path: str, line: int, msg: str) -> None:
        self.warnings.append((path, line, msg))
