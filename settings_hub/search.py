"""Filtering: fuzzy text, chord-aware tokens ("hyper t", "⌘⇧k"), the exact
modifier toggles and favorites. Higher score = better; None = no match."""
from __future__ import annotations

from . import chords
from .model import KeyRow, SettingRow


def _token_score(tok: str, hay: str) -> int | None:
    if not tok:
        return 0
    i = hay.find(tok)
    if i >= 0:
        word_start = i == 0 or not hay[i - 1].isalnum()
        return 30 + (20 if word_start else 0) - min(i, 20) // 4
    # subsequence, kept tight: inside ONE word, span ≤ 3× the token, ≤ 2 gaps
    if len(tok) < 3:
        return None
    best = None
    start = hay.find(tok[0])
    while start >= 0:
        j, gaps, last = start + 1, 0, start
        for ch in tok[1:]:
            k = hay.find(ch, j)
            if k < 0 or k - start > 3 * len(tok) or " " in hay[start:k]:
                gaps = 99
                break
            if k != last + 1:
                gaps += 1
            last, j = k, k + 1
        if gaps <= 2:
            best = max(best or 0, 12 - gaps * 3)
        start = hay.find(tok[0], start + 1)
    return best


def haystack(row) -> str:
    if isinstance(row, KeyRow):
        return " ".join([row.display, " ".join(c.text for c in row.chords), row.chord_text,
                         row.action, row.view, row.layer, row.doc]).lower()
    return " ".join([f"[{row.section}]", row.section, row.key, row.value, row.doc,
                     row.line_doc]).lower()


def chord_score(row, query: str) -> int | None:
    """A query that names modifiers ("hyper t", "cmd k", "⌘⇧z") matched
    against the row's keys; None = not a chord query / no hit."""
    if not isinstance(row, KeyRow):
        return None
    mods, rest = chords.query_mods(query.strip())
    if not mods:
        return None
    want_key = chords.key_name(rest) if rest and " " not in rest else ""
    hit = [c for c in row.chords if mods <= c.mods and (not want_key or c.strokes[-1].key == want_key)]
    if not hit:
        return None
    return 100 + (50 if any(c.mods == mods for c in hit) else 0)


def score(row, query: str) -> int | None:
    q = query.strip()
    if not q:
        return 0
    total = 0
    hay = haystack(row)
    # the row's name (keys / setting key) counts more than its description
    name = (row.display + " " + row.chord_text if isinstance(row, KeyRow)
            else f"{row.section} {row.key}").lower()
    for tok in q.lower().split():
        s = _token_score(tok, hay)
        if s is None:
            return None
        if tok in name:
            s += 40
        total += s
    return total


def mods_match(row, mods: set) -> bool:
    """The exact-modifier toggles (KeyMinder): a chord using exactly these."""
    if not mods:
        return True
    if not isinstance(row, KeyRow):
        return False
    return any(c.mods == frozenset(mods) for c in row.chords)


def filter_rows(rows: list, query: str = "", mods: set | None = None,
                favorites: set | None = None) -> list:
    out = []
    # a modifier query that hits real keys shows only those (no text noise)
    if query.strip() and any(chord_score(r, query) for r in rows):
        for n, r in enumerate(rows):
            if favorites is not None and r.id not in favorites:
                continue
            if mods and not mods_match(r, mods):
                continue
            cs = chord_score(r, query)
            if cs:
                out.append((-cs, n, r))
        out.sort(key=lambda t: (t[0], t[1]))
        return [r for _, _, r in out]
    for n, r in enumerate(rows):
        if favorites is not None and r.id not in favorites:
            continue
        if mods and not mods_match(r, mods):
            continue
        s = score(r, query)
        if s is None:
            continue
        out.append((-s, n, r))
    if query.strip():
        out.sort(key=lambda t: (t[0], t[1]))
    return [r for _, _, r in out]
