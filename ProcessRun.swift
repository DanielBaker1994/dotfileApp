// ProcessRun.swift — run a program to completion and collect its output.
// Foundation only: the AppKit-free files (AIFormat, RecentFiles) and their
// tests compile it too.

import Foundation

struct ProcessOutput {
    let code: Int32
    let out: String
    let err: String   // empty with mergeStderr (it went into `out`)
}

// Runs `exe args` to completion — blocking, so call it off the main thread
// for anything slow. `stdin` is fed to the program (none = /dev/null);
// stdout and stderr are drained concurrently, so a full stderr pipe can
// never stall the child while stdout is read. `mergeStderr`: one stream,
// in `out`. Throws when the program can't be started.
func runProcess(_ exe: String, _ args: [String], stdin: String? = nil,
                env: [String: String]? = nil, mergeStderr: Bool = false) throws -> ProcessOutput {
    let p = Process()
    p.executableURL = URL(fileURLWithPath: exe)
    p.arguments = args
    if let env { p.environment = env }
    let out = Pipe(), err = Pipe()
    p.standardOutput = out
    p.standardError = mergeStderr ? out : err
    let inp = stdin == nil ? nil : Pipe()
    p.standardInput = inp ?? FileHandle.nullDevice
    try p.run()
    var outData = Data(), errData = Data()
    let drained = DispatchGroup()
    func drain(_ pipe: Pipe, into keep: @escaping (Data) -> Void) {
        drained.enter()
        DispatchQueue.global(qos: .userInitiated).async {
            keep(pipe.fileHandleForReading.readDataToEndOfFile())
            drained.leave()
        }
    }
    drain(out) { outData = $0 }
    if !mergeStderr { drain(err) { errData = $0 } }
    if let inp, let stdin {
        let w = inp.fileHandleForWriting
        // a program that exits without reading its input must not take the
        // app down with SIGPIPE: the write just fails
        _ = fcntl(w.fileDescriptor, F_SETNOSIGPIPE, 1)
        try? w.write(contentsOf: Data(stdin.utf8))
        try? w.close()
    }
    p.waitUntilExit()
    drained.wait()
    return ProcessOutput(code: p.terminationStatus,
                         out: String(decoding: outData, as: UTF8.self),
                         err: String(decoding: errData, as: UTF8.self))
}
