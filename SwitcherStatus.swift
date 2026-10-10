import Foundation

struct UnreadChip {
    let app: String
    let count: String
    let mentions: Int
    let warn: Bool
}

struct SwitcherStatus {
    var cpu: Int?
    var ram: Int?
    var battery: (pct: Int, charging: Bool)?
    var chips: [UnreadChip] = []

    var unreadByApp: [String: String] {
        var out: [String: String] = [:]
        for c in chips where !c.count.isEmpty { out[c.app] = c.count }
        return out
    }

    static func gather(notifyScript: String) -> SwitcherStatus {
        var s = SwitcherStatus()
        guard case .success(let box) = PythonHelper.shared.callSync(
                "status.gather", ["notify_script": notifyScript], timeout: 60),
              let dict = box as? [String: Any] else { return s }
        s.cpu = dict["cpu"] as? Int
        s.ram = dict["ram"] as? Int
        if let b = dict["battery"] as? [String: Any], let pct = b["pct"] as? Int {
            s.battery = (pct, b["charging"] as? Bool ?? false)
        }
        s.chips = (dict["chips"] as? [[String: Any]] ?? []).map {
            UnreadChip(app: $0["app"] as? String ?? "",
                       count: $0["count"] as? String ?? "",
                       mentions: $0["mentions"] as? Int ?? 0,
                       warn: $0["warn"] as? Bool ?? false)
        }
        return s
    }
}
