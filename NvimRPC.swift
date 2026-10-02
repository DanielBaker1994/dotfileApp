import Foundation

// MARK: - nvim msgpack-RPC over its --listen socket
//
// The notes vim pane talks to its nvim through `--listen SOCK`. Every call
// used to run `nvim --headless --server SOCK --remote-expr …`: a process
// spawn, waited for ON THE MAIN THREAD — ~10 ms on a quiet Mac, far more
// where endpoint security scans every exec — on each hide, Esc, tab switch,
// edit shortcut, autosave reload (the 1 s note watcher) and dictation
// update. This is one persistent connection speaking nvim's msgpack-RPC
// (https://neovim.io/doc/user/api.html#rpc): a call is one write + one read.
//
// Answers keep `--remote-expr`'s shape: the result as text — strings as is,
// numbers in decimal, true / false, vim.NIL, "table" for lists / dicts — and
// nil when nvim is unreachable or the expression fails (the CLI's non-zero
// exit). A broken connection (nvim restarted) reconnects on the next call.
// AppKit-free: Tests/test_nvim_rpc.swift drives it against a real nvim.

indirect enum MsgPack {
    case null
    case bool(Bool)
    case int(Int64)
    case uint(UInt64)
    case double(Double)
    case str(String)
    case bin([UInt8])
    case array([MsgPack])
    case map([(MsgPack, MsgPack)])
    case ext(Int8, [UInt8])

    var int64: Int64? {
        switch self {
        case .int(let v): return v
        case .uint(let v): return v <= UInt64(Int64.max) ? Int64(v) : nil
        default: return nil
        }
    }

    // MARK: encode (what a request needs: arrays, strings, ints, …)

    func encode(into out: inout [UInt8]) {
        func be<T: FixedWidthInteger>(_ v: T) {
            withUnsafeBytes(of: v.bigEndian) { out.append(contentsOf: $0) }
        }
        switch self {
        case .null: out.append(0xc0)
        case .bool(let b): out.append(b ? 0xc3 : 0xc2)
        case .int(let v):
            if v >= 0 { MsgPack.uint(UInt64(v)).encode(into: &out) }
            else if v >= -32 { out.append(UInt8(bitPattern: Int8(v))) }
            else if v >= Int64(Int8.min) { out.append(0xd0); be(Int8(v)) }
            else if v >= Int64(Int16.min) { out.append(0xd1); be(Int16(v)) }
            else if v >= Int64(Int32.min) { out.append(0xd2); be(Int32(v)) }
            else { out.append(0xd3); be(v) }
        case .uint(let v):
            if v < 0x80 { out.append(UInt8(v)) }
            else if v <= UInt64(UInt8.max) { out.append(0xcc); be(UInt8(v)) }
            else if v <= UInt64(UInt16.max) { out.append(0xcd); be(UInt16(v)) }
            else if v <= UInt64(UInt32.max) { out.append(0xce); be(UInt32(v)) }
            else { out.append(0xcf); be(v) }
        case .double(let d): out.append(0xcb); be(d.bitPattern)
        case .str(let s):
            let b = Array(s.utf8)
            if b.count < 32 { out.append(0xa0 | UInt8(b.count)) }
            else if b.count <= Int(UInt8.max) { out.append(0xd9); be(UInt8(b.count)) }
            else if b.count <= Int(UInt16.max) { out.append(0xda); be(UInt16(b.count)) }
            else { out.append(0xdb); be(UInt32(b.count)) }
            out.append(contentsOf: b)
        case .bin(let b):
            if b.count <= Int(UInt8.max) { out.append(0xc4); be(UInt8(b.count)) }
            else if b.count <= Int(UInt16.max) { out.append(0xc5); be(UInt16(b.count)) }
            else { out.append(0xc6); be(UInt32(b.count)) }
            out.append(contentsOf: b)
        case .array(let a):
            if a.count < 16 { out.append(0x90 | UInt8(a.count)) }
            else if a.count <= Int(UInt16.max) { out.append(0xdc); be(UInt16(a.count)) }
            else { out.append(0xdd); be(UInt32(a.count)) }
            for x in a { x.encode(into: &out) }
        case .map(let m):
            if m.count < 16 { out.append(0x80 | UInt8(m.count)) }
            else if m.count <= Int(UInt16.max) { out.append(0xde); be(UInt16(m.count)) }
            else { out.append(0xdf); be(UInt32(m.count)) }
            for (k, v) in m { k.encode(into: &out); v.encode(into: &out) }
        case .ext(let t, let b):
            switch b.count {
            case 1: out.append(0xd4)
            case 2: out.append(0xd5)
            case 4: out.append(0xd6)
            case 8: out.append(0xd7)
            case 16: out.append(0xd8)
            default:
                if b.count <= Int(UInt8.max) { out.append(0xc7); be(UInt8(b.count)) }
                else if b.count <= Int(UInt16.max) { out.append(0xc8); be(UInt16(b.count)) }
                else { out.append(0xc9); be(UInt32(b.count)) }
            }
            out.append(UInt8(bitPattern: t))
            out.append(contentsOf: b)
        }
    }

    // MARK: decode (every type: nvim answers with whatever the expr gives)

    enum DecodeError: Error { case incomplete, invalid(UInt8) }

    // one value from `b` at `i` (advanced past it); `.incomplete` = the
    // bytes stop mid-value (read more and decode the message again)
    static func decode(_ b: [UInt8], _ i: inout Int) throws -> MsgPack {
        func need(_ n: Int) throws { if i + n > b.count { throw DecodeError.incomplete } }
        func uint(_ n: Int) throws -> UInt64 {
            try need(n)
            var v: UInt64 = 0
            for k in 0..<n { v = v << 8 | UInt64(b[i + k]) }
            i += n
            return v
        }
        func bytes(_ n: Int) throws -> [UInt8] {
            try need(n)
            defer { i += n }
            return Array(b[i..<i + n])
        }
        func string(_ n: Int) throws -> MsgPack {
            .str(String(decoding: try bytes(n), as: UTF8.self))
        }
        func array(_ n: Int) throws -> MsgPack {
            var a: [MsgPack] = []
            a.reserveCapacity(min(n, 1024))
            for _ in 0..<n { a.append(try decode(b, &i)) }
            return .array(a)
        }
        func map(_ n: Int) throws -> MsgPack {
            var m: [(MsgPack, MsgPack)] = []
            for _ in 0..<n { m.append((try decode(b, &i), try decode(b, &i))) }
            return .map(m)
        }
        func ext(_ n: Int) throws -> MsgPack {
            try need(1)
            let t = Int8(bitPattern: b[i])
            i += 1
            return .ext(t, try bytes(n))
        }
        try need(1)
        let tag = b[i]
        i += 1
        switch tag {
        case 0x00...0x7f: return .uint(UInt64(tag))
        case 0x80...0x8f: return try map(Int(tag & 0x0f))
        case 0x90...0x9f: return try array(Int(tag & 0x0f))
        case 0xa0...0xbf: return try string(Int(tag & 0x1f))
        case 0xc0: return .null
        case 0xc2: return .bool(false)
        case 0xc3: return .bool(true)
        case 0xc4: return .bin(try bytes(Int(try uint(1))))
        case 0xc5: return .bin(try bytes(Int(try uint(2))))
        case 0xc6: return .bin(try bytes(Int(try uint(4))))
        case 0xc7: return try ext(Int(try uint(1)))
        case 0xc8: return try ext(Int(try uint(2)))
        case 0xc9: return try ext(Int(try uint(4)))
        case 0xca: return .double(Double(Float(bitPattern: UInt32(try uint(4)))))
        case 0xcb: return .double(Double(bitPattern: try uint(8)))
        case 0xcc: return .uint(try uint(1))
        case 0xcd: return .uint(try uint(2))
        case 0xce: return .uint(try uint(4))
        case 0xcf: return .uint(try uint(8))
        case 0xd0: return .int(Int64(Int8(truncatingIfNeeded: try uint(1))))
        case 0xd1: return .int(Int64(Int16(truncatingIfNeeded: try uint(2))))
        case 0xd2: return .int(Int64(Int32(truncatingIfNeeded: try uint(4))))
        case 0xd3: return .int(Int64(bitPattern: try uint(8)))
        case 0xd4: return try ext(1)
        case 0xd5: return try ext(2)
        case 0xd6: return try ext(4)
        case 0xd7: return try ext(8)
        case 0xd8: return try ext(16)
        case 0xd9: return try string(Int(try uint(1)))
        case 0xda: return try string(Int(try uint(2)))
        case 0xdb: return try string(Int(try uint(4)))
        case 0xdc: return try array(Int(try uint(2)))
        case 0xdd: return try array(Int(try uint(4)))
        case 0xde: return try map(Int(try uint(2)))
        case 0xdf: return try map(Int(try uint(4)))
        case 0xe0...0xff: return .int(Int64(Int8(bitPattern: tag)))
        default: throw DecodeError.invalid(tag)
        }
    }
}

