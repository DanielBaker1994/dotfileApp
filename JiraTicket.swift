import AppKit
import WebKit

// The Jira ticket page (Return / double-click on an issue row): a header
// card — key, Copy key / Copy link / Open in browser, the summary, the
// workflow as a step bar (`[jira] workflow`, else a guessed order of the
// directory's statuses), the meta pills — then tabs: Details (description
// + properties), Comments (N), All fields. Drawn as HTML in the window's
// theme over the detail window's editor (PopupWindow.setPageOverlay).
// Buttons post `ws` messages: copy-key, copy-link, open, url:<href>.

enum JiraTicketPage {
    static func esc(_ s: String) -> String { ProseRender.esc(s) }
    static func css(_ c: NSColor) -> String { ProseRender.css(c) }

    // the workflow, in order: `[jira] workflow = "To Do, In Progress, …"`,
    // else the directory's statuses sorted by the usual lifecycle words
    static func workflow(current: String) -> [String] {
        if let w = configSectionValue("jira", "workflow"), !w.isEmpty {
            return w.split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
        }
        let dir = JiraPoll.readJSON(JiraPoll.directoryPath)?["statuses"] as? [String] ?? []
        var all = dir
        if !current.isEmpty, !all.contains(current) { all.append(current) }
        func rank(_ s: String) -> Int {
            let l = s.lowercased()
            if ["backlog", "open", "to do", "todo", "new", "selected"].contains(where: l.contains) { return 0 }
            if l.contains("progress") || l.contains("doing") || l.contains("develop") { return 1 }
            if l.contains("review") { return 2 }
            if l.contains("test") || l.contains("qa") || l.contains("verif") { return 3 }
            if l.contains("block") || l.contains("hold") { return 4 }
            if ["done", "closed", "resolved", "released", "complete"].contains(where: l.contains) { return 6 }
            return 5
        }
        // blocked / on hold are side states, not steps
        return all.filter { rank($0) != 4 }.sorted { (rank($0), $0) < (rank($1), $1) }
    }

    static func date(_ s: String) -> String {
        let iso = DateFormatter()
        iso.locale = Locale(identifier: "en_US_POSIX")
        for f in ["yyyy-MM-dd'T'HH:mm:ss.SSSZ", "yyyy-MM-dd'T'HH:mm:ssZ", "yyyy-MM-dd"] {
            iso.dateFormat = f
            if let d = iso.date(from: s) {
                let out = DateFormatter()
                out.dateFormat = Calendar.current.isDate(d, equalTo: Date(), toGranularity: .year) ? "MMM d, HH:mm" : "MMM d, yyyy"
                return out.string(from: d)
            }
        }
        return s
    }

    static func comments(_ raw: String?) -> [(author: String, body: String, created: String)] {
        guard let raw, let data = raw.data(using: .utf8),
              let arr = try? JSONSerialization.jsonObject(with: data) as? [[String: Any]] else { return [] }
        return arr.map { ($0["author"] as? String ?? "", $0["body"] as? String ?? "", $0["created"] as? String ?? "") }
    }

