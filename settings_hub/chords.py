"""Chord parsing: every source's dialect → model.Chord, so search, the
modifier filter and the clash check compare like with like."""
from __future__ import annotations

from .tables import ARROWS, KEY_NAMES, LABEL_MOD as _LABEL_MOD, MOD_WORDS, VIM_SPECIAL as _VIM_SPECIAL  # data/tables.json

import re

from .model import MODS, Chord, Stroke

# key names → the one display form


def key_name(k: str) -> str:
    low = k.lower()
    if low in KEY_NAMES:
        return KEY_NAMES[low]
    if re.fullmatch(r"f[0-9]{1,2}", low):
        return low.upper()
    if len(k) == 1:
        return k.upper()
    return k[:1].upper() + k[1:]


def stroke(mods, key: str) -> Stroke:
    return Stroke(frozenset(mods), key_name(key))


def chord(*strokes) -> Chord:
    return Chord(tuple(strokes))


# ------------------------------------------------------------- AeroSpace
def aerospace(s: str) -> Chord | None:
    """`alt-cmd-ctrl-shift-x`, `alt-minus`, `esc` → Chord."""
    parts = s.strip().split("-")
    if not parts or not parts[-1]:
        return None
    *mods, key = parts
    try:
        ms = [MOD_WORDS[m] for m in mods]
    except KeyError:
        return None
    return chord(stroke(ms, key))


# ------------------------------------------------------------- herdr / ghostty
def _plus_split(s: str) -> list:
    """`prefix+shift+a`, `ctrl++` → parts (a trailing `+` is the key)."""
    if s.endswith("++"):
        return s[:-2].split("+") + ["+"] if s[:-2] else ["+"]
    return s.split("+")


def plus_stroke(s: str) -> Stroke | None:
    parts = [p for p in _plus_split(s.strip()) if p != ""] or []
    if not parts:
        return None
    *mods, key = parts
    ms = []
    for m in mods:
        mm = MOD_WORDS.get(m.lower())
        if mm is None:
            return None
        ms.append(mm)
    return stroke(ms, key)


def herdr(s: str, prefix: Stroke | None) -> Chord | None:
    s = s.strip()
    if not s:
        return None
    if s.startswith("prefix+"):
        rest = plus_stroke(s[len("prefix+"):])
        if rest is None or prefix is None:
            return None
        return chord(prefix, rest)
    st = plus_stroke(s)
    return chord(st) if st else None


def ghostty(s: str) -> Chord | None:
    """`alt+space`, `ctrl+\\``, `super+shift+z`; `>`-sequences = strokes."""
    strokes = []
    for part in s.split(">"):
        st = plus_stroke(part)
        if st is None:
            return None
        strokes.append(st)
    return Chord(tuple(strokes)) if strokes else None


# ------------------------------------------------------------- vim


def vim(lhs: str) -> Chord | None:
    """`<C-x>`, `<M-j>`, `<leader>f`, `dd` → strokes."""
    out, i = [], 0
    while i < len(lhs):
        if lhs[i] == "<":
            j = lhs.find(">", i)
            if j > i:
                body = lhs[i + 1:j]
                bits = body.split("-")
                mods, key = bits[:-1], bits[-1] or "-"
                ms = [MOD_WORDS.get(m.lower()) for m in mods]
                if None not in ms:
                    out.append(stroke(ms, _VIM_SPECIAL.get(key.lower(), key)))
                    i = j + 1
                    continue
        c = lhs[i]
        # vim is case-sensitive: "D" = Shift+d
        out.append(Stroke(frozenset({"shift"}) if c.isalpha() and c.isupper() else frozenset(),
                          key_name(c)))
        i += 1
    return Chord(tuple(out)) if out else None


# ------------------------------------------------------------- [shortcuts] labels
GESTURE = re.compile(r"click|drag|wheel|mouse|while|\*\*|^term$|hold|scroll", re.I)
# `_LABEL_MOD` comes from data/tables.json (label_mods): label word -> modifier;
# it adds hyper and omits mod_words' symbol/letter aliases


