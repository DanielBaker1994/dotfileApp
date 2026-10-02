// sources: NvimRPC.swift
// The notes pane's nvim client (NvimRPC.swift) against a REAL headless nvim:
// msgpack round trips, `--remote-expr` parity (the answers the pane's code
// compares against), input, big answers, errors, a restarted nvim, speed.
// Usage: bin/run-tests.sh nvim

import Foundation

@main
struct NvimRPCTests {
    static var passed = 0
    static var failed = 0

    static func check(_ condition: Bool, _ message: String, line: Int = #line) {
        if condition {
            passed += 1
        } else {
            failed += 1
            print("  FAIL: \(message) (test_nvim_rpc.swift:\(line))")
        }
    }

    static func which(_ name: String) -> String? {
        for d in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"] where FileManager.default.isExecutableFile(atPath: "\(d)/\(name)") {
            return "\(d)/\(name)"
        }
        return nil
    }

    static func startNvim(_ nvim: String, _ sock: String) -> Process {
        let p = Process()
        p.executableURL = URL(fileURLWithPath: nvim)
        p.arguments = ["--headless", "--clean", "--listen", sock]
        p.standardInput = FileHandle.nullDevice
        p.standardOutput = FileHandle.nullDevice
        p.standardError = FileHandle.nullDevice
        try? p.run()
        for _ in 0..<200 where !FileManager.default.fileExists(atPath: sock) { usleep(10_000) }
        return p
    }

    // what `nvim --server SOCK --remote-expr EXPR` prints (nil = it failed)
    static func cli(_ nvim: String, _ sock: String, _ expr: String) -> String? {
        let p = Process()
        p.executableURL = URL(fileURLWithPath: nvim)
        p.arguments = ["--headless", "--clean", "--server", sock, "--remote-expr", expr]
        let out = Pipe()
        p.standardOutput = out
        p.standardError = FileHandle.nullDevice
        try? p.run()
        p.waitUntilExit()
        let s = String(decoding: out.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
        return p.terminationStatus == 0 ? s : nil
    }

    static func roundTrip(_ v: MsgPack) -> MsgPack? {
        var out: [UInt8] = []
        v.encode(into: &out)
        var i = 0
        guard let back = try? MsgPack.decode(out, &i), i == out.count else { return nil }
        return back
    }

    static func main() {
        // --- msgpack, no nvim needed ---
        for n: Int64 in [0, 1, 127, 128, 255, 256, 65535, 65536, 4_294_967_296, -1, -32, -33, -128, -129, -32768, -32769, Int64.min] {
            check(roundTrip(.int(n))?.int64 == n, "int \(n) round trip")
        }
        for s in ["", "a", String(repeating: "x", count: 31), String(repeating: "y", count: 32),
                  String(repeating: "z", count: 300), String(repeating: "w", count: 70_000), "héllo ✓"] {
            if case .str(let back)? = roundTrip(.str(s)) { check(back == s, "str of \(s.utf8.count) bytes") } else { check(false, "str of \(s.utf8.count) bytes") }
        }
        if case .array(let a)? = roundTrip(.array((0..<20).map { .int($0) })) { check(a.count == 20 && a[19].int64 == 19, "array16") } else { check(false, "array16") }
        if case .double(let d)? = roundTrip(.double(1.5)) { check(d == 1.5, "double") } else { check(false, "double") }
        var i = 0
        do { _ = try MsgPack.decode([0x92, 0x01], &i); check(false, "truncated array must be incomplete") }
        catch MsgPack.DecodeError.incomplete { check(true, "truncated array is incomplete") }
        catch { check(false, "truncated array: wrong error \(error)") }

        guard let nvim = which("nvim") else {
            print("  SKIP: no nvim on this Mac (the msgpack checks ran)")
            print("\nnvim rpc: \(passed) passed, \(failed) failed")
            exit(failed == 0 ? 0 : 1)
        }
        let dir = NSTemporaryDirectory() + "nvimrpc-\(getpid())"
        try? FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
        let sock = dir + "/n.sock"
        var p = startNvim(nvim, sock)
        defer { p.terminate(); try? FileManager.default.removeItem(atPath: dir) }
        let rpc = NvimRPC(path: sock)

        // --- parity with --remote-expr: what the pane's code compares against ---
        for e in ["mode()", "1+1", "0-3", "0-200", "0-70000", "v:true", "v:false", "v:null",
                  "luaeval('nil')", "1.5", "execute('echo 1')", "'héllo ✓'", "[1,'a']", "{'a':1}"] {
            let mine = rpc.eval(e)
            var want = cli(nvim, sock, e)
            // the CLI prints a table's address (table: 0x…): only the kind matters
            if want?.hasPrefix("table: ") == true { want = "table" }
            check(mine == want, "eval \(e): rpc \(mine.debugDescription) vs cli \(want.debugDescription)")
        }
        check(rpc.eval("nonexistent_fn()") == nil, "an nvim error is nil (the CLI exits non-zero)")
        check(rpc.eval("1+1") == "2", "the connection survives an error")

        // a big answer arrives in several reads
        check(rpc.eval("repeat('x', 200000)")?.count == 200_000, "200 KB answer")

        // input (--remote-send), then read it back
        check(rpc.input("ihello<Esc>"), "input delivered")
        usleep(50_000)
        check(rpc.eval("getline(1)") == "hello", "typed text landed")
        check(rpc.eval("mode()") == "n", "back in Normal mode")

        // speed: what used to be one process spawn per call
        let t0 = Date()
        for _ in 0..<200 { _ = rpc.eval("mode()") }
        let ms = Date().timeIntervalSince(t0) * 1000 / 200
        print(String(format: "  %.3f ms per call (200 calls)", ms))
        check(ms < 5, "a call is a few ms at most")

        // nvim restarted on the same socket: the next call reconnects
        p.terminate()
        p.waitUntilExit()
        try? FileManager.default.removeItem(atPath: sock)
        p = startNvim(nvim, sock)
        check(rpc.eval("1+2") == "3", "first call after an nvim restart reconnects")

        // nobody listening: nil, fast
        let gone = NvimRPC(path: dir + "/none.sock")
        let t1 = Date()
        check(gone.eval("1") == nil, "no socket = nil")
        check(Date().timeIntervalSince(t1) < 0.2, "no socket fails fast")

        print("\nnvim rpc: \(passed) passed, \(failed) failed")
        exit(failed == 0 ? 0 : 1)
    }
}
