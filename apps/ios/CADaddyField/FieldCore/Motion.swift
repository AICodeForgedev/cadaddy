// CADaddy Field — motion maths for the one continuous camera: a damped spring (SwiftUI's response / damping
// parameters) and the floor-rail "lift apart" layout. Foundation + simd only, so the tests can check them.

import Foundation
import simd

/// A damped spring with SwiftUI's parameters: `response` is the period of the undamped oscillation in seconds,
/// `dampingFraction` 1 is critical. The design uses response 0.55, damping 0.85.
public struct Spring: Equatable, Sendable {
    public var response: Double
    public var dampingFraction: Double
    public init(response: Double = 0.55, dampingFraction: Double = 0.85) {
        self.response = max(response, 0.01); self.dampingFraction = max(dampingFraction, 0)
    }

    public static let design = Spring()

    var stiffness: Double { let w = 2 * Double.pi / response; return w * w }
    var damping: Double { 4 * Double.pi * dampingFraction / response }

    /// Advances one value. Sub-steps keep it stable on long frames (a hitch never makes it explode).
    public func step<V: SIMD>(_ value: inout V, _ velocity: inout V, toward target: V, dt: Double) where V.Scalar == Float {
        guard dt > 0, dt.isFinite else { return }
        let n = max(1, min(Int(ceil(dt / (1.0 / 240.0))), 64))
        let h = Float(dt / Double(n))
        let k = Float(stiffness), c = Float(damping)
        for _ in 0..<n {
            let a = (target - value) * k - velocity * c
            velocity += a * h
            value += velocity * h
        }
    }

    public func step(_ value: inout Float, _ velocity: inout Float, toward target: Float, dt: Double) {
        var v = SIMD2<Float>(value, 0), vel = SIMD2<Float>(velocity, 0)
        step(&v, &vel, toward: SIMD2(target, 0), dt: dt)
        value = v.x; velocity = vel.x
    }

    /// Seconds until a unit step settles within `tolerance` and stays there (for the "never longer than ~0.7 s" check).
    public func settleTime(tolerance: Float = 0.02, limit: Double = 5) -> Double {
        var x: Float = 0, v: Float = 0
        var t = 0.0, lastOut = 0.0
        let dt = 1.0 / 120.0
        while t < limit {
            step(&x, &v, toward: 1, dt: dt)
            t += dt
            if abs(1 - x) > tolerance || abs(v) > tolerance * 4 { lastOut = t }
        }
        return lastOut
    }
}

/// The floor rail: dragging the thumb down the rail lifts the floors above it apart.
public enum FloorLift {
    /// Vertical offset of each level (index 0 = Site … last = Roof). `focus` is the fractional level under the thumb
    /// (nil = not lifting). Levels above the focus rise by `gap`; while lifting every level also separates by `spread`.
    public static func offsets(levels: Int, focus: Float?, gap: Float, spread: Float) -> [Float] {
        guard levels > 0 else { return [] }
        guard let f = focus, f.isFinite else { return Array(repeating: 0, count: levels) }
        return (0..<levels).map { i in
            let above = smoothstep(f + 0.15, f + 1.0, Float(i))
            return Float(i) * spread + above * gap
        }
    }

    /// The thumb's fractional level for a position on the rail. `t` 0 = top of the rail (Roof), 1 = bottom (Site).
    public static func focus(railFraction t: Float, levels: Int) -> Float {
        let c = min(max(t.isFinite ? t : 0, 0), 1)
        return Float(max(levels - 1, 0)) * (1 - c)
    }

    /// The level a detent snaps to.
    public static func detent(_ focus: Float, levels: Int) -> Int { min(max(Int(focus.rounded()), 0), max(levels - 1, 0)) }

    static func smoothstep(_ a: Float, _ b: Float, _ x: Float) -> Float {
        guard b > a else { return x >= b ? 1 : 0 }
        let t = min(max((x - a) / (b - a), 0), 1)
        return t * t * (3 - 2 * t)
    }
}

/// The reveal sweep: how solid each exterior bay is while the reveal front crosses the building.
public enum RevealSweep {
    /// Opacity of the solid exterior at `x` for sweep progress `p` (0 = all solid, 1 = all ghost).
    /// A soft band of `band` metres goes from solid to clear, so the sweep reads as glass, not a hard cut.
    public static func solidOpacity(x: Float, minX: Float, maxX: Float, progress p: Float, band: Float) -> Float {
        let span = max(maxX - minX, 0.001)
        let front = minX - band + (span + 2 * band) * min(max(p, 0), 1)
        // Bays behind the front (lower x than front) are clear; ahead of it, solid.
        return FloorLift.smoothstep(front - band, front + band, x)
    }
}
