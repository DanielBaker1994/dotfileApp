from __future__ import annotations

import os
import subprocess
import uuid
from pathlib import Path

DEFAULTS = {
    "pandoc": "/opt/homebrew/bin/pandoc",
    "engine": "/opt/homebrew/bin/weasyprint",
    "css": "",
    "outDir": "~/Downloads",
    "highlight": "tango",
    "filter": "",
    "themeCSS": "",
    "cacheDir": os.path.expanduser("~/.cache/kitchen-sink/prose"),
}

SOURCEPOS_FILTER = """function Span(el)
  if el.attributes.wrapper == "1" and #el.content > 0 then
    for _, x in ipairs(el.content) do
      if x.t ~= "RawInline" then return nil end
    end
    return el.content
  end
end

local box = { ["☐"] = true, ["☒"] = true }
function BulletList(el)
  for i, item in ipairs(el.content) do
    local a, b = item[1], item[2]
    if a and b and a.t == "Plain" and #a.content == 1 and a.content[1].t == "Str"
        and box[a.content[1].text] and b.t == "Div" and b.content[1]
        and (b.content[1].t == "Plain" or b.content[1].t == "Para") then
      local first = b.content[1]
      local inl = pandoc.List({ a.content[1], pandoc.Space() })
      inl:extend(first.content)
      first.content = inl
      local blocks = pandoc.List({ first })
      for j = 2, #b.content do blocks:insert(b.content[j]) end
      for j = 3, #item do blocks:insert(item[j]) end
      el.content[i] = blocks
    end
  end
  return el
end
"""

BUILTIN_CSS = """<style>
@page { size: A4; margin: 18mm 16mm; }
html { color: #1a1a1a; background: #fff; }
body { margin: 0 auto; max-width: 46em; font-family: "Helvetica Neue", Helvetica, Arial, sans-serif;
       font-size: 10.5pt; line-height: 1.55; hyphens: auto; overflow-wrap: break-word; }
h1, h2, h3, h4 { line-height: 1.25; margin: 1.3em 0 .5em; page-break-after: avoid; }
h1 { font-size: 1.7em; margin-top: 0; } h2 { font-size: 1.3em; } h3 { font-size: 1.1em; }
a { color: #0969da; text-decoration: none; }
code { font-family: Menlo, "SF Mono", monospace; font-size: .88em; background: #f0f1f3;
       padding: .1em .3em; border-radius: 3px; }
pre { background: #f6f8fa; border: 1px solid #d0d7de; border-radius: 5px; padding: 8px 10px;
      white-space: pre-wrap; line-height: 1.4; page-break-inside: avoid; }
pre code { background: none; padding: 0; }
blockquote { margin: 0 0 .9em; padding-left: 12px; border-left: 3px solid #c8c8c8; color: #555; }
table { border-collapse: collapse; margin: .4em 0 1em; }
th, td { border: 1px solid #bfbfbf; padding: 4px 10px; vertical-align: top; }
th { background: #f2f2f2; text-align: left; }
img { max-width: 100%; }
div.note, div.tip, div.important, div.warning, div.caution { margin: .8em 0; padding: .5em 1em .2em;
      border-left: 3px solid var(--a); background: var(--bg); border-radius: 4px; }
div.note { --a: #0969da; --bg: #eef5fd; } div.tip { --a: #1a7f37; --bg: #eef8f0; }
div.important { --a: #8250df; --bg: #f4effc; } div.warning { --a: #9a6700; --bg: #fdf6e6; }
div.caution { --a: #cf222e; --bg: #fdeff0; }
div.note > .title p, div.tip > .title p, div.important > .title p, div.warning > .title p,
div.caution > .title p { margin: 0 0 .3em; font-weight: bold; color: var(--a); }
</style>
"""


def config(raw) -> dict:
    merged = dict(DEFAULTS)
    for key, value in (raw or {}).items():
        if key in merged and value is not None:
            merged[key] = value
    return merged


def expand(path: str) -> str:
    return os.path.expanduser(path)


def output_path(note: str, c: dict) -> str:
    stem = os.path.splitext(os.path.basename(note))[0]
    return os.path.join(expand(c["outDir"]), (stem or "note") + ".pdf")


def filter_paths(c: dict) -> list:
    want = c["filter"].strip()
    if want.lower() == "none":
        return []
    parts = [p.strip() for p in want.split(",")] if want else []
    items = [expand(p) for p in parts if p]
    if not items and c["css"]:
        items = [os.path.join(os.path.dirname(expand(c["css"])), "diagrams.lua")]
    return [p for p in items if os.path.exists(p)]


