import Darwin
import Foundation

final class PythonHelper {
    struct Failure: Error, CustomStringConvertible {
        let description: String
        var localizedDescription: String { description }
    }

    static let shared = PythonHelper(libDir: "")

    private(set) var libDir: String

    func configure(libDir: String) {
        self.libDir = libDir
    }

    private struct Pending {
        let method: String
        let payload: Data
        let timeout: TimeInterval
        var attempts: Int
        let deliverOnMain: Bool
        let done: (Result<Any, Failure>) -> Void
    }

    private final class Box {
        var result: Result<Any, Failure>?
    }

    private let queue = DispatchQueue(label: "ws.python-helper")
    private var process: Process?
    private var stdin: FileHandle?
    private var stdoutPipe: Pipe?
    private var stdoutBuffer = Data()
    private var nextID = 1
    private var pending: [Int: Pending] = [:]
    private var pythonPath: String?
    private var pythonLookedUp = false
    private var generation = 0

    init(libDir: String) {
        self.libDir = libDir
    }

    func call(_ method: String, _ params: [String: Any] = [:],
              timeout: TimeInterval = 10,
              done: @escaping (Result<Any, Failure>) -> Void) {
        queue.async { self.callLocked(method, params, timeout, true, done) }
    }

    /// Block the calling thread until the worker answers. Never call this
    /// from the main thread while the worker may be slow.
    func callSync(_ method: String, _ params: [String: Any] = [:],
                  timeout: TimeInterval = 10) -> Result<Any, Failure> {
        let sem = DispatchSemaphore(value: 0)
        let box = Box()
        queue.async {
            self.callLocked(method, params, timeout, false) { result in
                box.result = result
                sem.signal()
            }
        }
        if sem.wait(timeout: .now() + timeout + 5) == .timedOut {
            return .failure(Failure(description: "helper timed out: \(method)"))
        }
        return box.result ?? .failure(Failure(description: "helper: no answer for \(method)"))
    }

    func killForTesting() {
        queue.sync {
            if let p = process, p.isRunning { kill(p.processIdentifier, SIGKILL) }
        }
    }

    private func callLocked(_ method: String, _ params: [String: Any],
                            _ timeout: TimeInterval, _ deliverOnMain: Bool,
                            _ done: @escaping (Result<Any, Failure>) -> Void) {
        guard !libDir.isEmpty else {
            deliver(done, deliverOnMain, .failure(Failure(description: "python helper not configured")))
            return
        }
        let id = nextID
        nextID += 1
        guard let payload = try? JSONSerialization.data(
            withJSONObject: ["id": id, "method": method, "params": params]) else {
            deliver(done, deliverOnMain, .failure(Failure(description: "helper: cannot encode request for \(method)")))
            return
        }
        pending[id] = Pending(method: method, payload: payload, timeout: timeout,
                              attempts: 1, deliverOnMain: deliverOnMain, done: done)
        guard ensureStarted() else {
            finish(id, .failure(Failure(description:
                "python 3.11+ not found — brew install python, or set WS_PYTHON")))
            return
        }
        if !write(payload) {
            stopLocked(generation)
            return
        }
        scheduleTimeout(id, attempts: 1, method: method, timeout: timeout)
    }

    private func scheduleTimeout(_ id: Int, attempts: Int, method: String, timeout: TimeInterval) {
        queue.asyncAfter(deadline: .now() + timeout) { [weak self] in
            guard let self, self.pending[id]?.attempts == attempts else { return }
            self.finish(id, .failure(Failure(description: "helper timed out: \(method)")))
        }
    }

    private func send(_ payload: Data) -> Bool {
        if !ensureStarted() { return false }
        return write(payload)
    }

    private func write(_ payload: Data) -> Bool {
        guard let stdin else { return false }
        var line = payload
        line.append(0x0A)
        do {
            try stdin.write(contentsOf: line)
            return true
        } catch {
            return false
        }
    }

    private func ensureStarted() -> Bool {
        if let p = process, p.isRunning { return true }
        teardownProcess()
        guard let python = findPython() else { return false }
        generation += 1
        let gen = generation
        let p = Process()
        p.executableURL = URL(fileURLWithPath: python)
        p.arguments = ["-B", "-m", "helper"]
        var env = ProcessInfo.processInfo.environment
        env["PATH"] = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:" + (env["PATH"] ?? "")
        env["PYTHONPATH"] = libDir + (env["PYTHONPATH"].map { ":" + $0 } ?? "")
        env["PYTHONDONTWRITEBYTECODE"] = "1"
        p.environment = env
        p.currentDirectoryURL = URL(fileURLWithPath: libDir, isDirectory: true)
        let out = Pipe(), inp = Pipe()
        p.standardOutput = out
        p.standardError = FileHandle.nullDevice
        p.standardInput = inp
        do {
            try p.run()
        } catch {
            return false
        }
        _ = fcntl(inp.fileHandleForWriting.fileDescriptor, F_SETNOSIGPIPE, 1)
        process = p
        stdin = inp.fileHandleForWriting
        stdoutPipe = out
        stdoutBuffer.removeAll()
        out.fileHandleForReading.readabilityHandler = { [weak self] fh in
            let data = fh.availableData
            guard let self else { return }
            self.queue.async {
                if data.isEmpty { self.stopLocked(gen) }
                else { self.ingest(data) }
            }
        }
        p.terminationHandler = { [weak self] _ in
            self?.queue.async { self?.stopLocked(gen) }
        }
        return true
    }