final class NvimRPC {
    let path: String
    private var fd: Int32 = -1
    private var msgid: UInt32 = 0
    private var inbox: [UInt8] = []
    private let lock = NSLock()

    init(path: String) { self.path = path }
    deinit { disconnect() }

    // nvim_eval(expr) as `--remote-expr` prints it; nil = unreachable / failed
    func eval(_ expr: String, timeout: Double = 1.5) -> String? {
        call("nvim_eval", [.str(expr)], timeout: timeout).map(Self.text)
    }

    // nvim_input(keys) — `--remote-send`; false = not delivered
    @discardableResult
    func input(_ keys: String, timeout: Double = 1.5) -> Bool {
        call("nvim_input", [.str(keys)], timeout: timeout) != nil
    }

    // `--remote-expr`'s text for a result (Lua's tostring of it)
    static func text(_ v: MsgPack) -> String {
        switch v {
        case .null: return "vim.NIL"
        case .bool(let b): return b ? "true" : "false"
        case .int(let n): return String(n)
        case .uint(let n): return String(n)
        case .double(let d):
            return d == d.rounded() && abs(d) < 1e15 ? String(Int64(d)) : String(d)
        case .str(let s): return s
        case .bin(let b): return String(decoding: b, as: UTF8.self)
        case .array, .map: return "table"
        case .ext: return "userdata"
        }
    }

