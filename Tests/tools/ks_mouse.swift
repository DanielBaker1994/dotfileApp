// ks_mouse: paced mouse events for VM UI checks (global top-left points).
// Build: swiftc -O -target arm64-apple-macos14 Tests/tools/ks_mouse.swift -o ks-mouse
//   move X Y | click X Y | dclick X Y | rclick X Y | drag X1 Y1 X2 Y2 [--cmd] | flags | release
import Foundation
import CoreGraphics
let a = CommandLine.arguments
let src = CGEventSource(stateID: .hidSystemState)
func post(_ t: CGEventType, _ p: CGPoint, _ flags: CGEventFlags = [], click: Int64 = 1) {
  let e = CGEvent(mouseEventSource: src, mouseType: t, mouseCursorPosition: p, mouseButton: .left)!
  e.setIntegerValueField(.mouseEventClickState, value: click)
  e.flags = flags
  e.post(tap: .cghidEventTap)
}
func key(_ code: CGKeyCode, _ down: Bool) {
  CGEvent(keyboardEventSource: src, virtualKey: code, keyDown: down)!.post(tap: .cghidEventTap)
}
func ms(_ n: UInt32) { usleep(n * 1000) }
switch a.count > 1 ? a[1] : "" {
case "move":
  let p = CGPoint(x: Double(a[2])!, y: Double(a[3])!)
  let cur = CGEvent(source: nil)!.location
  for i in 1...12 { let t = Double(i) / 12
    post(.mouseMoved, CGPoint(x: cur.x + (p.x - cur.x) * t, y: cur.y + (p.y - cur.y) * t)); ms(15) }
case "drag":
  let p1 = CGPoint(x: Double(a[2])!, y: Double(a[3])!), p2 = CGPoint(x: Double(a[4])!, y: Double(a[5])!)
  let cmd = a.contains("--cmd")
  let f: CGEventFlags = cmd ? .maskCommand : []
  if cmd { key(55, true); ms(30) }
  post(.mouseMoved, p1); ms(40)
  post(.leftMouseDown, p1, f); ms(60)
  for i in 1...25 { let t = Double(i) / 25
    post(.leftMouseDragged, CGPoint(x: p1.x + (p2.x - p1.x) * t, y: p1.y + (p2.y - p1.y) * t), f); ms(15) }
  ms(80); post(.leftMouseUp, p2, f); ms(30)
  if cmd { key(55, false) }
case "click", "dclick":
  let p = CGPoint(x: Double(a[2])!, y: Double(a[3])!)
  post(.mouseMoved, p); ms(40)
  for n in 1...(a[1] == "dclick" ? 2 : 1) {
    post(.leftMouseDown, p, click: Int64(n)); ms(30); post(.leftMouseUp, p, click: Int64(n)); ms(60)
  }
case "rclick":
  let p = CGPoint(x: Double(a[2])!, y: Double(a[3])!)
  post(.mouseMoved, p); ms(40)
  for t in [CGEventType.rightMouseDown, .rightMouseUp] {
    let e = CGEvent(mouseEventSource: src, mouseType: t, mouseCursorPosition: p, mouseButton: .right)!
    e.setIntegerValueField(.mouseEventClickState, value: 1); e.post(tap: .cghidEventTap); ms(40)
  }
case "flags":
  print(String(CGEventSource.flagsState(.hidSystemState).rawValue, radix: 16))
case "release":
  for c: CGKeyCode in [55, 54, 56, 60, 58, 61, 59, 62, 63] { key(c, false) }
  print(String(CGEventSource.flagsState(.hidSystemState).rawValue, radix: 16))
default:
  print("usage: ks_mouse move X Y | click X Y | dclick X Y | rclick X Y | drag X1 Y1 X2 Y2 [--cmd] | flags | release"); exit(2)
}
