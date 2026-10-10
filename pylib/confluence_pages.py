"""Confluence preview: URL absolutisation, the wsconf image wrapping, the
preview page template and the in-page highlight script.

Colors arrive as ready-made rgba() strings from the app's theme; the custom
scheme handler that serves wsconf:// stays in Swift."""
from __future__ import annotations

import base64
import json
import os
import re
from string import Template
from urllib.parse import urlsplit

SCHEME = "wsconf"


class _AT(Template):
    # the page's JS has ${...}-free but $-heavy regex literals; @ keeps the
    # substitution grammar out of the way
    delimiter = "@"


_PAGE = _AT(r"""<!doctype html><html><head><meta charset="utf-8">
<style>
:root { color-scheme: @light; }
html, body { background: transparent; }
body { font: 14px/1.6 -apple-system, "SF Pro Text", sans-serif; color: @text;
       margin: 0; padding: 14px 22px 80px; overflow-wrap: anywhere; }
h1, h2, h3, h4 { line-height: 1.3; margin: 1.2em 0 .4em; }
h1 { font-size: 1.5em; } h2 { font-size: 1.25em; } h3 { font-size: 1.1em; }
a { color: @accent; }
p, li { color: @text92; }
.dim { color: @dim; }
code { font: 12.5px ui-monospace, "SF Mono", monospace; background: @mantle; padding: 1px 4px; border-radius: 4px; }
pre { font: 12.5px/1.45 ui-monospace, "SF Mono", monospace; background: @mantle;
      padding: 10px 12px; border-radius: 6px; overflow: auto; border: 1px solid @hairline; }
table { border-collapse: collapse; margin: .6em 0; }
td, th { border: 1px solid @hairline; padding: 5px 9px; vertical-align: top; }
th { background: @mantle; text-align: left; }
img { max-width: 100%; height: auto; border-radius: 4px; }
blockquote { border-left: 3px solid @hairline; margin: .6em 0; padding: 0 12px; color: @dim; }
.confluence-information-macro, .panel, .aui-message { border-left: 3px solid @accent;
      background: @mantle; padding: 2px 12px; margin: .8em 0; border-radius: 4px; }
.confluence-information-macro-warning { border-color: @warn; }
.confluence-information-macro-note { border-color: @info; }
mark.wsh { background: @warn35; color: inherit; border-radius: 2px; padding: 0 1px; }
mark.wsh.on { background: @warn75; color: #000;
              box-shadow: 0 0 0 2px @accent; }
.ws-title { font-size: 1.6em; font-weight: 650; margin: .2em 0 .6em; }
</style></head><body><div class="ws-title">@title</div>@body
<script>
(function () {
  const terms = @terms;
  const esc = s => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const suf = '(?:s|es|ed|d|ing|ment|ments|er|ers|ly)?';
  const parts = terms.map(t => {
    const w = (t.text || '').trim().split(/\s+/).filter(Boolean).map(x => esc(x) + (t.prefix ? '' : suf));
    if (!w.length) return null;
    return '\\b' + w.join('\\s+') + (t.prefix ? '\\w*' : '\\b');
  }).filter(Boolean);
  let marks = [], cur = -1;
  if (parts.length) {
    const rx = new RegExp(parts.join('|'), 'gi');
    const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT, {
      acceptNode: n => (n.parentNode && /^(SCRIPT|STYLE|MARK)$/.test(n.parentNode.nodeName))
        ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT });
    const nodes = []; while (walker.nextNode()) nodes.push(walker.currentNode);
    for (const n of nodes) {
      const s = n.nodeValue; rx.lastIndex = 0; let m, last = 0, frag = null;
      while ((m = rx.exec(s)) && m[0].length) {
        frag = frag || document.createDocumentFragment();
        frag.appendChild(document.createTextNode(s.slice(last, m.index)));
        const mk = document.createElement('mark'); mk.className = 'wsh'; mk.textContent = m[0];
        frag.appendChild(mk); last = m.index + m[0].length;
        // the title is marked but not a stop: hits walk the body
        if (!(n.parentNode.closest && n.parentNode.closest('.ws-title'))) marks.push(mk);
      }
      if (frag) { frag.appendChild(document.createTextNode(s.slice(last))); n.parentNode.replaceChild(frag, n); }
    }
  }
  function snippet(mk) {
    let b = mk.parentElement; while (b && getComputedStyle(b).display === 'inline') b = b.parentElement;
    const t = (b ? b.innerText : mk.textContent).replace(/\s+/g, ' ');
    const at = t.toLowerCase().indexOf(mk.textContent.toLowerCase());
    const a = Math.max(0, at - 70), z = Math.min(t.length, at + mk.textContent.length + 90);
    return (a > 0 ? '…' : '') + t.slice(a, z) + (z < t.length ? '…' : '');
  }
  function go(i) {
    if (!marks.length) { post(-1); return; }
    if (cur >= 0) marks[cur].classList.remove('on');
    cur = (i + marks.length) % marks.length;
    marks[cur].classList.add('on');
    marks[cur].scrollIntoView({ block: 'center', behavior: 'smooth' });
    post(cur);
  }
  function post(i) {
    window.webkit.messageHandlers.ws.postMessage({ i: i, n: marks.length, snippet: i >= 0 ? snippet(marks[i]) : '' });
  }
  window.wsNext = () => go(cur + 1);
  window.wsPrev = () => go(cur - 1);
  if (marks.length) go(0); else post(-1);
})();
</script></body></html>
""")

