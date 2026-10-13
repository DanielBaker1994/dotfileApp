"""The file-based interface: read an input file, write the transformed table.

    python3 -m text_table INPUT OUTPUT [--format NAME] [--fallback-out PATH]
                          [--meta PATH] [--no-header] [--delimiter NAME]
    python3 -m text_table --list-formats [--json]

``INPUT`` / ``OUTPUT`` may be ``-`` for stdin / stdout (Neovim filters:
``:'<,'>!python3 -m text_table - - --format markdown``).

``--meta`` writes a JSON summary the caller can show or act on::

    {"ok": true, "format": "outlook", "mime": "text/html",
     "fallbackFormat": "markdown", "delimiter": "tab", "delimiterLabel": "tab",
     "columns": 4, "rows": 12, "warnings": ["1 short row padded"]}

Exit status: 0 = written; 1 = no table in the input (outputs are written
empty, meta has ``"ok": false``); 2 = usage error.
"""
from __future__ import annotations

import argparse
import json
import os
import sys
import tempfile
from typing import List, Optional

from .formats import DEFAULT_FORMAT, FORMATS, get_format
from .parse import DELIMITER_BY_NAME, parse_text


def _read(path: str) -> str:
    if path == "-":
        return sys.stdin.read()
    with open(path, encoding="utf-8", errors="replace") as fh:
        return fh.read()


def _write(path: str, text: str) -> None:
    """Atomic write (tmp + rename) so a reader never sees half a file."""
    if path == "-":
        sys.stdout.write(text)
        sys.stdout.flush()
        return
    d = os.path.dirname(os.path.abspath(path))
    os.makedirs(d, exist_ok=True)
    fd, tmp = tempfile.mkstemp(dir=d, prefix=".text_table-")
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as fh:
            fh.write(text)
        os.replace(tmp, path)
    except BaseException:
        try:
            os.unlink(tmp)
        except OSError:
            pass
        raise


def _parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(
        prog="python3 -m text_table",
        description="Turn OCR'd or pasted text into a table (Outlook HTML, Markdown, TSV, ...).",
    )
    p.add_argument("input", nargs="?", help="input text file, or - for stdin")
    p.add_argument("output", nargs="?", help="output file, or - for stdout")
    p.add_argument("-f", "--format", default=DEFAULT_FORMAT, help=f"one of: {', '.join(FORMATS)} (default {DEFAULT_FORMAT})")
    p.add_argument("--fallback-out", metavar="PATH", help="also write the format's plain-text fallback (rich formats only)")
    p.add_argument("--meta", metavar="PATH", help="write a JSON summary (delimiter, size, warnings)")
    p.add_argument("--no-header", action="store_true", help="the first row is data; name columns Column 1..N")
    p.add_argument("--delimiter", choices=sorted(DELIMITER_BY_NAME), help="force a delimiter instead of detecting one")
    p.add_argument("--list-formats", action="store_true", help="list the output formats and exit")
    p.add_argument("--json", action="store_true", help="with --list-formats: JSON output")
    return p


def main(argv: Optional[List[str]] = None) -> int:
    args = _parser().parse_args(argv)

    if args.list_formats:
        rows = [
            {"name": f.name, "label": f.label, "mime": f.mime, "fallback": f.fallback, "description": f.description}
            for f in FORMATS.values()
        ]
        if args.json:
            print(json.dumps(rows))
        else:
            for r in rows:
                print(f"{r['name']:<10} {r['label']:<11} {r['description']}")
        return 0

    if not args.input or not args.output:
        _parser().print_usage(sys.stderr)
        print("error: INPUT and OUTPUT are required", file=sys.stderr)
        return 2
    try:
        fmt = get_format(args.format)
    except ValueError as e:
        print(f"error: {e}", file=sys.stderr)
        return 2

    try:
        text = _read(args.input)
    except OSError as e:
        print(f"error: {e}", file=sys.stderr)
        return 2

    table = parse_text(text, has_header=not args.no_header, delimiter=args.delimiter)
    out = fmt.render(table)
    _write(args.output, out)
    fallback = fmt.fallback or fmt.name
    if args.fallback_out:
        _write(args.fallback_out, get_format(fallback).render(table) if fmt.fallback else out)

    ok = not table.is_empty()
    if args.meta:
        meta = {
            "ok": ok,
            "format": fmt.name,
            "mime": fmt.mime,
            "fallbackFormat": fallback,
            "delimiter": table.delimiter,
            "delimiterLabel": table.delimiter_label,
            "columns": table.columns,
            "rows": len(table.rows),
            "warnings": table.warnings,
        }
        _write(args.meta, json.dumps(meta) + "\n")
    return 0 if ok else 1
