"""Input parsing: OCR / pasted text -> a normalized :class:`Table`.

Pure and stdlib-only (Python 3.9+), so Neovim or any shell can import it
without the kitchen-sink helper. No knowledge of Outlook or any UI lives here;
`formats.py` turns the Table into output text.

Pipeline:

1. clean    — NBSP / zero-width / smart-dash cleanup, trailing space, blanks;
2. rules    — drop separator rows (``---``, ``——``, ``|:--|--:|``, ``+--+``,
              ``====``) that carry no content;
3. detect   — score every candidate delimiter by how *consistently* it splits
              the rows (the same column count on most lines), not by how often
              it occurs;
4. split    — split each row with the winner;
5. normalize — trim cells, pad short rows, fold overflow into the last cell.

The first remaining row is the header.
"""
from __future__ import annotations

import csv
import re
from collections import Counter
from dataclasses import dataclass, field
from typing import Callable, List, Optional, Sequence

# -- cleanup ----------------------------------------------------------------

# Characters OCR and web copies sprinkle in: NBSP and friends -> space,
# zero-width marks -> nothing.
_SPACES = dict.fromkeys(map(ord, "     　"), " ")
_ZERO_WIDTH = dict.fromkeys(map(ord, "​‌‍﻿"), None)

# Dash-like characters (hyphen, en/em dash, minus, box-drawing horizontals).
DASHES = "-‐‑‒–—―−─━┄┅┈┉╌╍═_"
_DASH_CLASS = "[" + re.escape(DASHES) + "]"

# A row made only of rule characters (at least three dashes / equals).
_RULE_CHARS = set(DASHES + "=|+:│┃┼╋╪╫╬├┤┬┴ \t")
_RULE_MIN_RUN = re.compile("(?:" + _DASH_CLASS + "|=){3,}")


def clean_lines(text: str) -> List[str]:
    """Normalize whitespace oddities and drop blank lines (tabs are kept:
    they are the strongest column signal)."""
    text = text.replace("\r\n", "\n").replace("\r", "\n")
    text = text.translate(_SPACES).translate(_ZERO_WIDTH)
    out = []
    for line in text.split("\n"):
        line = line.rstrip()
        if line.strip():
            out.append(line)
    return out


def is_rule_row(line: str) -> bool:
    """A separator row with no content: ``|---|:--:|``, ``-----``, ``——``,
    ``+----+----+``, ``=====``. Must contain a run of 3+ dashes / equals."""
    s = line.strip()
    if not s or not _RULE_MIN_RUN.search(s):
        return False
    return all(ch in _RULE_CHARS for ch in s)


# -- delimiters ---------------------------------------------------------------


@dataclass(frozen=True)
class Delimiter:
    """One candidate column separator.

    ``priority`` breaks near-ties (higher wins): an explicit tab beats a pipe
    beats a comma … beats a run of spaces, which beats a single space.
    """

    name: str
    label: str
    priority: float
    split: Callable[[str], List[str]]


def _split_regex(pattern: str) -> Callable[[str], List[str]]:
    rx = re.compile(pattern)
    return lambda line: rx.split(line.strip())


def _split_pipe(line: str) -> List[str]:
    s = line.strip()
    if s.startswith("|"):
        s = s[1:]
    if s.endswith("|") and not s.endswith("\\|"):
        s = s[:-1]
    # `\|` is an escaped pipe inside a Markdown cell.
    parts = re.split(r"(?<!\\)\|", s)
    return [p.replace("\\|", "|") for p in parts]


def _split_csv(sep: str) -> Callable[[str], List[str]]:
    def split(line: str) -> List[str]:
        try:
            return next(csv.reader([line.strip()], delimiter=sep, skipinitialspace=True))
        except (csv.Error, StopIteration):
            return line.split(sep)

    return split


DELIMITERS: Sequence[Delimiter] = (
    # One tab = one boundary: `a\t\tc` has an empty middle cell (spreadsheet
    # copies and the OCR column join both emit single tabs).
    Delimiter("tab", "tab", 1.00, lambda line: line.strip(" ").split("\t")),
    Delimiter("pipe", "| pipe", 0.95, _split_pipe),
    # Spaced dash runs used as separators: `Name —— Value`, `a -- b`.
    Delimiter("dashes", "— dash run", 0.85, _split_regex(r"\s*" + _DASH_CLASS + r"{2,}\s*")),
    Delimiter("semicolon", "; semicolon", 0.80, _split_csv(";")),
    Delimiter("comma", ", comma", 0.75, _split_csv(",")),
    # Colon followed by space / end of line: `Owner: Dan` — never `10:30`
    # or `https://`.
    Delimiter("colon", ": colon", 0.70, _split_regex(r"\s*:(?=\s|$)\s*")),
    # A spaced single dash / bullet: `Name - Value`, `a • b`.
    Delimiter("spaced-dash", "spaced dash", 0.60, _split_regex(r"\s+[\-–—•·]\s+")),
    Delimiter("spaces", "2+ spaces", 0.50, _split_regex(r"\s{2,}")),
    Delimiter("space", "single space", 0.10, _split_regex(r"\s+")),
)

DELIMITER_BY_NAME = {d.name: d for d in DELIMITERS}

_WEAK = {"comma", "semicolon", "colon", "spaced-dash", "dashes"}


@dataclass
class Score:
    delimiter: Delimiter
    columns: int  # the modal column count (>= 2 to count at all)
    consistency: float  # share of rows that have exactly `columns` cells
    coverage: float  # share of rows the delimiter splits at all
    header_fits: bool
    value: float