    private func stopLocked(_ gen: Int) {
        guard gen == generation else { return }
        generation += 1
        teardownProcess()
        let waiting = pending
        pending.removeAll()
        for (id, item) in waiting {
            if item.attempts < 2 {
                var retry = item
                retry.attempts += 1
                pending[id] = retry
                if send(retry.payload) {
                    scheduleTimeout(id, attempts: retry.attempts, method: retry.method,
                                    timeout: retry.timeout)
                    continue
                }
                pending.removeValue(forKey: id)
            }
            if item.deliverOnMain {
                DispatchQueue.main.async { item.done(.failure(Failure(description: "helper exited"))) }
            } else {
                item.done(.failure(Failure(description: "helper exited")))
            }
        }
    }

    private func teardownProcess() {
        if let p = process {
            p.terminationHandler = nil
            if p.isRunning { p.terminate() }
        }
        stdoutPipe?.fileHandleForReading.readabilityHandler = nil
        process = nil
        stdin = nil
        stdoutPipe = nil
        stdoutBuffer.removeAll()
    }

    private func ingest(_ data: Data) {
        stdoutBuffer.append(data)
        while let newline = stdoutBuffer.firstIndex(of: 0x0A) {
            let line = stdoutBuffer[..<newline]
            stdoutBuffer.removeSubrange(...newline)
            guard let obj = try? JSONSerialization.jsonObject(with: Data(line)) as? [String: Any],
                  let id = obj["id"] as? Int else { continue }
            let ok = obj["ok"] as? Bool ?? false
            if ok {
                finish(id, .success(obj["result"] ?? NSNull()))
            } else {
                let message = (obj["error"] as? [String: Any])?["message"] as? String
                    ?? "helper error"
                finish(id, .failure(Failure(description: message)))
            }
        }
    }

    private func finish(_ id: Int, _ result: Result<Any, Failure>) {
        guard let item = pending.removeValue(forKey: id) else { return }
        deliver(item.done, item.deliverOnMain, result)
    }

    private func deliver(_ done: @escaping (Result<Any, Failure>) -> Void, _ onMain: Bool,
                         _ result: Result<Any, Failure>) {
        if onMain {
            DispatchQueue.main.async { done(result) }
        } else {
            done(result)
        }
    }

    private func findPython() -> String? {
        if pythonLookedUp { return pythonPath }
        pythonLookedUp = true
        var candidates: [String] = []
        if let own = ProcessInfo.processInfo.environment["WS_PYTHON"], !own.isEmpty {
            candidates.append(own)
        }
        candidates += ["/opt/homebrew/bin/python3", "/usr/local/bin/python3"]
        for dir in (ProcessInfo.processInfo.environment["PATH"] ?? "").split(separator: ":")
        where dir != "/usr/bin" && dir != "/bin" {
            candidates.append(String(dir) + "/python3")
        }
        for candidate in candidates where FileManager.default.isExecutableFile(atPath: candidate) {
            if pythonRuns311(candidate) {
                pythonPath = candidate
                return candidate
            }
        }
        return nil
    }

    private func pythonRuns311(_ path: String) -> Bool {
        let p = Process()
        p.executableURL = URL(fileURLWithPath: path)
        p.arguments = ["-c", "import sys; sys.exit(0 if sys.version_info >= (3, 11) else 1)"]
        var env = ProcessInfo.processInfo.environment
        env["PYTHONDONTWRITEBYTECODE"] = "1"
        env["PYTHONPATH"] = libDir
        p.environment = env
        p.standardOutput = FileHandle.nullDevice
        p.standardError = FileHandle.nullDevice
        do {
            try p.run()
        } catch {
            return false
        }
        let exited = DispatchGroup()
        exited.enter()
        DispatchQueue.global(qos: .userInitiated).async {
            p.waitUntilExit()
            exited.leave()
        }
        if exited.wait(timeout: .now() + 5) == .timedOut {
            p.terminate()
            return false
        }
        return p.terminationStatus == 0
    }
}
