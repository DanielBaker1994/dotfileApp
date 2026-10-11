#!/usr/bin/env python3
"""snippet render — every nvim notes snippet expands and renders through pandoc
(PDF + screen HTML), then the DOMs are compared. Ported from
test_snippet_render.swift (the Swift app tree is gone; the test only ever drove
nvim, pandoc and the prose_pdf helper methods).

    python3 Tests/test_snippet_render.py
"""
from __future__ import annotations

import os
import shutil
import subprocess
import sys
import tempfile
import time
from concurrent.futures import ThreadPoolExecutor

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
HERE = os.path.join(ROOT, "Tests", "snippet_render")
sys.path.insert(0, os.path.join(ROOT, "pylib"))

import prose_pdf  # noqa: E402  (after the path tweak)

PANDOC = "/opt/homebrew/bin/pandoc"
ENGINE = "/opt/homebrew/bin/weasyprint"


def engine_python() -> str:
    """The interpreter weasyprint runs under (its shebang) — dom_compare needs it."""
    wp = os.path.realpath(ENGINE)
    try:
        with open(wp, encoding="utf-8") as fh:
            first = fh.readline().strip()
    except OSError:
        return ""
    return first[2:] if first.startswith("#!") else ""


def main() -> int:
    nvim = next((p for p in ("/opt/homebrew/bin/nvim", "/usr/local/bin/nvim") if os.access(p, os.X_OK)), None)
    python = engine_python()
    if not nvim or not os.access(PANDOC, os.X_OK) or not (python and os.access(python, os.X_OK)):
        print("  (nvim / pandoc / weasyprint missing: snippet render skipped)")
        return 0
    diagrams = os.path.expanduser("~/.dotfiles/markdown_generator/diagrams.lua")
    tmp = tempfile.mkdtemp(prefix=f"snippet-render-{os.getpid()}-")
    try:
        config = {"pandoc": PANDOC, "engine": ENGINE,
                  "filter": diagrams if os.path.exists(diagrams) else "none",
                  "cacheDir": os.path.join(tmp, "cache")}
        cfg = prose_pdf.config(config)
        css = prose_pdf.css_content(cfg)
        env = {**os.environ, "WS_REPO": ROOT, "OUT": tmp}
        ex = subprocess.run([nvim, "--headless", "--clean", "-u", "NONE",
                             "-c", f"luafile {HERE}/expand.lua", "-c", "qa!"],
                            capture_output=True, text=True, env=env)
        if "EXPAND FAIL" in ex.stderr:
            print("  FAIL: " + ex.stderr)
            return 1
        notes = sorted(n for n in os.listdir(tmp) if n.endswith(".md"))
        css_path = os.path.join(tmp, "header.css")
        with open(css_path, "w", encoding="utf-8") as fh:
            fh.write(css)

        def render(name: str) -> int:
            note = os.path.join(tmp, name)
            stem = os.path.splitext(note)[0]
            bad = 0
            for sourcepos, ext in ((False, "pdf"), (True, "scr")):
                args = prose_pdf.pandoc_args(note, css_path, f"{stem}.{ext}.html", cfg, sourcepos)
                p = subprocess.run([PANDOC, *args], capture_output=True, text=True)
                if p.returncode != 0:
                    print(f"  FAIL: pandoc ({ext}) on {name}: {p.stderr.strip()}")
                    bad += 1
            return bad

        t0 = time.time()
        with ThreadPoolExecutor(max_workers=os.cpu_count() or 4) as pool:
            failed = sum(pool.map(render, notes))
        print(f"  rendered {len(notes)} snippets × 2 in {int((time.time() - t0) * 1000)} ms")
        cmp = subprocess.run([python, "-I", os.path.join(HERE, "dom_compare.py"), tmp],
                             capture_output=True, text=True)
        print(cmp.stdout + cmp.stderr, end="")
        return 0 if failed == 0 and cmp.returncode == 0 else 1
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
