// CADaddy Field — merged meshes for the building view. Many boxes (a floor's walls, their edge lines, a bay of
// facade) become one mesh, so a floor is a handful of draw calls instead of hundreds.

import CoreGraphics
import RealityKit
import simd
import UIKit

struct MeshKit {
    private(set) var positions: [SIMD3<Float>] = []
    private(set) var normals: [SIMD3<Float>] = []
    private(set) var indices: [UInt32] = []

    var isEmpty: Bool { indices.isEmpty }

    /// An axis-aligned box.
    mutating func box(center c: SIMD3<Float>, size s: SIMD3<Float>) {
        let h = simd_abs(s) / 2
        guard h.x > 0, h.y > 0, h.z > 0 else { return }
        oriented(center: c, axes: (SIMD3(1, 0, 0), SIMD3(0, 1, 0), SIMD3(0, 0, 1)), half: h)
    }

    /// A box with min/max corners.
    mutating func box(min a: SIMD3<Float>, max b: SIMD3<Float>) { box(center: (a + b) / 2, size: b - a) }

    /// The 12 edges of an axis-aligned box as thin square bars (the ghost's fine lines).
    mutating func edges(center c: SIMD3<Float>, size s: SIMD3<Float>, width w: Float) {
        let h = s / 2
        for sy in [Float(-1), 1] { for sz in [Float(-1), 1] { box(center: c + SIMD3(0, sy * h.y, sz * h.z), size: SIMD3(s.x + w, w, w)) } }
        for sx in [Float(-1), 1] { for sz in [Float(-1), 1] { box(center: c + SIMD3(sx * h.x, 0, sz * h.z), size: SIMD3(w, s.y, w)) } }
        for sx in [Float(-1), 1] { for sy in [Float(-1), 1] { box(center: c + SIMD3(sx * h.x, sy * h.y, 0), size: SIMD3(w, w, s.z + w)) } }
    }

    /// A run through `points`: a rectangular duct or a round pipe, with each segment overlapping the bend.
    mutating func run(_ points: [SIMD3<Float>], profile: RunProfile) {
        guard points.count >= 2 else { return }
        for i in 1..<points.count {
            let a = points[i - 1], b = points[i]
            let d = b - a
            let len = simd_length(d)
            guard len > 1e-4 else { continue }
            let dir = d / len
            let ref: SIMD3<Float> = abs(dir.y) > 0.9 ? SIMD3(1, 0, 0) : SIMD3(0, 1, 0)
            let side = simd_normalize(simd_cross(ref, dir))
            let up = simd_cross(dir, side)
            switch profile {
            case let .rect(w, hgt):
                let ext = max(w, hgt) / 2
                oriented(center: (a + b) / 2, axes: (dir, up, side), half: SIMD3(len / 2 + ext, hgt / 2, w / 2))
            case let .round(r):
                prism(from: a - dir * r * 0.5, to: b + dir * r * 0.5, radius: r, side: side, up: up, sides: r > 0.03 ? 12 : 8)
            }
        }
    }

    /// A flat disc facing up (light glow pools, contact shadows).
    mutating func disc(center c: SIMD3<Float>, radius r: Float, sides: Int = 24) {
        let base = UInt32(positions.count)
        positions.append(c); normals.append(SIMD3(0, 1, 0))
        for i in 0..<sides {
            let t = Float(i) / Float(sides) * 2 * .pi
            positions.append(c + SIMD3(cos(t) * r, 0, sin(t) * r)); normals.append(SIMD3(0, 1, 0))
        }
        for i in 0..<UInt32(sides) {
            indices += [base, base + 1 + (i + 1) % UInt32(sides), base + 1 + i]
        }
    }

