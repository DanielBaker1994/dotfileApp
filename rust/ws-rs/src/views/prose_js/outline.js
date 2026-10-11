(function () {
  var root = document.documentElement;
  var st = document.createElement('style');
  st.textContent = [
    '#ws-ol-btn{position:fixed;top:10px;right:14px;z-index:50;width:30px;height:30px;border-radius:8px;border:none;',
    'background:var(--p-s0,#333);color:var(--p-dim,#999);font:16px/30px -apple-system,sans-serif;cursor:pointer;opacity:.75}',
    '#ws-ol-btn:hover{opacity:1;color:var(--p-text,#eee)}',
    '#ws-ol{position:fixed;top:0;right:0;bottom:0;width:260px;z-index:49;overflow:auto;padding:52px 8px 40px;box-sizing:border-box;',
    'background:var(--p-well,#222);border-left:1px solid var(--p-rule,#444);display:none;font:13px -apple-system,sans-serif}',
    '#ws-ol.open{display:block}',
    '#ws-ol a{display:block;padding:5px 8px;border-radius:6px;color:var(--p-dim,#999);text-decoration:none;cursor:pointer;',
    'white-space:nowrap;overflow:hidden;text-overflow:ellipsis}',
    '#ws-ol a:hover{background:var(--p-s0,#333);color:var(--p-text,#eee)}',
    '#ws-ol a.cur{color:var(--p-accent,#8cf);background:var(--p-s0,#333)}',
    '#ws-ol .none{padding:8px;color:var(--p-dim,#999)}',
    '#ws-stats{position:fixed;left:0;right:0;bottom:0;z-index:48;padding:4px 14px;text-align:center;pointer-events:none;',
    'font:11px -apple-system,sans-serif;color:var(--p-dim,#999);background:linear-gradient(transparent,var(--p-bg,#111) 70%)}'
  ].join('');
  root.appendChild(st);
  var btn = document.createElement('button');
  btn.id = 'ws-ol-btn'; btn.type = 'button'; btn.title = 'Outline'; btn.textContent = '\u2261';
  var ol = document.createElement('div'); ol.id = 'ws-ol';
  var stats = document.createElement('div'); stats.id = 'ws-stats';
  root.appendChild(ol); root.appendChild(btn); root.appendChild(stats);
  var heads = [];
  function line(e) { var l = parseInt(e.getAttribute('data-pos')); return isNaN(l) ? null : l; }
  function build() {
    heads = Array.prototype.slice.call(document.body.querySelectorAll('h1,h2,h3,h4,h5,h6'));
    ol.innerHTML = '';
    if (!heads.length) { var n = document.createElement('div'); n.className = 'none'; n.textContent = 'No headings'; ol.appendChild(n); }
    var top = Math.min.apply(null, heads.map(function (h) { return +h.tagName[1]; }).concat([6]));
    heads.forEach(function (h) {
      var a = document.createElement('a');
      a.textContent = h.textContent;
      a.title = h.textContent;
      a.style.paddingLeft = (8 + (+h.tagName[1] - top) * 14) + 'px';
      a.addEventListener('click', function () {
        window.scrollTo({ top: h.getBoundingClientRect().top + window.scrollY - 12 });
        var l = line(h);
        if (l !== null) window.webkit.messageHandlers.wsJump.postMessage(l);
      });
      ol.appendChild(a);
    });
    var text = document.body.innerText || '';
    var words = (text.match(/\S+/g) || []).length;
    var mins = Math.max(1, Math.round(words / 230));
    stats.textContent = words + ' words \u00b7 ' + mins + ' min read';
    mark();
  }
  function mark() {
    var cur = -1;
    for (var i = 0; i < heads.length; i++) { if (heads[i].getBoundingClientRect().top <= 40) cur = i; else break; }
    Array.prototype.forEach.call(ol.querySelectorAll('a'), function (a, i) { a.classList.toggle('cur', i === cur); });
  }
  btn.addEventListener('click', function () { ol.classList.toggle('open'); });
  window.addEventListener('scroll', mark, { passive: true });
  window.__wsToggleOutline = function () { ol.classList.toggle('open'); };
  window.__wsBuildOutline = build;
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', build); else build();
})();
