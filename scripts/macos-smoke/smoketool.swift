// Helpers of the macOS smoke test (scripts/macos-smoke): the runner has swiftc but no ImageMagick, wmctrl or xdotool.
//   smoketool windows <owner>            list windows of processes whose name contains <owner>; exit 0 when one is on screen
//   smoketool diff <a.png> <b.png>       number of pixels that differ
//   smoketool colours <a.png>            number of distinct colours (counted up to 100000)
//   smoketool caption <in> <out> <text>  <in> with a caption strip above it
import AppKit
import CoreGraphics
import Foundation

func fail(_ message: String, _ code: Int32) -> Never {
  FileHandle.standardError.write((message + "\n").data(using: .utf8)!)
  exit(code)
}

func pixels(_ path: String) -> (UnsafeMutablePointer<UInt8>, Int, Int)? {
  guard let source = CGImageSourceCreateWithURL(URL(fileURLWithPath: path) as CFURL, nil),
    let image = CGImageSourceCreateImageAtIndex(source, 0, nil)
  else { return nil }
  let w = image.width, h = image.height
  let buffer = UnsafeMutablePointer<UInt8>.allocate(capacity: w * h * 4)
  guard
    let context = CGContext(
      data: buffer, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w * 4,
      space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
  else { return nil }
  context.draw(image, in: CGRect(x: 0, y: 0, width: w, height: h))
  return (buffer, w, h)
}

func windows(_ owner: String) -> Int32 {
  let needle = owner.lowercased()
  let list = CGWindowListCopyWindowInfo([.optionAll], kCGNullWindowID) as? [[String: Any]] ?? []
  var onScreen = false
  for w in list {
    let name = (w[kCGWindowOwnerName as String] as? String) ?? ""
    guard name.lowercased().contains(needle) else { continue }
    let number = (w[kCGWindowNumber as String] as? Int) ?? -1
    let title = (w[kCGWindowName as String] as? String) ?? ""
    let layer = (w[kCGWindowLayer as String] as? Int) ?? -1
    let visible = (w[kCGWindowIsOnscreen as String] as? Bool) ?? false
    let b = (w[kCGWindowBounds as String] as? [String: Any]) ?? [:]
    print("\(number)\t\(name)\t\(title)\tlayer=\(layer)\tonscreen=\(visible)\tbounds=\(b["X"] ?? "?"),\(b["Y"] ?? "?"),\(b["Width"] ?? "?"),\(b["Height"] ?? "?")")
    if layer == 0 && visible { onScreen = true }
  }
  return onScreen ? 0 : 1
}

func diff(_ a: String, _ b: String) -> Int32 {
  guard let (pa, wa, ha) = pixels(a), let (pb, wb, hb) = pixels(b) else { fail("smoketool: cannot read an image", 2) }
  guard wa == wb && ha == hb else { fail("smoketool: sizes differ \(wa)x\(ha) vs \(wb)x\(hb)", 2) }
  var changed = 0
  for i in 0..<(wa * ha) where memcmp(pa + i * 4, pb + i * 4, 4) != 0 { changed += 1 }
  print(changed)
  return 0
}

func colours(_ path: String) -> Int32 {
  guard let (p, w, h) = pixels(path) else { fail("smoketool: cannot read the image", 2) }
  var seen = Set<UInt32>()
  for i in 0..<(w * h) {
    seen.insert(UnsafeRawPointer(p + i * 4).loadUnaligned(as: UInt32.self))
    if seen.count >= 100_000 { break }
  }
  print(seen.count)
  return 0
}

func caption(_ input: String, _ output: String, _ label: String) -> Int32 {
  guard let source = CGImageSourceCreateWithURL(URL(fileURLWithPath: input) as CFURL, nil),
    let image = CGImageSourceCreateImageAtIndex(source, 0, nil)
  else { fail("smoketool: cannot read \(input)", 2) }
  let w = image.width, h = image.height, strip = 28
  guard
    let context = CGContext(
      data: nil, width: w, height: h + strip, bitsPerComponent: 8, bytesPerRow: 0,
      space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
  else { fail("smoketool: no drawing context", 2) }
  context.setFillColor(CGColor(red: 0.12, green: 0.14, blue: 0.19, alpha: 1))
  context.fill(CGRect(x: 0, y: h, width: w, height: strip))
  context.draw(image, in: CGRect(x: 0, y: 0, width: w, height: h))
  let attributes: [NSAttributedString.Key: Any] = [
    .font: NSFont.monospacedSystemFont(ofSize: 15, weight: .regular),
    .foregroundColor: NSColor(red: 0.90, green: 0.91, blue: 0.94, alpha: 1),
  ]
  let line = CTLineCreateWithAttributedString(NSAttributedString(string: label, attributes: attributes) as CFAttributedString)
  context.setFillColor(CGColor(red: 0.90, green: 0.91, blue: 0.94, alpha: 1))
  context.textPosition = CGPoint(x: 10, y: CGFloat(h) + 8)
  CTLineDraw(line, context)
  guard let result = context.makeImage(),
    let destination = CGImageDestinationCreateWithURL(URL(fileURLWithPath: output) as CFURL, "public.png" as CFString, 1, nil)
  else { fail("smoketool: cannot write \(output)", 2) }
  CGImageDestinationAddImage(destination, result, nil)
  return CGImageDestinationFinalize(destination) ? 0 : 2
}

let args = CommandLine.arguments
switch args.count > 1 ? args[1] : "" {
case "windows" where args.count == 3: exit(windows(args[2]))
case "diff" where args.count == 4: exit(diff(args[2], args[3]))
case "colours" where args.count == 3: exit(colours(args[2]))
case "caption" where args.count == 5: exit(caption(args[2], args[3], args[4]))
default: fail("usage: smoketool windows <owner> | diff <a> <b> | colours <png> | caption <in> <out> <text>", 64)
}
