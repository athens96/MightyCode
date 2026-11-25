import Foundation
import CoreGraphics
import ImageIO
import UniformTypeIdentifiers

enum IconError: Error { case invalidSource, renderFailed, encodeFailed }
guard CommandLine.arguments.count == 4 else {
    fatalError("Usage: package-icons.swift source.png output.iconset output.ico")
}
let sourceURL = URL(fileURLWithPath: CommandLine.arguments[1])
let iconsetURL = URL(fileURLWithPath: CommandLine.arguments[2], isDirectory: true)
let icoURL = URL(fileURLWithPath: CommandLine.arguments[3])
guard let source = CGImageSourceCreateWithURL(sourceURL as CFURL, nil),
      CGImageSourceGetType(source) as String? == UTType.png.identifier,
      let image = CGImageSourceCreateImageAtIndex(source, 0, nil),
      image.width == image.height, image.width >= 1024 else {
    throw IconError.invalidSource
}

// Continuous-curvature squircle (superellipse, n = 4.85) matching Apple's macOS icon silhouette.
func squirclePath(in rect: CGRect) -> CGPath {
    let path = CGMutablePath()
    let cx = rect.midX
    let cy = rect.midY
    let rx = rect.width / 2.0
    let ry = rect.height / 2.0
    let exponent = 2.0 / 4.85
    let steps = 256
    for i in 0..<steps {
        let theta = (Double(i) / Double(steps)) * 2.0 * Double.pi
        let c = cos(theta)
        let s = sin(theta)
        let x = cx + rx * CGFloat(copysign(pow(abs(c), exponent), c))
        let y = cy + ry * CGFloat(copysign(pow(abs(s), exponent), s))
        if i == 0 {
            path.move(to: CGPoint(x: x, y: y))
        } else {
            path.addLine(to: CGPoint(x: x, y: y))
        }
    }
    path.closeSubpath()
    return path
}

func png(size: Int, macOSPlate: Bool) throws -> Data {
    guard let color = CGColorSpace(name: CGColorSpace.sRGB),
          let context = CGContext(data: nil, width: size, height: size,
                                  bitsPerComponent: 8, bytesPerRow: size * 4, space: color,
                                  bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else {
        throw IconError.renderFailed
    }
    context.interpolationQuality = .high
    let side = CGFloat(size)
    // macOS HIG uses an 824x824 squircle inside a 1024x1024 canvas (100px margin) with a subtle
    // drop shadow for macOS 14/15; macOS 26+ recognizes this silhouette and applies its Liquid Glass
    // rim edge-to-edge without shrinking the icon into a legacy gray container.
    // Windows does not apply an OS squircle mask, so fill the full tile with the squircle directly.
    let inset = macOSPlate ? side * (100.0 / 1024.0) : 0.0
    let tile = CGRect(x: inset, y: inset, width: side - inset * 2.0, height: side - inset * 2.0)
    let mask = squirclePath(in: tile)
    if macOSPlate && size >= 64 {
        context.saveGState()
        context.setShadow(
            offset: CGSize(width: 0, height: -side * 0.012),
            blur: side * 0.024,
            color: CGColor(colorSpace: color, components: [0, 0, 0, 0.28])
        )
        context.addPath(mask)
        context.setFillColor(red: 0.016, green: 0.412, blue: 0.725, alpha: 1.0)
        context.fillPath()
        context.restoreGState()
    }
    context.saveGState()
    context.addPath(mask)
    context.clip()
    context.draw(image, in: tile)
    context.restoreGState()
    guard let resized = context.makeImage() else { throw IconError.renderFailed }
    let data = NSMutableData()
    guard let destination = CGImageDestinationCreateWithData(data, UTType.png.identifier as CFString, 1, nil) else {
        throw IconError.encodeFailed
    }
    CGImageDestinationAddImage(destination, resized, nil)
    guard CGImageDestinationFinalize(destination) else { throw IconError.encodeFailed }
    return data as Data
}

for points in [16, 32, 128, 256, 512] {
    for scale in [1, 2] {
        let suffix = scale == 2 ? "@2x" : ""
        try png(size: points * scale, macOSPlate: true).write(to: iconsetURL.appendingPathComponent("icon_\(points)x\(points)\(suffix).png"))
    }
}

// Windows 10/11 accept PNG-compressed ICO entries, including their alpha channel.
let sizes = [16, 24, 32, 48, 64, 128, 256]
let entries = try sizes.map { try png(size: $0, macOSPlate: false) }
var ico = Data()
func append16(_ value: UInt16) { ico.append(contentsOf: [UInt8(value & 255), UInt8(value >> 8)]) }
func append32(_ value: UInt32) {
    ico.append(contentsOf: [UInt8(value & 255), UInt8((value >> 8) & 255), UInt8((value >> 16) & 255), UInt8(value >> 24)])
}
append16(0); append16(1); append16(UInt16(sizes.count))
var offset = UInt32(6 + sizes.count * 16)
for (size, entry) in zip(sizes, entries) {
    let dimension = size == 256 ? UInt8(0) : UInt8(size)
    ico.append(contentsOf: [dimension, dimension, 0, 0])
    append16(1); append16(32); append32(UInt32(entry.count)); append32(offset)
    offset += UInt32(entry.count)
}
for entry in entries { ico.append(entry) }
try ico.write(to: icoURL, options: .atomic)
