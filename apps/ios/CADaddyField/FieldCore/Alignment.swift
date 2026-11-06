// CADaddy Field — placing the model in the real room.
//
// The model frame is the export frame: metres, Y up, the anchor at the origin, the finished floor at y = 0.
// Alignment is a rotation about vertical (yaw) plus a translation; the floor height comes from ARKit plane
// detection, so only the horizontal fit is solved from the user's picks.

import Foundation
import simd

/// world = T(translation) · Ry(yaw) · model
public struct YawAlignment: Equatable, Codable, Sendable {
    public var yaw: Float
    public var translation: SIMD3<Float>

    public init(yaw: Float = 0, translation: SIMD3<Float> = .zero) { self.yaw = yaw; self.translation = translation }

    public var matrix: simd_float4x4 {
        var m = simd_float4x4(simd_quatf(angle: yaw, axis: SIMD3(0, 1, 0)))
        m.columns.3 = SIMD4(translation, 1)
        return m
    }

    public func apply(_ p: SIMD3<Float>) -> SIMD3<Float> {
        let w = matrix * SIMD4(p, 1); return SIMD3(w.x, w.y, w.z)
    }

    /// World point back into the model frame.
    public func modelPoint(fromWorld p: SIMD3<Float>) -> SIMD3<Float> {
        let w = matrix.inverse * SIMD4(p, 1); return SIMD3(w.x, w.y, w.z)
    }

    /// Recovers yaw + translation from a world-from-model matrix (e.g. a restored ARAnchor). Any tilt is dropped.
    public init(matrix m: simd_float4x4) {
        let x = SIMD3(m.columns.0.x, m.columns.0.y, m.columns.0.z)
        yaw = atan2(-x.z, x.x)
        translation = SIMD3(m.columns.3.x, m.columns.3.y, m.columns.3.z)
    }

    public static let nudgeStep: Float = Float(Units.metersPerInch / 8)          // 1/8"
    public static let rotateStep: Float = Float(0.1 * Double.pi / 180)          // 0.1°

    /// Moves the model by `steps` nudge steps along its own X/Z axes (as rotated) and world Y.
    public mutating func nudge(x: Int = 0, y: Int = 0, z: Int = 0) {
        let s = Self.nudgeStep
        let ax = SIMD3<Float>(cos(yaw), 0, -sin(yaw))
        let az = SIMD3<Float>(sin(yaw), 0, cos(yaw))
        translation += ax * Float(x) * s + SIMD3(0, Float(y) * s, 0) + az * Float(z) * s
    }

    /// Rotates by `steps` × 0.1° about the vertical axis through the model's anchor (the origin).
    public mutating func rotate(steps: Int) {
        yaw = Self.normalize(yaw + Float(steps) * Self.rotateStep)
    }

    static func normalize(_ a: Float) -> Float {
        var x = a.truncatingRemainder(dividingBy: 2 * .pi)
        if x > .pi { x -= 2 * .pi }
        if x <= -.pi { x += 2 * .pi }
        return x
    }
}

public struct AlignmentFit: Equatable, Sendable {
    public var alignment: YawAlignment
    /// Horizontal distance between each transformed model pick and its real pick, metres.
    public var residuals: [Double]
    public var rms: Double { residuals.isEmpty ? 0 : (residuals.reduce(0) { $0 + $1 * $1 } / Double(residuals.count)).squareRoot() }
    public var maxResidual: Double { residuals.max() ?? 0 }
    /// Real span ÷ model span (1 = picks agree on distance).
    public var scaleRatio: Double
}

public enum AlignmentSolver {
    /// Picks closer together than this (in the model or in the room) are refused: the rotation is unreliable.
    public static let minSeparation: Double = 0.3
    /// A fit whose picks disagree on distance by more than this fraction is refused as a mis-pick.
    public static let maxScaleError: Double = 0.10

    /// Two-point (or more) fit: rotation about vertical + horizontal translation, least squares.
    /// `floorY` is the detected real floor height; the model's floor (y = 0) lands on it.
    public static func fit(model: [SIMD3<Float>], real: [SIMD3<Float>], floorY: Float) throws -> AlignmentFit {
        guard model.count == real.count else { throw FieldError("Each model point needs one real point.") }
        guard model.count >= 2 else { throw FieldError("Pick at least two points.") }
        guard model.count <= 64 else { throw FieldError("Too many points.") }
        guard floorY.isFinite else { throw FieldError("The floor has not been found yet.") }
        let a = model.map { SIMD2<Double>(Double($0.x), Double($0.z)) }
        let b = real.map { SIMD2<Double>(Double($0.x), Double($0.z)) }
        guard (a + b).allSatisfy({ $0.x.isFinite && $0.y.isFinite }) else { throw FieldError("A picked point is not valid.") }
        let n = Double(a.count)
        let ca = a.reduce(.zero, +) / n
        let cb = b.reduce(.zero, +) / n
        let spreadA = a.map { simd_length($0 - ca) }.max() ?? 0
        let spreadB = b.map { simd_length($0 - cb) }.max() ?? 0
        guard spreadA * 2 >= minSeparation else { throw FieldError("The model points are too close together. Pick points at least 1 ft apart, farther is better.") }
        guard spreadB * 2 >= minSeparation else { throw FieldError("The real points are too close together. Tap points at least 1 ft apart, farther is better.") }
        // Minimise Σ|R(a-ca) - (b-cb)|² for R = rotation by yaw in the glTF/RealityKit convention:
        // Ry(θ)·(x, z) = (x cosθ + z sinθ, -x sinθ + z cosθ).
        var sCos = 0.0, sSin = 0.0, la = 0.0, lb = 0.0
        for (p, q) in zip(a, b) {
            let u = p - ca, v = q - cb
            sCos += u.x * v.x + u.y * v.y
            sSin += u.y * v.x - u.x * v.y
            la += simd_length_squared(u); lb += simd_length_squared(v)
        }
        let theta = atan2(sSin, sCos)
        let c = cos(theta), s = sin(theta)
        let rot: (SIMD2<Double>) -> SIMD2<Double> = { SIMD2($0.x * c + $0.y * s, -$0.x * s + $0.y * c) }
        let t = cb - rot(ca)
        let residuals = zip(a, b).map { simd_length(rot($0) + t - $1) }
        let ratio = la > 0 ? (lb / la).squareRoot() : 1
        guard abs(ratio - 1) <= maxScaleError else {
            throw FieldError(String(format: "The picks disagree on distance by %.0f%%. Check that each real point matches its model point.", abs(ratio - 1) * 100))
        }
        let alignment = YawAlignment(yaw: Float(theta), translation: SIMD3(Float(t.x), floorY, Float(t.y)))
        return AlignmentFit(alignment: alignment, residuals: residuals, scaleRatio: ratio)
    }

    /// Quick placement: the anchor (model origin) goes on the tapped floor point; the user then turns it.
    public static func quick(anchorAt p: SIMD3<Float>, floorY: Float?, yaw: Float) -> YawAlignment {
        YawAlignment(yaw: yaw, translation: SIMD3(p.x, floorY ?? p.y, p.z))
    }
}
