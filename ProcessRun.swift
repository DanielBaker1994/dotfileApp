import Foundation

struct ProcessOutput {
    let code: Int32
    let out: String
    let err: String
}

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