def _strip_cells(cells: List[str]) -> List[str]:
    cells = [c.strip() for c in cells]
    # A trailing empty cell from `a, b,` is noise, not a column.
    while len(cells) > 1 and cells[-1] == "":
        cells.pop()
    return cells


def score(delim: Delimiter, lines: Sequence[str], guard: bool = True) -> Optional[Score]:
    counts = [len(_strip_cells(delim.split(line))) for line in lines]
    split_counts = [c for c in counts if c >= 2]
    if not split_counts:
        return None
    columns, modal = Counter(split_counts).most_common(1)[0]
    # Prefer the larger column count when two counts tie.
    for c, n in Counter(split_counts).items():
        if n == modal and c > columns:
            columns = c
    n = len(lines)
    consistency = sum(1 for c in counts if c == columns) / n
    coverage = len(split_counts) / n
    header_fits = counts[0] == columns
    value = (
        0.60 * consistency
        + 0.25 * coverage
        + 0.05 * (1.0 if header_fits else 0.0)
        + 0.10 * delim.priority
    )
    if not guard:
        return Score(delim, columns, consistency, coverage, header_fits, value)
    # A single space splits everything; only trust it when it is perfectly
    # regular (every row the same count), otherwise it splits prose.
    if delim.name == "space" and consistency < 1.0:
        return None
    # A run of 2+ spaces must explain most rows to beat nothing.
    if delim.name == "spaces" and consistency < 0.5:
        return None
    # Punctuation that also lives in prose (commas, colons, dashes) only
    # counts when the header row splits too and most rows agree.
    if delim.name in _WEAK and (not header_fits or consistency < 0.5):
        return None
    return Score(delim, columns, consistency, coverage, header_fits, value)


def detect_delimiter(lines: Sequence[str]) -> Optional[Score]:
    """The best-scoring delimiter, or ``None`` when no candidate splits any
    row into two or more cells (single-column input)."""
    best: Optional[Score] = None
    for d in DELIMITERS:
        s = score(d, lines)
        if s and (best is None or s.value > best.value + 1e-9):
            best = s
    return best


# -- the table ----------------------------------------------------------------


@dataclass
class Table:
    header: List[str]
    rows: List[List[str]]
    delimiter: Optional[str] = None  # Delimiter.name, None = single column
    delimiter_label: str = "none"
    warnings: List[str] = field(default_factory=list)

    @property
    def columns(self) -> int:
        return len(self.header)

    def is_empty(self) -> bool:
        return not self.header and not self.rows

    def numeric_columns(self) -> List[bool]:
        """Columns whose non-empty body cells all look numeric (for right
        alignment): ``12``, ``-3.5``, ``1,234``, ``$9.99``, ``45%``."""
        out = []
        for i in range(self.columns):
            vals = [r[i] for r in self.rows if i < len(r) and r[i]]
            out.append(bool(vals) and all(_NUMERIC.match(v) for v in vals))
        return out


_NUMERIC = re.compile(r"^[\(\-+]?[$€£¥]?\s?\d[\d,]*(\.\d+)?\s?[%kKmMbB]?\)?$")
_SPACE_RUN = re.compile(r"[ ]{2,}")


def _normalize_cell(cell: str) -> str:
    return _SPACE_RUN.sub(" ", cell.strip())


def parse_text(text: str, has_header: bool = True, delimiter: Optional[str] = None) -> Table:
    """Parse OCR / pasted text into a :class:`Table`.

    ``delimiter`` forces a candidate by name (see :data:`DELIMITERS`) instead
    of detecting one. Never raises on messy input: problems become
    ``Table.warnings``.
    """
    warnings: List[str] = []
    lines = clean_lines(text)
    rules = [ln for ln in lines if is_rule_row(ln)]
    lines = [ln for ln in lines if not is_rule_row(ln)]
    if rules:
        warnings.append(f"dropped {len(rules)} separator row{'s' if len(rules) != 1 else ''}")
    if not lines:
        return Table([], [], warnings=warnings + ["no text"])

    picked: Optional[Score]
    if delimiter:
        d = DELIMITER_BY_NAME.get(delimiter)
        if d is None:
            raise ValueError(f"unknown delimiter {delimiter!r}; one of: {', '.join(DELIMITER_BY_NAME)}")
        picked = score(d, lines, guard=False) or Score(d, 1, 1.0, 0.0, True, 0.0)
    else:
        picked = detect_delimiter(lines)

    if picked is None:
        cells = [[_normalize_cell(ln)] for ln in lines]
        warnings.append("no column delimiter found; one column")
        name, label, ncols = None, "none", 1
    else:
        split = picked.delimiter.split
        cells = [[_normalize_cell(c) for c in _strip_cells(split(ln))] for ln in lines]
        name, label, ncols = picked.delimiter.name, picked.delimiter.label, max(picked.columns, 1)

    padded, merged = 0, 0
    rows: List[List[str]] = []
    for r in cells:
        if len(r) < ncols:
            r = r + [""] * (ncols - len(r))
            padded += 1
        elif len(r) > ncols:
            r = r[: ncols - 1] + [" ".join(c for c in r[ncols - 1 :] if c)]
            merged += 1
        rows.append(r)
    if padded:
        warnings.append(f"{padded} short row{'s' if padded != 1 else ''} padded")
    if merged:
        warnings.append(f"{merged} long row{'s' if merged != 1 else ''}: extra cells joined into the last column")

    if has_header:
        header, body = rows[0], rows[1:]
    else:
        header, body = [f"Column {i + 1}" for i in range(ncols)], rows
    return Table(header, body, name, label, warnings)
