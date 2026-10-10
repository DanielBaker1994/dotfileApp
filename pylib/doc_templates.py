from __future__ import annotations

import jsonmgr
import re

# lazy (worker surface): a broken doc_templates.json fails at the first
# method call and is retried on the next one
jsonmgr.lazy_module(__name__, globals(), {
    "BUILTIN": ("pylib/doc_templates", ("builtin",), list),
})

_PALETTE = re.compile(r":root:has\(\.([A-Za-z0-9_-]+)\)\s*\{[^}]*--p-bg")
_CLASS = re.compile(r'class="([^"]*)"')
_FOOT = re.compile(r'data-foot="([^"]*)"')


def from_css(css: str) -> list:
    out, seen = [], set()
    for match in _PALETTE.finditer(css):
        name = match.group(1)
        if name != "doc" and name not in seen:
            seen.add(name)
            out.append(name)
    return out


def names(configured, css=None) -> list:
    configured = [w.strip() for w in (configured or "").split(",")]
    configured = [w for w in configured if w]
    if configured:
        return configured
    found = from_css(css) if css else []
    return found or list(jsonmgr.field("pylib/doc_templates", "builtin"))


def parse(line: str):
    text = line.strip()
    if not text.startswith("<div") or not text.endswith("</div>"):
        return None
    class_match = _CLASS.search(text)
    if not class_match:
        return None
    words = class_match.group(1).split(" ")
    if not words or words[0] != "doc":
        return None
    foot_match = _FOOT.search(text)
    return {"template": words[1] if len(words) > 1 else "",
            "foot": foot_match.group(1) if foot_match else ""}


def current(text: str):
    first = text.split("\n", 1)[0] if text else ""
    parsed = parse(first)
    return parsed["template"] if parsed else None


def marker(template: str, foot: str) -> str:
    return '<div class="doc %s"%s></div>' % (template, ' data-foot="%s"' % foot if foot else "")


def edit(text: str, template) -> dict:
    lines = text.split("\n")
    first = lines[0] if lines else ""
    old = parse(first)
    blank_after = len(lines) > 1 and lines[1].strip() == ""
    if old is None and template is None:
        return {"remove": 0, "insert": []}
    if old is None:
        return {"remove": 0, "insert": [marker(template, ""), ""]}
    if template is None:
        return {"remove": 2 if blank_after else 1, "insert": []}
    return {"remove": 1, "insert": [marker(template, old["foot"])]}


def apply(text: str, template) -> str:
    change = edit(text, template)
    lines = text.split("\n")
    lines[0:min(change["remove"], len(lines))] = change["insert"]
    return "\n".join(lines)