def _label_stroke(tok: str) -> Stroke | None:
    *mods, key = _plus_split(tok)
    if not key:
        return None
    ms = set()
    for m in mods:
        mm = _LABEL_MOD.get(m.strip().lower())
        if mm is None:
            return None
        ms |= set(MODS) if mm == "hyper" else {mm}
    if key.strip().lower() in _LABEL_MOD:            # modifier alone ("Shift")
        return None
    return stroke(ms, key.strip())


def app_label(text: str) -> tuple:
    """A [shortcuts] key label → (chords, gesture_parts).

    "Cmd+Plus / Cmd+Minus", "Ctrl+Shift+H / J / K / L" (bare alternatives
    inherit the previous modifiers), "P D A S R C M T B I" (several keys),
    "Esc Esc" / "Ctrl+B L" (sequences), "1-9" (a range), "↑ ↓ / Ctrl+N".
    """
    chords, gestures = [], []
    prev_mods = None
    for alt in [a.strip() for a in text.split(" / ")]:
        if not alt:
            continue
        if GESTURE.search(alt):
            gestures.append(alt)
            continue
        toks = alt.split()
        if len(toks) > 1 and len(set(toks)) == 1:      # "Esc Esc"
            st = _label_stroke(toks[0])
            if st:
                chords.append(Chord(tuple([st] * len(toks))))
            else:
                gestures.append(alt)
            continue
        if len(toks) == 2:                              # "Ctrl+B L": a prefix, then a key
            first, then = _label_stroke(toks[0]), _label_stroke(toks[1])
            if first and then and first.mods:
                chords.append(Chord((first, then)))
                continue
        got_any = False
        for tok in toks:
            m = re.fullmatch(r"(\d)-(\d)", tok)
            if m:
                for d in range(int(m.group(1)), int(m.group(2)) + 1):
                    chords.append(chord(stroke([], str(d))))
                got_any = True
                continue
            st = _label_stroke(tok)
            if st is None:
                gestures.append(tok)
                continue
            if not st.mods and prev_mods and len(toks) == 1 and len(st.key) == 1 and st.key.isalnum():
                st = Stroke(prev_mods, st.key)          # "/ J / K / L"
            chords.append(chord(st))
            got_any = True
            if st.mods:
                prev_mods = st.mods
        if not got_any and not toks:
            gestures.append(alt)
    return chords, gestures


# ------------------------------------------------------------- human input
def query_mods(text: str) -> tuple:
    """Search text → (modifier set it names, the rest). "hyper t" →
    ({ctrl,alt,shift,cmd}, "t"); "⌘⇧k" → ({cmd,shift}, "k")."""
    mods, rest = set(), []
    for tok in re.split(r"[\s+]+", text.strip()):
        if not tok:
            continue
        low = tok.lower()
        if low == "hyper":
            mods |= set(MODS)
            continue
        if low in ("cmd", "command", "super", "ctrl", "control", "opt", "option", "alt", "shift"):
            mods.add(MOD_WORDS[low])
            continue
        sym = set()
        while tok and tok[0] in "⌃⌥⇧⌘":
            sym.add(MOD_WORDS[tok[0]])
            tok = tok[1:]
        mods |= sym
        if tok:
            rest.append(tok)
    return mods, " ".join(rest)


def parse_human(text: str) -> Chord | None:
    """`hyper+/`, `cmd+shift+k`, `⌘⇧K`, `alt-h` (aerospace style) → Chord."""
    t = text.strip()
    if not t:
        return None
    mods, rest = query_mods(t.replace("-", " ") if re.fullmatch(r"[a-z]+(-[a-z0-9]+)+", t) else t)
    if not rest or " " in rest:
        return None
    return chord(stroke(mods, rest))
