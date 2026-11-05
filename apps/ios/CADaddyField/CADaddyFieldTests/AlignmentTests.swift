import simd
import XCTest

final class AlignmentTests: XCTestCase {
    let a1 = SIMD3<Float>(0, 0, 0), a2 = SIMD3<Float>(4, 0, -7.6), a3 = SIMD3<Float>(4, 0, 0)

    func testRecoversKnownTransforms() throws {
        for yawDeg in [-179.0, -90, -30, 0, 12.5, 45, 90, 135, 179.9] {
            let truth = YawAlignment(yaw: Float(yawDeg * .pi / 180), translation: SIMD3(1.5, 0.02, -2.25))
            let real = [truth.apply(a1), truth.apply(a2)]
            let fit = try AlignmentSolver.fit(model: [a1, a2], real: real, floorY: 0.02)
            XCTAssertEqual(Double(fit.alignment.yaw), Double(truth.yaw), accuracy: 1e-4, "yaw \(yawDeg)")
            XCTAssertNear(fit.alignment.translation, truth.translation, 1e-4)
            XCTAssertLessThan(fit.rms, 1e-4)
            // A model device lands where the true transform puts it.
            let device = SIMD3<Float>(0.02, 0.4572, -3.5)
            XCTAssertNear(fit.alignment.apply(device), truth.apply(device), 1e-3)
        }
    }

    func testFloorComesFromPlaneDetectionNotTaps() throws {
        let truth = YawAlignment(yaw: 0.3, translation: SIMD3(2, -1.4, 1))
        // Taps land 3 cm above and below the floor (raycast noise); the fit uses the detected floor.
        var real = [truth.apply(a1), truth.apply(a2)]
        real[0].y += 0.03; real[1].y -= 0.03
        let fit = try AlignmentSolver.fit(model: [a1, a2], real: real, floorY: -1.4)
        XCTAssertEqual(fit.alignment.translation.y, -1.4, accuracy: 1e-6)
    }

    func testResidualReportsDisagreementInInches() throws {
        let truth = YawAlignment(yaw: 1.0, translation: SIMD3(0, 0, 0))
        var real = [truth.apply(a1), truth.apply(a2)]
        // Second real point tapped 1" too far along the line between the picks.
        let dir = simd_normalize(real[1] - real[0])
        real[1] += dir * Float(Units.metersPerInch)
        let fit = try AlignmentSolver.fit(model: [a1, a2], real: real, floorY: 0)
        // Least squares splits the 1" error: 1/2" at each pick.
        XCTAssertEqual(Units.inches(fromMeters: fit.residuals[0]), 0.5, accuracy: 0.01)
        XCTAssertEqual(Units.inches(fromMeters: fit.rms), 0.5, accuracy: 0.01)
        XCTAssertEqual(Units.inchesText(fit.rms), "0.50 in")
    }

    func testThreePointFit() throws {
        let truth = YawAlignment(yaw: -2.0, translation: SIMD3(-3, 0, 5))
        let fit = try AlignmentSolver.fit(model: [a1, a2, a3], real: [a1, a2, a3].map(truth.apply), floorY: 0)
        XCTAssertEqual(fit.alignment.yaw, truth.yaw, accuracy: 1e-4)
        XCTAssertNear(fit.alignment.translation, truth.translation, 1e-4)
    }

    func testDegeneratePicksRefused() {
        XCTAssertThrowsFieldError(try AlignmentSolver.fit(model: [a1, a1], real: [a1, a2], floorY: 0), contains: "model points are too close")
        XCTAssertThrowsFieldError(try AlignmentSolver.fit(model: [a1, a2], real: [a1, SIMD3(0.1, 0, 0)], floorY: 0), contains: "real points are too close")
        XCTAssertThrowsFieldError(try AlignmentSolver.fit(model: [a1], real: [a1], floorY: 0), contains: "at least two")
        XCTAssertThrowsFieldError(try AlignmentSolver.fit(model: [a1, a2], real: [a1], floorY: 0))
        XCTAssertThrowsFieldError(try AlignmentSolver.fit(model: [a1, a2], real: [a1, SIMD3(.nan, 0, 0)], floorY: 0), contains: "not valid")
        XCTAssertThrowsFieldError(try AlignmentSolver.fit(model: [a1, a2], real: [a1, a2], floorY: .nan), contains: "floor")
        // Picks that disagree on distance by 20% are a mis-pick, not a fit.
        XCTAssertThrowsFieldError(try AlignmentSolver.fit(model: [a1, a2], real: [a1, a2 * 1.2], floorY: 0), contains: "disagree")
    }

    func testQuickPlacementPutsAnchorOnTappedFloor() {
        let al = AlignmentSolver.quick(anchorAt: SIMD3(1, 0.07, -2), floorY: 0.0, yaw: 0.5)
        XCTAssertNear(al.apply(.zero), SIMD3(1, 0, -2), 1e-6)
        XCTAssertEqual(al.yaw, 0.5)
    }

    func testNudgeAndRotateSteps() {
        var al = YawAlignment(yaw: 0, translation: .zero)
        al.nudge(x: 8)
        XCTAssertNear(al.translation, SIMD3(Float(Units.metersPerInch), 0, 0), 1e-6) // 8 × 1/8" = 1"
        al.nudge(y: -1)
        XCTAssertEqual(al.translation.y, -Float(Units.metersPerInch / 8), accuracy: 1e-7)
        // Nudges follow the model's own axes once it is rotated.
        var r = YawAlignment(yaw: .pi / 2, translation: .zero)
        r.nudge(x: 8)
        XCTAssertNear(r.translation, SIMD3(0, 0, -Float(Units.metersPerInch)), 1e-6)
        XCTAssertNear(r.apply(SIMD3(1, 0, 0)) - r.translation, SIMD3(0, 0, -1), 1e-6)
        var s = YawAlignment()
        s.rotate(steps: 10)
        XCTAssertEqual(Double(s.yaw) * 180 / .pi, 1.0, accuracy: 1e-4)
        s.rotate(steps: 3600)
        XCTAssertEqual(Double(s.yaw) * 180 / .pi, 1.0, accuracy: 1e-3) // wraps
    }

    func testMatrixRoundTripAndInverse() {
        let al = YawAlignment(yaw: -1.2, translation: SIMD3(3, 0.1, -4))
        let back = YawAlignment(matrix: al.matrix)
        XCTAssertEqual(back.yaw, al.yaw, accuracy: 1e-5)
        XCTAssertNear(back.translation, al.translation, 1e-6)
        let p = SIMD3<Float>(0.5, 1.2, -3)
        XCTAssertNear(al.modelPoint(fromWorld: al.apply(p)), p, 1e-5)
    }
}
