import AppKit
import WebKit

enum JiraTicketPage {
    static let categoryNames = ["To Do", "In Progress", "Done"]

    static func category(of status: String) -> Int {
        category(of: status, in: JiraPoll.readJSON(JiraPoll.directoryPath)?["statusCategories"] as? [String: String] ?? [:])
    }
    static func category(of status: String, in cats: [String: String]) -> Int {
        switch cats[status] {
        case "new": return 0
        case "indeterminate": return 1
        case "done": return 2
        default: break
        }
        let l = status.lowercased(), s = JiraStyle.current
        if s.matches(l, s.words["done"] ?? []) || s.matches(l, s.words["cancelled"] ?? []) { return 2 }
        if s.matches(l, s.words["new"] ?? []) { return 0 }
        return 1
    }

    typealias Comment = (author: String, body: String, created: String)
    private static var commentsByKey: [String: [Comment]] = [:]
    private static var commentsStamp: Date?
    private static var commentLoading = false

    private static func cacheStamp() -> Date? {
        (try? FileManager.default.attributesOfItem(atPath: JiraPoll.issueCachePath))?[.modificationDate] as? Date
    }

    /// Comments for one issue, parsed by pylib/jira_data.py (one parse per
    /// cache write; the worker keeps the mtime-keyed index). Same contract as
    /// before: immediate answer when the mirror is current for this cache
    /// write, `[]` when there is no cache, `nil` + `ready` while loading.
    static func cachedComments(_ key: String, ready: @escaping () -> Void) -> [Comment]? {
        let stamp = cacheStamp()
        if stamp != nil, stamp == commentsStamp, let c = commentsByKey[key] { return c }
        if stamp == nil { return [] }
        if !commentLoading {
            commentLoading = true
            pythonHelper.call("jira.comments", ["path": JiraPoll.issueCachePath, "key": key],
                              timeout: 120) { result in
                commentLoading = false
                switch result {
                case .success(let box):
                    let d = box as? [String: Any]
                    commentsStamp = stamp
                    commentsByKey[key] = ((d?["comments"] as? [[String: Any]]) ?? []).map {
                        ($0["author"] as? String ?? "", $0["body"] as? String ?? "",
                         $0["created"] as? String ?? "")
                    }
                case .failure(let e):
                    wsLog("jira: comments unresolved: \(e.description)")
                    commentsStamp = stamp
                    commentsByKey[key] = []
                }
                ready()
            }
        }
        return nil
    }

    private static func hex(_ c: NSColor) -> String { ProseRender.css(c) }

    /// The ticket page, built by pylib/jira_pages.py; colors go over as hex
    /// (the theme algebra still lives in Swift for now). Completion on main.
    static func ticketHTML(_ row: FieldRow, colors c: PopupColors, url: String?,
                           labels: [String: String], comments: [Comment]?,
                           done: @escaping (String?) -> Void) {
        var params: [String: Any] = [
            "fields": row.fields,
            "title": row.title,
            "categories": JiraPoll.readJSON(JiraPoll.directoryPath)?["statusCategories"] as? [String: String] ?? [:],
            "words": JiraStyle.current.words,
            "labels": labels,
            "colors": ["bg": hex(c.base), "card": hex(c.mantle), "s0": hex(c.surface0),
                       "s1": hex(c.surface1), "tx": hex(c.text), "dim": hex(c.dim),
                       "acc": hex(c.accentOn), "on": hex(c.onAccent),
                       "ok": hex(c.tone(.success)), "warn": hex(c.tone(.warning))],
        ]
        if let comments { params["comments"] = comments.map { ["author": $0.author, "body": $0.body, "created": $0.created] } }
        if let w = configSectionValue("jira", "workflow"), !w.isEmpty { params["workflow"] = w }
        if let url { params["url"] = url }
        pythonHelper.call("jira.ticket_html", params, timeout: 60) { result in
            if case .success(let box) = result, let d = box as? [String: Any],
               let html = d["html"] as? String {
                done(html)
            } else {
                if case .failure(let e) = result { wsLog("jira: ticket page failed: \(e.description)") }
                done(nil)
            }
        }
    }

    /// The comments fragment for the in-page refresh (built by python too).
    static func commentsHTML(_ cms: [Comment], done: @escaping (String) -> Void) {
        let rows = cms.map { ["author": $0.author, "body": $0.body, "created": $0.created] }
        pythonHelper.call("jira.comments_html", ["comments": rows], timeout: 60) { result in
            if case .success(let box) = result, let d = box as? [String: Any],
               let html = d["html"] as? String {
                done(html)
            } else {
                done("")
            }
        }
    }
}

final class JiraTicketView: NSView, WKScriptMessageHandler, PageZoomable {
    let web: WKWebView
    var pageZoom: CGFloat { get { web.pageZoom } set { web.pageZoom = newValue } }
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

    func setComments(_ cms: [JiraTicketPage.Comment]) {
        if web.isLoading {
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.1) { [weak self] in self?.setComments(cms) }
            return
        }
        JiraTicketPage.commentsHTML(cms) { [weak self] frag in
            guard let self else { return }
            func js(_ s: String) -> String {
                let d = (try? JSONSerialization.data(withJSONObject: [s])) ?? Data("[\"\"]".utf8)
                return String(decoding: d, as: UTF8.self) + "[0]"
            }
            self.web.evaluateJavaScript("""
                document.getElementById('c').innerHTML=\(js(frag));
                document.getElementById('ctab').textContent=\(js("Comments \(cms.count)"));
                """)
        }
    }

    func userContentController(_ c: WKUserContentController, didReceive m: WKScriptMessage) {
        guard let s = m.body as? String else { return }
        onAction?(s)
    }
}
