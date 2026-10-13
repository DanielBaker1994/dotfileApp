"""Output transformation: a parsed :class:`~text_table.parse.Table` -> text.

Every destination is one :class:`OutputFormat` in :data:`FORMATS`; add a new
one by writing a ``render(table) -> str`` and registering it. Parsing never
changes for a new format.

``mime`` tells the integration layer how to put the result on the clipboard:
``text/plain`` goes through ``pbcopy`` as-is; ``text/html`` is written as the
rich ``public.html`` flavor, with the ``fallback`` format's text as the plain
flavor (what a terminal or plain-text field pastes).
"""
from __future__ import annotations

import csv
import html
import io
import unicodedata
from dataclasses import dataclass
from typing import Callable, Dict, List, Optional

from .parse import Table


@dataclass(frozen=True)
class OutputFormat:
    name: str
    label: str
    mime: str
    description: str
    render: Callable[[Table], str]
    fallback: Optional[str] = None  # plain-text twin for a rich format


# -- helpers ------------------------------------------------------------------


def _width(s: str) -> int:
    """Display width: East Asian wide / fullwidth chars take two cells."""
    return sum(2 if unicodedata.east_asian_width(ch) in "WF" else 1 for ch in s)


def _pad(s: str, width: int, right: bool = False) -> str:
    gap = " " * max(width - _width(s), 0)
    return gap + s if right else s + gap


def _col_widths(table: Table, cell: Callable[[str], str], minimum: int = 3) -> List[int]:
    widths = [max(minimum, _width(cell(h))) for h in table.header]
    for r in table.rows:
        for i, c in enumerate(r):
            widths[i] = max(widths[i], _width(cell(c)))
    return widths


# -- Markdown (GFM pipe table) -------------------------------------------------


def _md_cell(s: str) -> str:
    return s.replace("\\", "\\\\").replace("|", "\\|")


def render_markdown(table: Table) -> str:
    if table.is_empty():
        return ""
    numeric = table.numeric_columns()
    widths = _col_widths(table, _md_cell)

    def line(cells: List[str]) -> str:
        return "| " + " | ".join(
            _pad(_md_cell(c), widths[i], numeric[i]) for i, c in enumerate(cells)
        ) + " |"

    rule = "| " + " | ".join(
        ("-" * (w - 1) + ":") if numeric[i] else "-" * w for i, w in enumerate(widths)
    ) + " |"
    return "\n".join([line(table.header), rule] + [line(r) for r in table.rows]) + "\n"


# -- Outlook (inline-styled HTML) -------------------------------------------------

# Outlook (new + legacy, Mac + Windows) keeps inline `style=""` but drops
# `<style>` blocks and classes, so every cell carries its own style.
_OUTLOOK_TABLE = (
    "border-collapse:collapse;border:1px solid #bfbfbf;"
    "font-family:Aptos,Calibri,Arial,sans-serif;font-size:11pt;"
)
_OUTLOOK_TH = (
    "border:1px solid #bfbfbf;padding:4px 8px;background:#f2f2f2;"
    "font-weight:bold;text-align:left;vertical-align:top;"
)
_OUTLOOK_TD = "border:1px solid #bfbfbf;padding:4px 8px;vertical-align:top;"


def _html_table(table: Table, styled: bool) -> str:
    if table.is_empty():
        return ""
    numeric = table.numeric_columns()
    esc = lambda s: html.escape(s, quote=False)  # noqa: E731

    def style(base: str, i: int) -> str:
        if not styled:
            return ' style="text-align:right"' if numeric[i] else ""
        return f' style="{base}{"text-align:right;" if numeric[i] else ""}"'

    out = [f'<table style="{_OUTLOOK_TABLE}">' if styled else "<table>"]
    out.append("<thead><tr>")
    out.extend(f"<th{style(_OUTLOOK_TH, i)}>{esc(h)}</th>" for i, h in enumerate(table.header))
    out.append("</tr></thead>")
    out.append("<tbody>")
    for r in table.rows:
        out.append("<tr>")
        out.extend(f"<td{style(_OUTLOOK_TD, i)}>{esc(c)}</td>" for i, c in enumerate(r))
        out.append("</tr>")
    out.append("</tbody></table>")
    return "".join(out)


