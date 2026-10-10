"""Jira page templates: the ticket detail page and its comment fragment.

The app hosts the HTML in a WKWebView; every decision about what the page
shows (stepper, pills, props, the all-fields table, comments) lives here.
Colors arrive as hex strings from the app's theme (the theme algebra still
lives in Swift for now); words/categories arrive as the resolved data the
app already caches.
"""
from __future__ import annotations

import datetime
from string import Template

import jira_data
import jsonmgr

# the three status-category labels live in jira/defaults.json (one home; the
# ticket stepper + the board/directory pickers all read them)
_JIRA_DEFAULTS = jsonmgr.load("jira/defaults")

CATEGORY_NAMES = list(_JIRA_DEFAULTS["category_names"])

_PAGE = Template("""<!doctype html><html><head><meta charset="utf-8"><style>
:root{--bg:$bg;--card:$card;--s0:$s0;--s1:$s1;--tx:$tx;--dim:$dim;
  --acc:$acc;--on:$on;--ok:$ok;--warn:$warn;--line:$s1}
*{box-sizing:border-box}
html,body{margin:0;background:var(--bg);color:var(--tx);font:13.5px/1.55 -apple-system,BlinkMacSystemFont,system-ui,sans-serif;-webkit-user-select:text}
.card{background:var(--card);border-bottom:1px solid var(--line);padding:16px 22px 14px;display:flex;flex-direction:column;gap:10px}
.top{display:flex;align-items:center;gap:8px}
.key{font:12px ui-monospace,Menlo,monospace;color:var(--dim);letter-spacing:.02em}
.grow{flex:1}
.btn{font:500 12px -apple-system,system-ui;color:var(--tx);background:transparent;border:1px solid var(--line);border-radius:6px;padding:4px 11px;cursor:pointer}
.btn:hover{background:var(--s0)}
.btn.pri{background:var(--acc);color:var(--on);border-color:transparent;font-weight:600}
h1{font-size:19px;line-height:1.3;font-weight:600;margin:0;text-wrap:balance}
.steps{display:flex;flex-wrap:wrap;align-items:center;gap:5px;font-size:11.5px}
.step{padding:3px 10px;border-radius:999px;background:var(--s0);color:var(--dim)}
.step.done{color:var(--ok)}
.step.now{background:var(--acc);color:var(--on);font-weight:600}
.sep{color:var(--dim);opacity:.6}
.step .sub{font-weight:400;opacity:.85}
.pills{display:flex;flex-wrap:wrap;gap:6px}
.pill{font-size:11px;padding:2px 9px;border-radius:999px;background:var(--s0);color:var(--tx)}
.pill.warn{color:var(--warn)} .pill.dim{color:var(--dim)}
.tabs{display:flex;gap:2px;padding:0 18px;border-bottom:1px solid var(--line);position:sticky;top:0;background:var(--bg)}
.tab{padding:9px 12px 8px;font-size:12.5px;color:var(--dim);cursor:pointer;border-bottom:2px solid transparent;user-select:none}
.tab.on{color:var(--tx);border-color:var(--acc)}
.pane{display:none;padding:16px 22px 40px}
.pane.on{display:block}
.split{display:grid;grid-template-columns:minmax(0,1fr) 230px;gap:28px}
@media (max-width:700px){.split{grid-template-columns:1fr}}
.desc p{margin:0 0 .8em;max-width:75ch}
dl{display:grid;grid-template-columns:auto 1fr;gap:7px 14px;margin:0;font-size:12.5px;align-content:start}
dt{color:var(--dim)} dd{margin:0;overflow-wrap:anywhere}
.h{font-size:10.5px;letter-spacing:.08em;text-transform:uppercase;color:var(--dim);margin:0 0 8px}
.empty{color:var(--dim)}
.cmt{display:flex;gap:10px;margin-bottom:16px}
.av{width:26px;height:26px;border-radius:50%;background:var(--s1);color:var(--tx);display:grid;place-items:center;font-size:10px;font-weight:700;flex:none}
.cb{min-width:0}.ch{font-size:12.5px;margin-bottom:3px}.ct{max-width:75ch}
.dim{color:var(--dim)}
</style></head><body>
<div class="card">
  <div class="top"><span class="key">$key</span><span class="grow"></span>
    <button class="btn" data-a="copy-key">Copy key</button>$link_btn$open_btn</div>
  <h1>$title</h1>
  $stepper_block
  $pills_block
</div>
<div class="tabs"><span class="tab on" data-t="d">Details</span><span class="tab" data-t="c" id="ctab">$comments_label</span><span class="tab" data-t="f">All fields</span></div>
<div class="pane on" id="d"><div class="split"><div><p class="h">Description</p><div class="desc">$desc</div></div><dl>$props</dl></div></div>
<div class="pane" id="c">$cm</div>
<div class="pane" id="f"><dl>$all</dl></div>
<script>
document.querySelectorAll('[data-a]').forEach(b=>b.addEventListener('click',()=>window.webkit.messageHandlers.ws.postMessage(b.dataset.a)));
document.querySelectorAll('.tab').forEach(t=>t.addEventListener('click',()=>{
  document.querySelectorAll('.tab').forEach(x=>x.classList.toggle('on',x===t));
  document.querySelectorAll('.pane').forEach(p=>p.classList.toggle('on',p.id===t.dataset.t));
}));
document.addEventListener('click',e=>{const a=e.target.closest('a[href]');if(a){e.preventDefault();window.webkit.messageHandlers.ws.postMessage('url:'+a.href)}});
</script></body></html>
""")

