// Samples what is on screen (no Screen Recording permission needed): the
// frontmost app, and every on-screen window front to back with its owner and
// bounds. Prints one JSON line per change. Usage: swift windows.swift <ms>
import AppKit
import CoreGraphics
let total = Double(CommandLine.arguments.dropFirst().first ?? "60000") ?? 60000
let start = Date()
var last = ""
while Date().timeIntervalSince(start) * 1000 < total {
  let front = NSWorkspace.shared.frontmostApplication?.localizedName ?? "?"
  let list = (CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]]) ?? []
  var wins: [String] = []
  for w in list {
    let layer = w[kCGWindowLayer as String] as? Int ?? -1
    let owner = w[kCGWindowOwnerName as String] as? String ?? "?"
    // Ordinary windows, plus anything Chrome for Testing or the Dock (a
    // minimize animation) puts on screen at another level.
    guard layer == 0 || owner.contains("Chrome for Testing") || owner == "Dock" else { continue }
    let b = w[kCGWindowBounds as String] as? [String: Any] ?? [:]
    let id = w[kCGWindowNumber as String] as? Int ?? 0
    wins.append("\(owner)#\(id)L\(layer)@\(b["X"] ?? 0),\(b["Y"] ?? 0),\(b["Width"] ?? 0)x\(b["Height"] ?? 0)")
  }
  let line = "{\"front\":\"\(front)\",\"windows\":[\(wins.map { "\"\($0)\"" }.joined(separator: ","))]}"
  if line != last {
    let ms = Int(Date().timeIntervalSince(start) * 1000)
    print("{\"ms\":\(ms),\"at\":\(Int(Date().timeIntervalSince1970 * 1000)),\"state\":\(line)}")
    fflush(stdout)
    last = line
  }
  usleep(10_000)
}