    // one request → its result; nil on a transport error, a timeout or an
    // nvim error. A dead connection is reopened once (nvim restarted).
    func call(_ method: String, _ params: [MsgPack], timeout: Double) -> MsgPack? {
        lock.lock()
        defer { lock.unlock() }
        let deadline = Date().addingTimeInterval(timeout)
        for _ in 0..<2 {
            if fd < 0, !connect() { return nil }
            msgid &+= 1
            let id = Int64(msgid)
            var out: [UInt8] = []
            MsgPack.array([.uint(0), .uint(UInt64(id)), .str(method), .array(params)]).encode(into: &out)
            guard writeAll(out) else { disconnect(); continue }
            var closed = false
            while !closed {
                switch nextMessage(deadline: deadline) {
                case .closed:
                    // nvim went away (restarted): reconnect + resend once
                    disconnect()
                    closed = true
                case .timeout:
                    // the stream may still owe a late answer — never reuse
                    // it out of step
                    disconnect()
                    return nil
                case .message(let msg):
                    // [1, msgid, error, result]; notifications ([2, …]) skipped
                    guard case .array(let a) = msg, a.count == 4, a[0].int64 == 1, a[1].int64 == id else { continue }
                    if case .null = a[2] { return a[3] }
                    return nil
                }
            }
        }
        return nil
    }

    private enum Next { case message(MsgPack), timeout, closed }

    func disconnect() {
        if fd >= 0 { close(fd) }
        fd = -1
        inbox.removeAll()
    }

    private func connect() -> Bool {
        guard FileManager.default.fileExists(atPath: path) else { return false }
        let s = socket(AF_UNIX, SOCK_STREAM, 0)
        guard s >= 0 else { return false }
        var on: Int32 = 1
        setsockopt(s, SOL_SOCKET, SO_NOSIGPIPE, &on, socklen_t(MemoryLayout<Int32>.size))
        _ = fcntl(s, F_SETFD, FD_CLOEXEC)
        var addr = sockaddr_un()
        addr.sun_family = sa_family_t(AF_UNIX)
        let bytes = Array(path.utf8)
        guard bytes.count < MemoryLayout.size(ofValue: addr.sun_path) else { close(s); return false }
        withUnsafeMutableBytes(of: &addr.sun_path) { p in
            for (k, c) in bytes.enumerated() { p[k] = c }
        }
        let ok = withUnsafePointer(to: &addr) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                Darwin.connect(s, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) == 0
            }
        }
        guard ok else { close(s); return false }
        fd = s
        inbox.removeAll()
        return true
    }

    private func writeAll(_ data: [UInt8]) -> Bool {
        var off = 0
        while off < data.count {
            let n = data[off...].withUnsafeBytes { write(fd, $0.baseAddress, $0.count) }
            if n < 0, errno == EINTR { continue }
            guard n > 0 else { return false }
            off += n
        }
        return true
    }

    // the next whole message from the socket
    private func nextMessage(deadline: Date) -> Next {
        while true {
            if !inbox.isEmpty {
                var i = 0
                do {
                    let v = try MsgPack.decode(inbox, &i)
                    inbox.removeFirst(i)
                    return .message(v)
                } catch MsgPack.DecodeError.incomplete {
                    // fall through: read more
                } catch {
                    return .timeout   // garbage: drop the connection, no resend
                }
            }
            let ms = Int32(max(0, deadline.timeIntervalSinceNow * 1000))
            var pfd = pollfd(fd: fd, events: Int16(POLLIN), revents: 0)
            let r = poll(&pfd, 1, ms)
            if r < 0, errno == EINTR { continue }
            guard r > 0 else { return .timeout }
            var buf = [UInt8](repeating: 0, count: 65536)
            let n = read(fd, &buf, buf.count)
            if n < 0, errno == EINTR { continue }
            guard n > 0 else { return .closed }
            inbox.append(contentsOf: buf[0..<n])
        }
    }
}