_HEX_KEYS = list(_JIRA_DEFAULTS["pages"]["hex_keys"])


def esc(s: str) -> str:
    return (s.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")
            .replace('"', "&quot;"))


def _s(x) -> str:
    return x if isinstance(x, str) else ""


def _v(fields: dict, key: str) -> str:
    return _s(fields.get(key)).strip()


def date(s: str, now: datetime.datetime = None) -> str:
    """The app's ticket timestamp style: 'Mar 4, 09:03' in the current year,
    'Mar 4, 2020' otherwise; unparseable input passes through."""
    if not s:
        return s
    now = now or datetime.datetime.now().astimezone()
    for fmt in ("%Y-%m-%dT%H:%M:%S.%f%z", "%Y-%m-%dT%H:%M:%S%z", "%Y-%m-%d"):
        try:
            d = datetime.datetime.strptime(s, fmt)
        except ValueError:
            continue
        if d.tzinfo is None:
            local = d
        else:
            local = d.astimezone()
        if local.year == now.year:
            return "%s %d, %02d:%02d" % (local.strftime("%b"), local.day, local.hour, local.minute)
        return "%s %d, %d" % (local.strftime("%b"), local.day, local.year)
    return s


def _stepper(fields: dict, params: dict) -> str:
    status = _v(fields, "status")
    steps = jira_data.workflow_steps(params.get("workflow"))
    if steps and status in steps:
        cur = steps.index(status)
        chunks = []
        for i, s in enumerate(steps):
            cls = "done" if i < cur else "now" if i == cur else ""
            chunks.append('<span class="step %s">%s%s</span>'
                          % (cls, "✓ " if i < cur else "", esc(s)))
        return '<span class="sep">›</span>'.join(chunks)
    if not status:
        return ""
    cur = jira_data.category(status, params.get("categories") or {}, params.get("words") or {})
    chunks = []
    for i, s in enumerate(CATEGORY_NAMES):
        cls = "done" if i < cur else "now" if i == cur else ""
        if i == cur and status.lower() != s.lower():
            name = '%s <span class="sub">· %s</span>' % (esc(s), esc(status))
        else:
            name = esc(s)
        chunks.append('<span class="step %s">%s%s</span>'
                      % (cls, "✓ " if i < cur else "", name))
    return '<span class="sep">›</span>'.join(chunks)