    /// A textured quad facing up, with UVs (for the soft shadow).
    static func quad(center c: SIMD3<Float>, size: SIMD2<Float>) -> MeshResource? {
        var d = MeshDescriptor(name: "quad")
        let h = size / 2
        d.positions = MeshBuffer([c + SIMD3(-h.x, 0, -h.y), c + SIMD3(h.x, 0, -h.y), c + SIMD3(h.x, 0, h.y), c + SIMD3(-h.x, 0, h.y)])
        d.normals = MeshBuffer(Array(repeating: SIMD3<Float>(0, 1, 0), count: 4))
        d.textureCoordinates = MeshBuffer([SIMD2<Float>(0, 0), SIMD2(1, 0), SIMD2(1, 1), SIMD2(0, 1)])
        d.primitives = .triangles([0, 2, 1, 0, 3, 2])
        return try? MeshResource.generate(from: [d])
    }

    private mutating func oriented(center c: SIMD3<Float>, axes: (SIMD3<Float>, SIMD3<Float>, SIMD3<Float>), half h: SIMD3<Float>) {
        let (ax, ay, az) = axes
        let faces: [(SIMD3<Float>, SIMD3<Float>, SIMD3<Float>, Float, Float, Float)] = [
            (ax, ay, az, h.x, h.y, h.z), (-ax, az, ay, h.x, h.z, h.y),
            (ay, az, ax, h.y, h.z, h.x), (-ay, ax, az, h.y, h.x, h.z),
            (az, ax, ay, h.z, h.x, h.y), (-az, ay, ax, h.z, h.y, h.x),
        ]
        for (n, u, v, dn, du, dv) in faces {
            let base = UInt32(positions.count)
            let fc = c + n * dn
            let corners = [fc - u * du - v * dv, fc + u * du - v * dv, fc + u * du + v * dv, fc - u * du + v * dv]
            positions += corners
            normals += [n, n, n, n]
            // Wind counter-clockwise seen from outside.
            if simd_dot(simd_cross(u, v), n) > 0 { indices += [base, base + 1, base + 2, base, base + 2, base + 3] } else { indices += [base, base + 2, base + 1, base, base + 3, base + 2] }
        }
    }

    private mutating func prism(from a: SIMD3<Float>, to b: SIMD3<Float>, radius r: Float, side: SIMD3<Float>, up: SIMD3<Float>, sides: Int) {
        let base = UInt32(positions.count)
        for i in 0..<sides {
            let t = Float(i) / Float(sides) * 2 * .pi
            let n = side * cos(t) + up * sin(t)
            positions += [a + n * r, b + n * r]
            normals += [n, n]
        }
        let dir = simd_normalize(b - a)
        for i in 0..<UInt32(sides) {
            let j = (i + 1) % UInt32(sides)
            let a0 = base + i * 2, b0 = a0 + 1, a1 = base + j * 2, b1 = a1 + 1
            if simd_dot(simd_cross(side, up), dir) > 0 { indices += [a0, a1, b1, a0, b1, b0] } else { indices += [a0, b1, a1, a0, b0, b1] }
        }
        // Caps.
        for (p, n) in [(a, -dir), (b, dir)] {
            let cb = UInt32(positions.count)
            positions.append(p); normals.append(n)
            for i in 0..<sides {
                let t = Float(i) / Float(sides) * 2 * .pi
                positions.append(p + (side * cos(t) + up * sin(t)) * r); normals.append(n)
            }
            for i in 0..<UInt32(sides) {
                let j = (i + 1) % UInt32(sides)
                let tri: [UInt32] = [cb, cb + 1 + i, cb + 1 + j]
                let e1 = positions[Int(tri[1])] - p, e2 = positions[Int(tri[2])] - p
                indices += simd_dot(simd_cross(e1, e2), n) > 0 ? tri : [cb, cb + 1 + j, cb + 1 + i]
            }
        }
    }

    func resource(_ name: String) -> MeshResource? {
        guard !indices.isEmpty else { return nil }
        var d = MeshDescriptor(name: name)
        d.positions = MeshBuffer(positions)
        d.normals = MeshBuffer(normals)
        d.primitives = .triangles(indices)
        return try? MeshResource.generate(from: [d])
    }

