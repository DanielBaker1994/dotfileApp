(function () {
  var root = document.documentElement;
  var st = document.createElement('style');
  st.textContent = [
    '::highlight(ws-find){background-color:var(--p-warning-tint,rgba(255,200,0,.3));color:inherit}',
    '::highlight(ws-find-cur){background-color:var(--p-accent,#fc0);color:var(--p-bg,#000)}',
    '#ws-find{position:fixed;left:14px;bottom:26px;z-index:51;display:none;align-items:center;gap:6px;padding:4px 10px;',
    'border-radius:8px;background:var(--p-well,#222);border:1px solid var(--p-rule,#444);font:13px ui-monospace,Menlo,monospace;color:var(--p-text,#eee)}',
    '#ws-find.open{display:flex}',
    '#ws-find input{width:220px;border:none;outline:none;background:transparent;color:inherit;font:inherit}',
    '#ws-find .cnt{color:var(--p-dim,#999);font-size:11px;min-width:44px;text-align:right}',
    '#ws-find .sl{color:var(--p-accent,#8cf)}'
  ].join('');
  root.appendChild(st);
  var bar = document.createElement('div'); bar.id = 'ws-find';
  bar.innerHTML = '<span class="sl">/</span><input type="text" spellcheck="false" autocomplete="off" autocapitalize="off"><span class="cnt"></span>';
  root.appendChild(bar);
  var input = bar.querySelector('input'), cnt = bar.querySelector('.cnt');
  var ranges = [], cur = -1, state = 'idle';
  function post(s) { state = s; try { window.webkit.messageHandlers.wsSearch.postMessage(s); } catch (e) {} }
  function clearHl() {
    if (window.CSS && CSS.highlights) { CSS.highlights.delete('ws-find'); CSS.highlights.delete('ws-find-cur'); }
  }
  function regex(q) {
    try { return new RegExp(q, 'gi'); }
    catch (e) { return new RegExp(q.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'), 'gi'); }
  }
  function scan() {
    ranges = []; cur = -1; clearHl();
    var q = input.value;
    if (!q) { cnt.textContent = ''; return; }
    var re = regex(q);
    var w = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT, {
      acceptNode: function (n) {
        var p = n.parentElement;
        return p && !/^(SCRIPT|STYLE)$/.test(p.tagName) ? NodeFilter.FILTER_ACCEPT : NodeFilter.FILTER_REJECT;
      }
    });
    var n;
    while ((n = w.nextNode())) {
      var t = n.nodeValue, m;
      re.lastIndex = 0;
      while ((m = re.exec(t)) !== null) {
        if (m[0].length === 0) { re.lastIndex++; continue; }
        var r = document.createRange();
        r.setStart(n, m.index); r.setEnd(n, m.index + m[0].length);
        ranges.push(r);
      }
    }
    if (window.CSS && CSS.highlights && ranges.length) CSS.highlights.set('ws-find', new Highlight(...ranges));
    cnt.textContent = ranges.length ? '' : 'no match';
  }
  function show(i, scroll) {
    if (!ranges.length) return;
    cur = (i + ranges.length) % ranges.length;
    if (window.CSS && CSS.highlights) CSS.highlights.set('ws-find-cur', new Highlight(ranges[cur]));
    cnt.textContent = (cur + 1) + '/' + ranges.length;
    if (scroll) {
      var rc = ranges[cur].getBoundingClientRect();
      window.scrollTo({ top: window.scrollY + rc.top - window.innerHeight / 2 });
    }
  }
  function firstFromView() {
    for (var i = 0; i < ranges.length; i++) { if (ranges[i].getBoundingClientRect().top >= 0) return i; }
    return 0;
  }
  function close() {
    input.value = ''; ranges = []; cur = -1; clearHl();
    cnt.textContent = ''; bar.classList.remove('open'); input.blur(); post('idle');
  }
  input.addEventListener('input', function () { scan(); show(firstFromView(), true); });
  input.addEventListener('keydown', function (e) {
    if (e.key === 'Enter') {
      e.preventDefault();
      if (!input.value) { close(); return; }
      if (cur < 0) { scan(); show(firstFromView(), true); }
      input.blur(); post(ranges.length ? 'active' : 'idle');
      if (!ranges.length) close();
    } else if (e.key === 'Escape') { e.preventDefault(); close(); }
  });
  window.__wsFindOpen = function () {
    bar.classList.add('open'); input.focus(); input.select(); post('typing');
  };
  window.__wsFindStep = function (back) { if (ranges.length) show(cur + (back ? -1 : 1), true); };
  window.__wsFindEsc = close;
  window.__wsFindRefresh = function () {
    if (!input.value) return;
    var keep = cur; scan(); if (ranges.length) show(Math.min(keep < 0 ? 0 : keep, ranges.length - 1), false);
  };
})();
