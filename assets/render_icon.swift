import AppKit
import CoreGraphics
import Foundation

func renderIconPNG(size: Int, to url: URL) throws {
    guard let rep = NSBitmapImageRep(
        bitmapDataPlanes: nil,
        pixelsWide: size,
        pixelsHigh: size,
        bitsPerSample: 8,
        samplesPerPixel: 4,
        hasAlpha: true,
        isPlanar: false,
        colorSpaceName: .deviceRGB,
        bytesPerRow: size * 4,
        bitsPerPixel: 32
    ) else {
        throw NSError(domain: "CodaIcon", code: 1, userInfo: [NSLocalizedDescriptionKey: "Failed to create bitmap"])
    }

    rep.size = NSSize(width: size, height: size)
    guard let nsCtx = NSGraphicsContext(bitmapImageRep: rep) else {
        throw NSError(domain: "CodaIcon", code: 2, userInfo: [NSLocalizedDescriptionKey: "Failed to create graphics context"])
    }

    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = nsCtx
    let ctx = nsCtx.cgContext
    ctx.setAllowsAntialiasing(true)
    ctx.setShouldAntialias(true)

    let s = CGFloat(size) / 64.0
    ctx.scaleBy(x: s, y: s)

    // Flip to top-left SVG coordinates (0..64)
    ctx.translateBy(x: 0, y: 64.0)
    ctx.scaleBy(x: 1.0, y: -1.0)

    let colorSpace = CGColorSpaceCreateDeviceRGB()

    // 1. Squircle background (#171920 -> #0b0c10)
    let bgRect = CGRect(x: 2, y: 2, width: 60, height: 60)
    let bgPath = CGPath(roundedRect: bgRect, cornerWidth: 15, cornerHeight: 15, transform: nil)
    ctx.saveGState()
    ctx.addPath(bgPath)
    ctx.clip()
    let bgColors = [
        CGColor(red: 0x17 / 255.0, green: 0x19 / 255.0, blue: 0x20 / 255.0, alpha: 1.0),
        CGColor(red: 0x0b / 255.0, green: 0x0c / 255.0, blue: 0x10 / 255.0, alpha: 1.0),
    ] as CFArray
    if let grad = CGGradient(colorsSpace: colorSpace, colors: bgColors, locations: [0.0, 1.0]) {
        ctx.drawLinearGradient(grad, start: CGPoint(x: 32, y: 2), end: CGPoint(x: 32, y: 62), options: [])
    }

    // Subtle warm radial glow behind the C arc
    let glowColors = [
        CGColor(red: 0xf5 / 255.0, green: 0x9e / 255.0, blue: 0x0b / 255.0, alpha: 0.14),
        CGColor(red: 0xf5 / 255.0, green: 0x9e / 255.0, blue: 0x0b / 255.0, alpha: 0.0),
    ] as CFArray
    if let glow = CGGradient(colorsSpace: colorSpace, colors: glowColors, locations: [0.0, 1.0]) {
        ctx.drawRadialGradient(glow, startCenter: CGPoint(x: 32, y: 32), startRadius: 2, endCenter: CGPoint(x: 32, y: 32), endRadius: 28, options: [])
    }
    ctx.restoreGState()

    // Border (#2a2d37, 1.5px)
    ctx.saveGState()
    ctx.addPath(bgPath)
    ctx.setStrokeColor(CGColor(red: 0x2a / 255.0, green: 0x2d / 255.0, blue: 0x37 / 255.0, alpha: 1.0))
    ctx.setLineWidth(1.5)
    ctx.strokePath()
    ctx.restoreGState()

    // 2. Open C arc (~265 deg) with amber -> coral gradient
    let arcPath = CGMutablePath()
    arcPath.addArc(
        center: CGPoint(x: 32, y: 32),
        radius: 17.5,
        startAngle: CGFloat.pi / 4.0,
        endAngle: -CGFloat.pi * 0.375,
        clockwise: false
    )
    ctx.saveGState()
    ctx.addPath(arcPath)
    ctx.setLineWidth(5.5)
    ctx.setLineCap(.round)
    ctx.replacePathWithStrokedPath()
    ctx.clip()
    let arcColors = [
        CGColor(red: 0xf5 / 255.0, green: 0x9e / 255.0, blue: 0x0b / 255.0, alpha: 1.0),
        CGColor(red: 0xe0 / 255.0, green: 0x6c / 255.0, blue: 0x50 / 255.0, alpha: 1.0),
    ] as CFArray
    if let arcGrad = CGGradient(colorsSpace: colorSpace, colors: arcColors, locations: [0.0, 1.0]) {
        ctx.drawLinearGradient(arcGrad, start: CGPoint(x: 14, y: 14), end: CGPoint(x: 48, y: 50), options: [])
    }
    ctx.restoreGState()

    // 3. Prompt chevron ❯ (#f4f4f5)
    let chevPath = CGMutablePath()
    chevPath.move(to: CGPoint(x: 25.5, y: 24.5))
    chevPath.addLine(to: CGPoint(x: 34.0, y: 32.0))
    chevPath.addLine(to: CGPoint(x: 25.5, y: 39.5))
    ctx.saveGState()
    ctx.addPath(chevPath)
    ctx.setStrokeColor(CGColor(red: 0xf4 / 255.0, green: 0xf4 / 255.0, blue: 0xf5 / 255.0, alpha: 1.0))
    ctx.setLineWidth(4.5)
    ctx.setLineCap(.round)
    ctx.setLineJoin(.round)
    ctx.strokePath()
    ctx.restoreGState()

    // 4. Live emerald mesh node dot (#22c55e) with subtle outer halo
    ctx.saveGState()
    let nodeHalo = CGRect(x: 46.0 - 5.6, y: 19.0 - 5.6, width: 11.2, height: 11.2)
    ctx.setFillColor(CGColor(red: 0x22 / 255.0, green: 0xc5 / 255.0, blue: 0x5e / 255.0, alpha: 0.25))
    ctx.fillEllipse(in: nodeHalo)
    let nodeRect = CGRect(x: 46.0 - 4.2, y: 19.0 - 4.2, width: 8.4, height: 8.4)
    ctx.setFillColor(CGColor(red: 0x22 / 255.0, green: 0xc5 / 255.0, blue: 0x5e / 255.0, alpha: 1.0))
    ctx.fillEllipse(in: nodeRect)
    ctx.restoreGState()

    NSGraphicsContext.restoreGraphicsState()

    guard let pngData = rep.representation(using: .png, properties: [:]) else {
        throw NSError(domain: "CodaIcon", code: 3, userInfo: [NSLocalizedDescriptionKey: "Failed to encode PNG"])
    }
    try pngData.write(to: url)
}