    /// A soft radial gradient (opaque centre to clear edge) as a texture, for shadows and light glows.
    static func radialTexture(inner: UIColor, outer: UIColor, size: Int = 128) -> TextureResource? {
        let s = CGFloat(size)
        let fmt = UIGraphicsImageRendererFormat()
        fmt.scale = 1
        fmt.opaque = false
        let img = UIGraphicsImageRenderer(size: CGSize(width: s, height: s), format: fmt).image { ctx in
            let colors = [inner.cgColor, inner.withAlphaComponent(inner.cgAlpha * 0.45).cgColor, outer.cgColor] as CFArray
            guard let g = CGGradient(colorsSpace: CGColorSpaceCreateDeviceRGB(), colors: colors, locations: [0, 0.45, 1]) else { return }
            ctx.cgContext.drawRadialGradient(g, startCenter: CGPoint(x: s / 2, y: s / 2), startRadius: 0,
                                             endCenter: CGPoint(x: s / 2, y: s / 2), endRadius: s / 2, options: [])
        }
        guard let cg = img.cgImage else { return nil }
        return try? TextureResource.generate(from: cg, options: .init(semantic: .color))
    }
}

private extension UIColor {
    var cgAlpha: CGFloat { var a: CGFloat = 0; getRed(nil, green: nil, blue: nil, alpha: &a); return a }
}

// MARK: - Shell geometry (walls from rooms)

extension SampleBuilding {
    /// Wall boxes (centre, size) for one level, full storey height or cut at `cut` metres above the floor.
    /// Exterior walls, corridor walls with door gaps, demising walls and the bath partition of each guest room.
    func walls(level i: Int, cut: Float? = nil) -> [(SIMD3<Float>, SIMD3<Float>)] {
        guard let lv = level(i), lv.height > 0 else { return [] }
        let h = min(cut ?? lv.height, lv.height)
        let y = lv.elevation + h / 2
        let lo = footprintMin, hi = footprintMax
        var out: [(SIMD3<Float>, SIMD3<Float>)] = []
        func wallX(_ x0: Float, _ x1: Float, z: Float, t: Float) { if x1 - x0 > 0.05 { out.append((SIMD3((x0 + x1) / 2, y, z), SIMD3(x1 - x0, h, t))) } }
        func wallZ(_ z0: Float, _ z1: Float, x: Float, t: Float) { if z1 - z0 > 0.05 { out.append((SIMD3(x, y, (z0 + z1) / 2), SIMD3(t, h, z1 - z0))) } }
        wallX(lo.x, hi.x, z: lo.y + 0.15, t: 0.3)
        wallX(lo.x, hi.x, z: hi.y - 0.15, t: 0.3)
        wallZ(lo.y, hi.y, x: lo.x + 0.15, t: 0.3)
        wallZ(lo.y, hi.y, x: hi.x - 0.15, t: 0.3)
        var seenX = Set<String>()
        for r in rooms(on: i) where r.name != "Corridor" {
            let south = r.max.y <= footprintMax.y / 2
            let corridorZ = south ? r.max.y : r.min.y
            // Corridor wall with a door near the room's high-x end.
            let doorA = r.max.x - 1.35, doorB = r.max.x - 0.45
            wallX(r.min.x, doorA, z: corridorZ, t: 0.15)
            wallX(doorB, r.max.x, z: corridorZ, t: 0.15)
            for x in [r.min.x, r.max.x] where x > lo.x + 0.1 && x < hi.x - 0.1 {
                let key = "\(x)|\(south)"
                if seenX.insert(key).inserted { wallZ(r.min.y, r.max.y, x: x, t: 0.15) }
            }
            if r.name == "Guest Room" {
                let bz = south ? r.max.y - 2.4 : r.min.y + 2.4
                wallX(r.min.x, r.min.x + 2.4, z: bz, t: 0.1)
                wallZ(min(bz, corridorZ), max(bz, corridorZ), x: r.min.x + 2.4, t: 0.1)
            }
        }
        return out
    }
}