def render_outlook(table: Table) -> str:
    body = _html_table(table, styled=True)
    if not body:
        return ""
    # A full document with a charset: the pasteboard's HTML flavor is read
    # as Latin-1 by some apps without it.
    return '<html><head><meta charset="utf-8"></head><body>' + body + "</body></html>\n"


def render_html(table: Table) -> str:
    body = _html_table(table, styled=False)
    return body + "\n" if body else ""


# -- delimited / plain text -------------------------------------------------------


def _render_delimited(table: Table, sep: str) -> str:
    if table.is_empty():
        return ""
    buf = io.StringIO()
    w = csv.writer(buf, delimiter=sep, lineterminator="\n")
    w.writerow(table.header)
    w.writerows(table.rows)
    return buf.getvalue()


def render_tsv(table: Table) -> str:
    if table.is_empty():
        return ""
    clean = lambda s: s.replace("\t", " ").replace("\n", " ")  # noqa: E731
    lines = ["\t".join(map(clean, table.header))]
    lines += ["\t".join(map(clean, r)) for r in table.rows]
    return "\n".join(lines) + "\n"


def render_csv(table: Table) -> str:
    return _render_delimited(table, ",")


def render_grid(table: Table) -> str:
    """A monospace box table for plain-text mail, chat or a terminal."""
    if table.is_empty():
        return ""
    numeric = table.numeric_columns()
    widths = _col_widths(table, lambda s: s, minimum=1)
    rule = "+" + "+".join("-" * (w + 2) for w in widths) + "+"
    head_rule = "+" + "+".join("=" * (w + 2) for w in widths) + "+"

    def line(cells: List[str], align: bool) -> str:
        return "| " + " | ".join(
            _pad(c, widths[i], align and numeric[i]) for i, c in enumerate(cells)
        ) + " |"

    out = [rule, line(table.header, False), head_rule]
    out += [line(r, True) for r in table.rows]
    out.append(rule)
    return "\n".join(out) + "\n"


def _jira_cell(s: str) -> str:
    # `|` splits cells and an empty cell collapses; a space keeps it.
    return s.replace("|", "\\|") or " "


def render_jira(table: Table) -> str:
    """Jira / Confluence wiki markup (``||head||`` / ``|cell|``)."""
    if table.is_empty():
        return ""
    lines = ["||" + "||".join(_jira_cell(h) for h in table.header) + "||"]
    lines += ["|" + "|".join(_jira_cell(c) for c in r) + "|" for r in table.rows]
    return "\n".join(lines) + "\n"


# -- the registry -------------------------------------------------------------------

FORMATS: Dict[str, OutputFormat] = {
    f.name: f
    for f in (
        OutputFormat(
            "outlook",
            "Outlook",
            "text/html",
            "Bordered HTML table with inline styles (Outlook, Teams, Word, Gmail); plain fallback = Markdown",
            render_outlook,
            fallback="markdown",
        ),
        OutputFormat("markdown", "Markdown", "text/plain", "GitHub-flavored pipe table, padded", render_markdown),
        OutputFormat("tsv", "TSV", "text/plain", "Tab-separated (Excel, Numbers, Sheets)", render_tsv),
        OutputFormat("csv", "CSV", "text/plain", "Comma-separated, quoted as needed", render_csv),
        OutputFormat("grid", "Text grid", "text/plain", "Monospace box table for plain-text mail / chat", render_grid),
        OutputFormat("jira", "Jira wiki", "text/plain", "Jira / Confluence wiki markup", render_jira),
        OutputFormat(
            "html",
            "HTML",
            "text/html",
            "Bare semantic <table> (no styling); plain fallback = Markdown",
            render_html,
            fallback="markdown",
        ),
    )
}

DEFAULT_FORMAT = "outlook"


def get_format(name: str) -> OutputFormat:
    try:
        return FORMATS[name.strip().lower()]
    except KeyError:
        raise ValueError(f"unknown format {name!r}; one of: {', '.join(FORMATS)}") from None


def render(table: Table, name: str = DEFAULT_FORMAT) -> str:
    return get_format(name).render(table)
