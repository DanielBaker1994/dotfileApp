"""Jira board: the column/card mapping and the board page template.

The app hosts the page in a WKWebView; colors arrive as hex/rgba strings
(the theme algebra still lives in Swift for now).
"""
from __future__ import annotations

from string import Template

import jira_data
import jira_pages
import jsonmgr

# board caps/labels/color schema live in jira/defaults.json (one home)
_BOARD_DEFAULTS = jsonmgr.load("jira/defaults")["boards"]

# a done column only keeps this many cards (the Table view shows them all)
DONE_LIMIT = _BOARD_DEFAULTS["done_limit"]
OTHER_COLUMN = _BOARD_DEFAULTS["other_column"]
_HOT_PATTERN = "|".join(_BOARD_DEFAULTS["hot_words"])


class _AT(Template):
    # the page's JS is full of `${...}` template literals; @ keeps them out
    # of the substitution grammar
    delimiter = "@"


_PAGE = _AT(r"""<!doctype html><html><head><meta charset="utf-8"><style>
:root { --bg: @bg; --col: @col; --card: @card;
  --card-hover: @cardHover; --line: @line; --text: @text;
  --dim: @dim; --accent: @accent; --todo: @dim;
  --prog: @accent; --done: @done; --hot: @hot; }
* { box-sizing: border-box; }
html, body { margin: 0; height: 100%; background: transparent; color: var(--text);
  font: 12.5px/1.4 -apple-system, BlinkMacSystemFont, "SF Pro Text", sans-serif; }
#board { display: flex; gap: 10px; padding: 12px 14px 14px; height: 100%; overflow-x: auto; }
.col { flex: 1 0 210px; max-width: 360px; background: var(--col); border-radius: 10px; display: flex;
  flex-direction: column; min-height: 0; }
.col h4 { margin: 0; padding: 10px 12px 8px; font-weight: 600; font-size: 10.5px; line-height: 1.2;
  letter-spacing: .08em; text-transform: uppercase; color: var(--dim); display: flex; gap: 8px; }
.col h4 .n { margin-left: auto; font-variant-numeric: tabular-nums; }
.cards { padding: 0 8px 8px; display: flex; flex-direction: column; gap: 7px; overflow-y: auto; min-height: 0; }
.card { background: var(--card); border: 1px solid var(--line); border-radius: 8px; padding: 8px 10px;
  display: grid; gap: 6px; cursor: default; }
.card:hover { background: var(--card-hover); }
.card .t { color: var(--text); overflow-wrap: anywhere; }
.card .f { display: flex; align-items: center; gap: 8px; color: var(--dim); font-size: 11.5px; }
.key { font: 11.5px ui-monospace, "SF Mono", Menlo, monospace; color: var(--accent); }
.dot { width: 8px; height: 8px; border-radius: 2px; flex: none; border: 1.5px solid var(--todo); }
.c1 .dot { border-color: var(--prog); background: linear-gradient(90deg, var(--prog) 50%, transparent 50%); }
.c2 .dot { border-color: var(--done); background: var(--done); }
.c2 .t { color: var(--dim); }
.hot { color: var(--hot); }
.av { margin-left: auto; width: 20px; height: 20px; border-radius: 50%; background: var(--line);
  color: var(--text); font-size: 9.5px; font-weight: 600; display: grid; place-items: center; flex: none; }
.more, .empty { color: var(--dim); font-size: 11.5px; padding: 4px 4px 2px; }
.none { color: var(--dim); padding: 40px; text-align: center; width: 100%; }
</style></head><body><div id="board"></div><script>
function esc(s) { return String(s).replace(/[&<>"]/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;'}[c])); }
function initials(n) { const p = n.trim().split(/\s+/); return ((p[0]||'')[0]||'') + (p.length > 1 ? p[p.length-1][0] : ''); }
function render(cols) {
  const b = document.getElementById('board');
  if (!cols.length || cols.every(c => !c.cards.length)) { b.innerHTML = '<div class="none">No issues here</div>'; return; }
  b.innerHTML = cols.map(c => `<section class="col"><h4>${esc(c.name)}<span class="n">${c.cards.length + c.more}</span></h4>
    <div class="cards">${c.cards.map(k => `<div class="card c${k.cat}" data-key="${esc(k.key)}" title="${esc(k.status)}">
      <div class="f"><span class="dot"></span><span class="key">${esc(k.key)}</span><span>${esc(k.type)}</span></div>
      <div class="t">${esc(k.title)}</div>
      <div class="f"><span class="${/@hotPattern/i.test(k.priority) ? 'hot' : ''}">${esc(k.priority)}</span>
        ${k.assignee ? `<span class="av" title="${esc(k.assignee)}">${esc(initials(k.assignee).toUpperCase())}</span>` : ''}</div>
    </div>`).join('')}${c.more ? `<div class="more">+ ${c.more} more — Table shows them all</div>` : ''}
    ${!c.cards.length && !c.more ? '<div class="empty">Nothing here</div>' : ''}</div></section>`).join('');
}
document.addEventListener('click', e => {
  const c = e.target.closest('.card');
  if (c) window.webkit.messageHandlers.board.postMessage('open:' + c.dataset.key);
});
window.webkit.messageHandlers.board.postMessage('ready');
</script></body></html>
""")

_COLOR_KEYS = list(_BOARD_DEFAULTS["color_keys"])


def _s(x) -> str:
    return x if isinstance(x, str) else ""


def board_columns(params: dict) -> dict:
    """Rows -> the board's columns. With a board spec, statuses map to their
    column and the rest land in 'Not on the board'; without one, the three
    status categories are the columns. A fully-done column clamps to
    DONE_LIMIT cards."""
    rows = [r for r in (params.get("rows") or []) if isinstance(r, dict)]
    spec = [c for c in (params.get("columns") or []) if isinstance(c, dict)]
    cats = params.get("categories") or {}
    words = params.get("words") or {}
    people = params.get("people") or {}
    names = params.get("categoryNames") or list(jira_pages.CATEGORY_NAMES)

    if spec:
        cols = [{"name": _s(c.get("name")),
                 "statuses": [s for s in (c.get("statuses") or []) if isinstance(s, str)],
                 "cards": []} for c in spec]
    else:
        cols = [{"name": n, "statuses": [], "cards": []} for n in names]

    other = []
    for r in rows:
        def f(k):
            return _s(r.get(k))
        st = f("status")
        assignee = f("assignee")
        card = {"key": f("key"),
                "title": f("title") or _s(r.get("rowTitle")),
                "type": f("type"), "priority": f("priority"),
                "assignee": (people.get(assignee) or assignee) if assignee else "",
                "status": st,
                "cat": jira_data.category(st, cats, words)}
        if spec:
            for col in cols:
                if st in col["statuses"]:
                    col["cards"].append(card)
                    break
            else:
                other.append(card)
        else:
            cols[card["cat"]]["cards"].append(card)

    out = []
    for c in cols:
        done = bool(c["cards"]) and all(k["cat"] == 2 for k in c["cards"])
        keep = c["cards"][:DONE_LIMIT] if done else c["cards"]
        out.append({"name": c["name"], "cards": keep, "more": len(c["cards"]) - len(keep)})
    if other:
        out.append({"name": OTHER_COLUMN, "cards": other, "more": 0})
    return {"columns": out}


def board_page(colors: dict) -> str:
    subst = {k: _s((colors or {}).get(k)) for k in _COLOR_KEYS}
    subst["hotPattern"] = _HOT_PATTERN
    return _PAGE.safe_substitute(subst)
