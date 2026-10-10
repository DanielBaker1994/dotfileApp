import Foundation
import IOKit.ps

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
        let a = cpuTicks(), started = Date()
        s.chips = readUnread(notifyScript)
        s.ram = ramPercent()
        s.battery = battery()
        let left = 0.3 - Date().timeIntervalSince(started)
        if left > 0 { Thread.sleep(forTimeInterval: left) }
        if let a, let b = cpuTicks(), b.total > a.total {
            s.cpu = Int((Double(b.busy - a.busy) / Double(b.total - a.total) * 100).rounded())
        }
        return s
    }

    private static func cpuTicks() -> (busy: UInt64, total: UInt64)? {
        var info = host_cpu_load_info()
        var count = mach_msg_type_number_t(MemoryLayout<host_cpu_load_info>.stride / MemoryLayout<integer_t>.stride)
        let r = withUnsafeMutablePointer(to: &info) {
            $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
                host_statistics(mach_host_self(), HOST_CPU_LOAD_INFO, $0, &count)
            }
        }
        guard r == KERN_SUCCESS else { return nil }
        let t = info.cpu_ticks
        let busy = UInt64(t.0) + UInt64(t.1) + UInt64(t.3)
        return (busy, busy + UInt64(t.2))
    }

    private static func ramPercent() -> Int? {
        var vm = vm_statistics64()
        var count = mach_msg_type_number_t(MemoryLayout<vm_statistics64>.stride / MemoryLayout<integer_t>.stride)
        let r = withUnsafeMutablePointer(to: &vm) {
            $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
                host_statistics64(mach_host_self(), HOST_VM_INFO64, $0, &count)
            }
        }
        guard r == KERN_SUCCESS else { return nil }
        let pages = UInt64(vm.active_count) + UInt64(vm.wire_count) + UInt64(vm.compressor_page_count)
        let total = ProcessInfo.processInfo.physicalMemory
        guard total > 0 else { return nil }
        return Int(Double(pages * UInt64(vm_kernel_page_size)) / Double(total) * 100)
    }

    private static func battery() -> (pct: Int, charging: Bool)? {
        guard let blob = IOPSCopyPowerSourcesInfo()?.takeRetainedValue(),
              let list = IOPSCopyPowerSourcesList(blob)?.takeRetainedValue() as? [CFTypeRef] else { return nil }
        for src in list {
            guard let d = IOPSGetPowerSourceDescription(blob, src)?.takeUnretainedValue() as? [String: Any],
                  d[kIOPSTypeKey] as? String == kIOPSInternalBatteryType,
                  let cur = d[kIOPSCurrentCapacityKey] as? Int,
                  let max = d[kIOPSMaxCapacityKey] as? Int, max > 0 else { continue }
            return (cur * 100 / max, d[kIOPSPowerSourceStateKey] as? String == kIOPSACPowerValue)
        }
        return nil
    }

    private static func readUnread(_ script: String) -> [UnreadChip] {
        guard FileManager.default.fileExists(atPath: script) else { return [] }
        var env = ProcessInfo.processInfo.environment
        env["PATH"] = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:" + (env["PATH"] ?? "")
        env["PYTHONDONTWRITEBYTECODE"] = "1"
        guard let r = try? runProcess("/usr/bin/env", ["python3", script, "--json"], env: env),
              r.code == 0,
              let obj = try? JSONSerialization.jsonObject(with: Data(r.out.utf8)) as? [String: Any],
              let srcs = obj["sources"] as? [[String: Any]] else { return [] }
        return srcs.compactMap { d in
            guard let app = d["app"] as? String else { return nil }
            let count: String
            switch d["count"] {
            case let n as Int: count = n == 0 ? "" : n > 999 ? "999+" : String(n)
            case let t as String: count = t
            default: count = ""
            }
            return UnreadChip(app: app, count: count, mentions: d["mentions"] as? Int ?? 0,
                              warn: d["warn"] as? Bool ?? false)
        }
    }
}
