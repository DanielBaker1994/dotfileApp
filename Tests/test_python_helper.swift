// sources: PythonHelper.swift
import Foundation

var passed = 0
var failed = 0
func check(_ condition: Bool, _ message: String, line: Int = #line) {
    if condition { passed += 1 } else { failed += 1; print("  FAIL: \(message) (test_python_helper.swift:\(line))") }
}

func pump(_ timeout: TimeInterval, _ done: @escaping () -> Bool) {
    let deadline = Date().addingTimeInterval(timeout)
    while !done() && Date() < deadline {
        RunLoop.main.run(mode: .default, before: Date().addingTimeInterval(0.02))
    }
}

let libDir = URL(fileURLWithPath: #filePath)
    .deletingLastPathComponent().deletingLastPathComponent()
    .appendingPathComponent("pylib").path

@main
struct Main {
    static func main() {
        let helper = PythonHelper(libDir: libDir)

        print("ping:")
        var pong: [String: Any]?
        helper.call("ping") { r in if case .success(let v) = r { pong = v as? [String: Any] } }
        pump(15) { pong != nil }
        check(pong?["version"] as? Int == 1, "ping answers with a version: \(String(describing: pong))")

        print("unknown method:")
        var unknown: String?
        helper.call("no-such-method") { r in if case .failure(let f) = r { unknown = f.description } }
        pump(15) { unknown != nil }
        check(unknown?.contains("unknown method") == true, "fails with its name: \(unknown ?? "-")")

        print("concurrency:")
        var answered = 0
        for _ in 0..<25 {
            helper.call("ping") { _ in answered += 1 }
        }
        pump(20) { answered == 25 }
        check(answered == 25, "25 calls in flight all answer (got \(answered))")

        print("restart:")
        helper.killForTesting()
        var recovered = false
        helper.call("ping") { r in if case .success = r { recovered = true } }
        pump(20) { recovered }
        check(recovered, "the worker restarts after being killed")

        print("broken worker:")
        let emptyDir = NSTemporaryDirectory() + "ws-helper-empty-\(getpid())"
        try? FileManager.default.createDirectory(atPath: emptyDir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(atPath: emptyDir) }
        let broken = PythonHelper(libDir: emptyDir)
        var bad: String?
        broken.call("ping", timeout: 8) { r in if case .failure(let f) = r { bad = f.description } }
        pump(12) { bad != nil }
        check(bad != nil, "a package-less worker fails fast: \(bad ?? "hung")")

        print("python helper client: \(passed) passed, \(failed) failed")
        exit(failed == 0 ? 0 : 1)
    }
}
