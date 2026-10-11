(function () {
  var COPY = '<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="13" height="13" rx="2" ry="2"></rect><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"></path></svg>';
  var CHECK = '<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"></polyline></svg>';
  function addCopyButtons() {
    if (!document.getElementById('ws-copy')) return;
    document.querySelectorAll('pre').forEach(function (pre) {
      if (pre.parentElement && pre.parentElement.classList.contains('codeblock')) return;
      var code = pre.querySelector('code') || pre;
      var btn = document.createElement('button');
      btn.className = 'copy-btn';
      btn.type = 'button';
      btn.innerHTML = COPY;
      btn.setAttribute('aria-label', 'Copy code to clipboard');
      btn.addEventListener('click', function () {
        var text = code.innerText;
        function done(ok) {
          btn.classList.toggle('copied', ok);
          btn.innerHTML = ok ? CHECK : COPY;
          setTimeout(function () { btn.innerHTML = COPY; btn.classList.remove('copied'); }, 1500);
        }
        function fallback() {
          var ta = document.createElement('textarea');
          ta.value = text; ta.style.position = 'fixed'; ta.style.opacity = '0';
          document.body.appendChild(ta); ta.select();
          var ok = false;
          try { ok = document.execCommand('copy'); } catch (e) {}
          document.body.removeChild(ta); done(ok);
        }
        if (navigator.clipboard && navigator.clipboard.writeText) {
          navigator.clipboard.writeText(text).then(function () { done(true); }, fallback);
        } else { fallback(); }
      });
      var wrap = document.createElement('div');
      wrap.className = 'codeblock';
      pre.parentNode.insertBefore(wrap, pre);
      wrap.appendChild(btn);
      wrap.appendChild(pre);
    });
  }
  window.__wsAddCopyButtons = addCopyButtons;
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', addCopyButtons);
  else addCopyButtons();
})();