def sourcepos_filter_path(c: dict) -> str:
    path = os.path.join(c["cacheDir"], "sourcepos-fix.lua")
    try:
        with open(path, encoding="utf-8") as f:
            current = f.read()
    except OSError:
        current = None
    if current != SOURCEPOS_FILTER:
        os.makedirs(c["cacheDir"], exist_ok=True)
        with open(path, "w", encoding="utf-8") as f:
            f.write(SOURCEPOS_FILTER)
    return path


def pandoc_args(note: str, css: str, html: str, c: dict, sourcepos: bool = False) -> list:
    directory = os.path.dirname(note)
    stem = os.path.splitext(os.path.basename(note))[0]
    filters = ([sourcepos_filter_path(c)] if sourcepos else []) + filter_paths(c)
    return (["-s", "-f", "gfm+sourcepos" if sourcepos else "gfm", "-t", "html5",
             "--syntax-highlighting=%s" % (c["highlight"] or "tango"),
             "-V", "lang=en", "--metadata", "pagetitle=%s" % stem,
             "--resource-path=%s" % directory,
             "--include-in-header=%s" % css,
             "--metadata=ws-header=%s" % css]
            + ["--lua-filter=%s" % f for f in filters]
            + ["-o", html, note])


def engine_args(note: str, html: str, out: str) -> list:
    directory = os.path.dirname(os.path.abspath(note))
    base = Path(directory).as_uri() + "/"
    return ["--pdf-tags", "-u", base, html, out]


def header_file(c: dict) -> str:
    own = expand(c["css"])
    own_exists = bool(c["css"]) and os.path.isfile(own)
    if own_exists and not c["themeCSS"]:
        return own
    text = None
    if own_exists:
        try:
            with open(own, encoding="utf-8") as f:
                text = f.read()
        except OSError:
            text = None
    if text is None:
        text = BUILTIN_CSS
    if c["themeCSS"]:
        text += "\n" + c["themeCSS"]
    path = _scratch(c, "header")
    with open(path, "w", encoding="utf-8") as f:
        f.write(text)
    return path


def css_content(c: dict) -> str:
    text = None
    if c["css"]:
        try:
            with open(expand(c["css"]), encoding="utf-8") as f:
                text = f.read()
        except OSError:
            text = None
    if text is None:
        text = BUILTIN_CSS
    if c["themeCSS"]:
        text += "\n" + c["themeCSS"]
    return text


def screen_html(note: str, raw) -> str | None:
    c = config(raw)
    pandoc = expand(c["pandoc"])
    if not os.access(pandoc, os.X_OK) or not os.path.isfile(note):
        return None
    css = html = None
    try:
        os.makedirs(c["cacheDir"], exist_ok=True)
        css = header_file(c)
        html = _scratch(c, "prose")
        p = subprocess.run([pandoc] + pandoc_args(note, css, html, c, sourcepos=True),
                           capture_output=True, text=True)
        if p.returncode != 0:
            return None
        with open(html, encoding="utf-8") as f:
            return f.read()
    except OSError:
        return None
    finally:
        _remove_scratch(css, c)
        _remove_scratch(html, c)


def export(note: str, raw) -> dict:
    c = config(raw)
    if not os.access(expand(c["pandoc"]), os.X_OK):
        return {"ok": False, "error": "pandoc not found — brew install pandoc"}
    if not os.access(expand(c["engine"]), os.X_OK):
        return {"ok": False, "error": "weasyprint not found — brew install weasyprint"}
    if not os.path.isfile(note):
        return {"ok": False, "error": "note not found: %s" % note}
    out = output_path(note, c)
    css = html = None
    try:
        os.makedirs(c["cacheDir"], exist_ok=True)
        os.makedirs(os.path.dirname(out), exist_ok=True)
        css = header_file(c)
        html = _scratch(c, "pdf")
        p = subprocess.run([expand(c["pandoc"])] + pandoc_args(note, css, html, c),
                           capture_output=True, text=True)
        if p.returncode != 0:
            return {"ok": False, "error": "pandoc failed: %s" % first_line(p.stderr)}
        w = subprocess.run([expand(c["engine"])] + engine_args(note, html, out),
                           capture_output=True, text=True)
        if w.returncode != 0 or not os.path.isfile(out):
            return {"ok": False, "error": "weasyprint failed: %s" % first_line(w.stderr)}
        return {"ok": True, "out": out}
    except OSError as e:
        return {"ok": False, "error": str(e)}
    finally:
        _remove_scratch(css, c)
        _remove_scratch(html, c)


def first_line(text: str) -> str:
    for line in (text or "").split("\n"):
        if line.strip():
            return line
    return "exit status"


def _scratch(c: dict, prefix: str) -> str:
    return os.path.join(c["cacheDir"], "%s-%s.html" % (prefix, str(uuid.uuid4()).upper()))


def _remove_scratch(path, c: dict) -> None:
    if not path or not path.startswith(c["cacheDir"]):
        return
    try:
        os.remove(path)
    except OSError:
        pass