    static func html(_ row: FieldRow, colors c: PopupColors, url: String?, labels: [String: String]) -> String {
        let f = row.fields
        func v(_ k: String) -> String { (f[k] ?? "").trimmingCharacters(in: .whitespacesAndNewlines) }
        let key = v("key"), title = v("title").isEmpty ? (v("summary").isEmpty ? row.title : v("summary")) : v("title")
        let status = v("status")
        let steps = workflow(current: status)
        let cur = steps.firstIndex(of: status)
        var stepper = ""
        if let cur {
            stepper = steps.enumerated().map { i, s in
                let cls = i < cur ? "done" : i == cur ? "now" : ""
                return "<span class=\"step \(cls)\">\(i < cur ? "✓ " : "")\(esc(s))</span>"
            }.joined(separator: "<span class=\"sep\">›</span>")
        } else if !status.isEmpty {
            stepper = "<span class=\"step now\">\(esc(status))</span>"
        }
        var pills: [String] = []
        if !v("priority").isEmpty { pills.append("<span class=\"pill warn\">\(esc(v("priority")))</span>") }
        let rel = v("releaseLabel").isEmpty ? v("release") : v("releaseLabel")
        if !rel.isEmpty { pills.append("<span class=\"pill\">\(esc(rel))</span>") }
        for l in v("labels").split(separator: ",").map({ $0.trimmingCharacters(in: .whitespaces) }) where !l.isEmpty {
            pills.append("<span class=\"pill dim\">\(esc(l))</span>")
        }
        let props: [(String, String)] = [
            ("Assignee", v("assignee").isEmpty ? "Unassigned" : v("assignee")), ("Reporter", v("reporter")),
            ("Project", v("project")), ("Release", rel), ("Priority", v("priority")),
            ("Created", date(v("created"))), ("Updated", date(v("updated"))),
        ].filter { !$0.1.isEmpty }
        let propsHTML = props.map { "<dt>\(esc($0.0))</dt><dd>\(esc($0.1))</dd>" }.joined()
        let desc = v("description")
        let descHTML = desc.isEmpty ? "<p class=\"empty\">No description.</p>"
            : desc.components(separatedBy: "\n\n").map { "<p>\(esc($0).replacingOccurrences(of: "\n", with: "<br>"))</p>" }.joined()
        let cms = comments(f["comments"])
        let cmHTML = cms.isEmpty ? "<p class=\"empty\">No comments.</p>" : cms.reversed().map { cm in
            let initials = cm.author.split(separator: " ").prefix(2).compactMap(\.first).map(String.init).joined()
            return """
            <div class="cmt"><span class="av">\(esc(initials))</span><div class="cb">
            <div class="ch"><b>\(esc(cm.author))</b> <span class="dim">· \(esc(date(cm.created)))</span></div>
            <div class="ct">\(esc(cm.body).replacingOccurrences(of: "\n", with: "<br>"))</div></div></div>
            """
        }.joined()
        let skip: Set<String> = ["comments", "description"]
        let all = f.sorted { $0.key < $1.key }.filter { !$0.key.hasPrefix("__") && !skip.contains($0.key) && !$0.value.isEmpty }
            .map { "<dt>\(esc(labels[$0.key] ?? $0.key))</dt><dd>\(esc($0.value))</dd>" }.joined()
        let openBtn = url == nil ? "" : "<button class=\"btn pri\" data-a=\"open\" title=\"Open in browser\">Open ↗</button>"
        let linkBtn = url == nil ? "" : "<button class=\"btn\" data-a=\"copy-link\">Copy link</button>"
        let tone = { (t: PopupTone) in css(c.tone(t)) }
        return """
        <!doctype html><html><head><meta charset="utf-8"><style>
        :root{--bg:\(css(c.base));--card:\(css(c.mantle));--s0:\(css(c.surface0));--s1:\(css(c.surface1));--tx:\(css(c.text));--dim:\(css(c.dim));
          --acc:\(css(c.accentOn));--on:\(css(c.onAccent));--ok:\(tone(.success));--warn:\(tone(.warning));--line:\(css(c.surface1))}
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
          <div class="top"><span class="key">\(esc(key))</span><span class="grow"></span>
            <button class="btn" data-a="copy-key">Copy key</button>\(linkBtn)\(openBtn)</div>
          <h1>\(esc(title))</h1>
          \(stepper.isEmpty ? "" : "<div class=\"steps\">\(stepper)</div>")
          \(pills.isEmpty ? "" : "<div class=\"pills\">\(pills.joined())</div>")
        </div>
        <div class="tabs"><span class="tab on" data-t="d">Details</span><span class="tab" data-t="c">Comments \(cms.count)</span><span class="tab" data-t="f">All fields</span></div>
        <div class="pane on" id="d"><div class="split"><div><p class="h">Description</p><div class="desc">\(descHTML)</div></div><dl>\(propsHTML)</dl></div></div>
        <div class="pane" id="c">\(cmHTML)</div>
        <div class="pane" id="f"><dl>\(all)</dl></div>
        <script>
        document.querySelectorAll('[data-a]').forEach(b=>b.addEventListener('click',()=>window.webkit.messageHandlers.ws.postMessage(b.dataset.a)));
        document.querySelectorAll('.tab').forEach(t=>t.addEventListener('click',()=>{
          document.querySelectorAll('.tab').forEach(x=>x.classList.toggle('on',x===t));
          document.querySelectorAll('.pane').forEach(p=>p.classList.toggle('on',p.id===t.dataset.t));
        }));
        document.addEventListener('click',e=>{const a=e.target.closest('a[href]');if(a){e.preventDefault();window.webkit.messageHandlers.ws.postMessage('url:'+a.href)}});
        </script></body></html>
        """
    }
}

final class JiraTicketView: NSView, WKScriptMessageHandler {
    let web: WKWebView
    var onAction: ((String) -> Void)?

    override init(frame: NSRect) {
        let cfg = WKWebViewConfiguration()
        web = WKWebView(frame: frame, configuration: cfg)
        super.init(frame: frame)
        cfg.userContentController.add(WeakScriptHandler(self), name: "ws")
        web.autoresizingMask = [.width, .height]
        web.frame = bounds
        web.setValue(false, forKey: "drawsBackground")
        addSubview(web)
        wantsLayer = true
    }
    required init?(coder: NSCoder) { fatalError() }

    func show(_ html: String, background: NSColor) {
        layer?.backgroundColor = background.cgColor
        web.loadHTMLString(html, baseURL: nil)
    }

    func userContentController(_ c: WKUserContentController, didReceive m: WKScriptMessage) {
        guard let s = m.body as? String else { return }
        onAction?(s)
    }
}