def _pills(fields: dict) -> tuple:
    pills = []
    if _v(fields, "priority"):
        pills.append('<span class="pill warn">%s</span>' % esc(_v(fields, "priority")))
    rel = _v(fields, "releaseLabel") or _v(fields, "release")
    if rel:
        pills.append('<span class="pill">%s</span>' % esc(rel))
    labs = [p.strip() for p in _v(fields, "labels").split(",") if p.strip()]
    for l in labs[:6]:
        pills.append('<span class="pill dim">%s</span>' % esc(l))
    if len(labs) > 6:
        rest = labs[6:]
        pills.append('<span class="pill dim" title="%s">+%d</span>'
                     % (esc(", ".join(rest)), len(rest)))
    return pills, rel


def comments_html(comments) -> str:
    """The comments list fragment (newest first), also injected into a
    loading page after the app answers with the rows."""
    if not comments:
        return '<p class="empty">No comments.</p>'
    out = []
    for cm in reversed(comments):
        if not isinstance(cm, dict):
            continue
        author = _s(cm.get("author"))
        initials = "".join(w[0] for w in [x for x in author.split(" ") if x][:2])
        body = esc(_s(cm.get("body"))).replace("\n", "<br>")
        out.append('<div class="cmt"><span class="av">%s</span><div class="cb">\n'
                   '<div class="ch"><b>%s</b> <span class="dim">· %s</span></div>\n'
                   '<div class="ct">%s</div></div></div>\n'
                   % (esc(initials), esc(author), esc(date(_s(cm.get("created")))), body))
    return "".join(out)


def ticket_html(params: dict) -> str:
    fields = params.get("fields") or {}
    key = _v(fields, "key")
    title = _v(fields, "title") or _v(fields, "summary") or _s(params.get("title"))
    stepper = _stepper(fields, params)
    pills, rel = _pills(fields)

    props = [
        ("Assignee", _v(fields, "assignee") or "Unassigned"),
        ("Reporter", _v(fields, "reporter")),
        ("Project", _v(fields, "project")),
        ("Release", rel),
        ("Priority", _v(fields, "priority")),
        ("Created", date(_v(fields, "created"))),
        ("Updated", date(_v(fields, "updated"))),
    ]
    props_html = "".join("<dt>%s</dt><dd>%s</dd>" % (esc(k), esc(val))
                         for k, val in props if val)

    desc = _v(fields, "description")
    if desc:
        desc_html = "".join("<p>%s</p>" % esc(p).replace("\n", "<br>")
                            for p in desc.split("\n\n"))
    else:
        desc_html = '<p class="empty">No description.</p>'

    labels = params.get("labels") or {}
    skip = {"comments", "description"}
    all_html = "".join(
        "<dt>%s</dt><dd>%s</dd>" % (esc(labels.get(k) or k), esc(val))
        for k, val in sorted(fields.items())
        if not k.startswith("__") and k not in skip and val)

    comments = params.get("comments")
    cm_html = comments_html(comments) if comments is not None \
        else '<p class="empty">Loading comments…</p>'
    comments_label = "Comments %d" % len(comments) if comments is not None else "Comments"

    url = params.get("url")
    open_btn = ('<button class="btn pri" data-a="open" title="Open in browser">Open ↗</button>'
                if url else "")
    link_btn = '<button class="btn" data-a="copy-link">Copy link</button>' if url else ""

    colors = params.get("colors") or {}
    return _PAGE.safe_substitute(
        {k: _s(colors.get(k)) for k in _HEX_KEYS},
        key=esc(key), title=esc(title),
        stepper_block='<div class="steps">%s</div>' % stepper if stepper else "",
        pills_block='<div class="pills">%s</div>' % "".join(pills) if pills else "",
        comments_label=comments_label,
        desc=desc_html, props=props_html, cm=cm_html, all=all_html,
        link_btn=link_btn, open_btn=open_btn)
