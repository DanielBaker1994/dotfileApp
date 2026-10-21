// ks-ocr — turn a screenshot into text a non-vision model can check.
//
//   ks-ocr <file.png>                      one JSON object: {"lines":[{text,conf,x,y,w,h}], "blank":bool, ...}
//   ks-ocr <file.png> --find <text>        exit 0 if some line contains <text> (case-insensitive), else 1
//   ks-ocr <file.png> --absent <text>      exit 0 if no line contains <text>, else 1
//
// Coordinates are in image pixels, top-left origin. `blank` is true when one
// colour covers >= 98% of a sampled grid (a blank/unpainted pane). Uses the
// macOS Vision framework (accurate mode), no network, about 0.3-1 s per image.
import AppKit
import Foundation
import Vision

func fail(_ msg: String, _ code: Int32) -> Never {
    FileHandle.standardError.write((msg + "\n").data(using: .utf8)!)
    exit(code)
}

var args = Array(CommandLine.arguments.dropFirst())
if args.isEmpty || args[0] == "-h" || args[0] == "--help" || args[0] == "help" {
    print("usage: ks-ocr <file.png> [--find TEXT | --absent TEXT]")
    exit(args.isEmpty ? 2 : 0)
}
let path = args.removeFirst()
var find: String?
var absent: String?
while !args.isEmpty {
    let a = args.removeFirst()
    guard !args.isEmpty else { fail("ks-ocr: \(a) needs a value", 2) }
    switch a {
    case "--find": find = args.removeFirst()
    case "--absent": absent = args.removeFirst()
    default: fail("ks-ocr: unknown option \(a)", 2)
    }
}

guard let img = NSImage(contentsOfFile: path),
      let cg = img.cgImage(forProposedRect: nil, context: nil, hints: nil)
else { fail("ks-ocr: cannot read image \(path)", 4) }
let W = cg.width, H = cg.height

// Blank check: sample a 40x40 grid and count the most common colour.
var blank = false
var dominant = 0.0
if let rep = Optional(NSBitmapImageRep(cgImage: cg)) {
    var counts: [UInt32: Int] = [:]
    let n = 40
    for gy in 0..<n {
        for gx in 0..<n {
            let x = min(W - 1, gx * W / n + W / (2 * n))
            let y = min(H - 1, gy * H / n + H / (2 * n))
            guard let c = rep.colorAt(x: x, y: y)?.usingColorSpace(.deviceRGB) else { continue }
            // Quantise to 5 bits per channel so antialiasing noise doesn't split a flat fill.
            let k = (UInt32(c.redComponent * 31) << 10) | (UInt32(c.greenComponent * 31) << 5) | UInt32(c.blueComponent * 31)
            counts[k, default: 0] += 1
        }
    }
    let total = counts.values.reduce(0, +)
    dominant = total == 0 ? 1 : Double(counts.values.max() ?? 0) / Double(total)
    blank = dominant >= 0.98
}

let req = VNRecognizeTextRequest()
req.recognitionLevel = .accurate
req.usesLanguageCorrection = false
do {
    try VNImageRequestHandler(cgImage: cg, options: [:]).perform([req])
} catch {
    fail("ks-ocr: vision failed: \(error)", 4)
}

var lines: [[String: Any]] = []
for obs in req.results ?? [] {
    guard let top = obs.topCandidates(1).first else { continue }
    let b = obs.boundingBox  // normalised, bottom-left origin
    lines.append([
        "text": top.string,
        "conf": (Double(top.confidence) * 100).rounded() / 100,
        "x": Int(b.minX * Double(W)),
        "y": Int((1 - b.maxY) * Double(H)),
        "w": Int(b.width * Double(W)),
        "h": Int(b.height * Double(H)),
    ])
}
lines.sort { ($0["y"] as! Int, $0["x"] as! Int) < ($1["y"] as! Int, $1["x"] as! Int) }

func contains(_ needle: String) -> Bool {
    lines.contains { ($0["text"] as! String).localizedCaseInsensitiveContains(needle) }
}
if let f = find {
    let hit = contains(f)
    print(hit ? "found: \(f)" : "not found: \(f)")
    exit(hit ? 0 : 1)
}
if let a = absent {
    let hit = contains(a)
    print(hit ? "present: \(a)" : "absent: \(a)")
    exit(hit ? 1 : 0)
}

let out: [String: Any] = [
    "file": path, "width": W, "height": H,
    "blank": blank, "dominantColorShare": (dominant * 100).rounded() / 100,
    "lineCount": lines.count, "lines": lines,
]
let data = try! JSONSerialization.data(withJSONObject: out, options: [.sortedKeys])
print(String(data: data, encoding: .utf8)!)