func main() throws {
    let args = CommandLine.arguments
    let fm = FileManager.default
    let scriptDir = URL(fileURLWithPath: args[0]).deletingLastPathComponent()

    let icon512URL = scriptDir.appendingPathComponent("icon-512.png")
    try renderIconPNG(size: 512, to: icon512URL)

    if let icnsIdx = args.firstIndex(of: "--icns"), icnsIdx + 1 < args.count {
        let icnsURL = URL(fileURLWithPath: args[icnsIdx + 1])
        let tmpDir = fm.temporaryDirectory.appendingPathComponent("coda-iconset-\(UUID().uuidString)")
        let iconsetURL = tmpDir.appendingPathComponent("AppIcon.iconset")
        try fm.createDirectory(at: iconsetURL, withIntermediateDirectories: true)
        defer { try? fm.removeItem(at: tmpDir) }

        let specs: [(String, Int)] = [
            ("icon_16x16.png", 16),
            ("icon_16x16@2x.png", 32),
            ("icon_32x32.png", 32),
            ("icon_32x32@2x.png", 64),
            ("icon_128x128.png", 128),
            ("icon_128x128@2x.png", 256),
            ("icon_256x256.png", 256),
            ("icon_256x256@2x.png", 512),
            ("icon_512x512.png", 512),
            ("icon_512x512@2x.png", 1024),
        ]

        for (filename, pxSize) in specs {
            try renderIconPNG(size: pxSize, to: iconsetURL.appendingPathComponent(filename))
        }

        let proc = Process()
        proc.executableURL = URL(fileURLWithPath: "/usr/bin/iconutil")
        proc.arguments = ["-c", "icns", iconsetURL.path, "-o", icnsURL.path]
        try proc.run()
        proc.waitUntilExit()
        if proc.terminationStatus != 0 {
            throw NSError(domain: "CodaIcon", code: Int(proc.terminationStatus), userInfo: [NSLocalizedDescriptionKey: "iconutil failed"])
        }
    }
}

try main()
