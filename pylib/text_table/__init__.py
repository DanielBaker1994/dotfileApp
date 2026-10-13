"""text_table — turn OCR'd / pasted text into a table in a chosen format.

Standalone and UI-free: the kitchen-sink "Text to Table" panel, Neovim, or a
shell all use the same file-based CLI::

    python3 -m text_table IN.txt OUT.html --format outlook [--fallback-out OUT.md] [--meta META.json]

or the library::

    from text_table import parse_text, render
    render(parse_text(text), "markdown")

Responsibilities are split: ``parse`` (delimiter detection + normalization),
``formats`` (the destination registry), ``cli`` (files in, files out). The
clipboard and the UI belong to the caller.
"""
from __future__ import annotations

from .formats import DEFAULT_FORMAT, FORMATS, OutputFormat, get_format, render
from .parse import DELIMITERS, Table, detect_delimiter, parse_text

__all__ = [
    "DEFAULT_FORMAT",
    "DELIMITERS",
    "FORMATS",
    "OutputFormat",
    "Table",
    "detect_delimiter",
    "get_format",
    "parse_text",
    "render",
    "transform",
]


def transform(text: str, fmt: str = DEFAULT_FORMAT, has_header: bool = True, delimiter: str | None = None) -> str:
    """Parse ``text`` and render it as ``fmt`` in one call."""
    return render(parse_text(text, has_header=has_header, delimiter=delimiter), fmt)