_DATA_FILE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "confluence", "defaults.json")
# the preview page's color-key schema lives in confluence/defaults.json
with open(_DATA_FILE, encoding="utf-8") as _fh:
    _CONFLUENCE_DEFAULTS = json.load(_fh)

_COLOR_KEYS = list(_CONFLUENCE_DEFAULTS["pages"]["color_keys"])

_IMG = re.compile(r'(<img\b[^>]*?\bsrc\s*=\s*)"([^"]+)"', re.IGNORECASE)


def _s(x) -> str:
    return x if isinstance(x, str) else ""


def wrap_image(real: str) -> str:
    b = base64.b64encode(real.encode("utf-8")).decode("ascii")
    return "%s://fetch/%s" % (SCHEME, b.replace("+", "-").replace("/", "_").rstrip("="))


def absolute(s: str, base: str) -> str:
    s = _s(s)
    if s.startswith(("http://", "https://", "data:")):
        return s
    try:
        parts = urlsplit(base)
        port = parts.port
    except ValueError:
        return s
    if not parts.scheme or not parts.hostname:
        return s
    origin = "%s://%s" % (parts.scheme, parts.hostname) + (":%d" % port if port else "")
    if s.startswith("//"):
        return parts.scheme + ":" + s
    if s.startswith("/"):
        ctx = parts.path
        if ctx and ctx != "/" and not s.startswith(ctx + "/") and s.startswith("/download/"):
            return base + s
        return origin + s
    return base + "/" + s


def rewrite(html: str, base: str) -> str:
    """All <img src=...>: absolutised, and on-site images point at the app's
    custom scheme so the loader can attach the auth header. srcset is
    neutralised (the loader would not know how to fetch those)."""
    host = urlsplit(base).hostname
    if not host:
        return html

    def sub(m):
        src = absolute(m.group(2).replace("&amp;", "&"), base)
        on_site = urlsplit(src).hostname == host
        return m.group(1) + '"' + (wrap_image(src) if on_site else src) + '"'

    return _IMG.sub(sub, html).replace("srcset=", "data-srcset=")


def _esc_title(s: str) -> str:
    return _s(s).replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


def preview_html(params: dict) -> str:
    page = params.get("page") if isinstance(params.get("page"), dict) else {}
    row = params.get("row") if isinstance(params.get("row"), dict) else {}
    base = _s(page.get("site")) or _s(params.get("site"))
    body = _s(page.get("html"))
    typ = _s(page.get("type")) or _s(row.get("type"))
    if typ == "attachment":
        mt = _s(page.get("mediaType"))
        u = absolute(_s(page.get("url")) or _s(row.get("url")), base)
        body = ('<p><img src="%s"></p>' % u) if mt.startswith("image/") \
            else '<p class=dim>Attachment (%s) — open it in the browser.</p>' % (mt or "file")
    if not body:
        body = "<p class=dim>(this page has no body)</p>"
    title = _s(page.get("title")) or _s(row.get("title"))
    colors = params.get("colors") if isinstance(params.get("colors"), dict) else {}
    terms = params.get("terms") if isinstance(params.get("terms"), list) else []
    subs = {k: _s(colors.get(k)) for k in _COLOR_KEYS}
    subs["title"] = _esc_title(title)
    subs["body"] = rewrite(body, base)
    subs["terms"] = json.dumps(terms, ensure_ascii=False, separators=(",", ":"))
    return _PAGE.safe_substitute(subs)
