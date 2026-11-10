import CoreGraphics
import simd
import XCTest

/// Layout rules for the building / floor / room screens (design polish): chrome that never touches, the
/// building framed into the free area for any proportions and phone, labels that never overlap, depth fog.
final class FieldLayoutTests: XCTestCase {
    /// Phone screens (points) with their safe areas: small, compact, standard, Pro, Pro Max, plus an old SE.
    let phones: [(CGSize, CGFloat, CGFloat)] = [
        (CGSize(width: 320, height: 568), 20, 0),
        (CGSize(width: 375, height: 667), 20, 0),
        (CGSize(width: 393, height: 852), 59, 34),
        (CGSize(width: 402, height: 874), 62, 34),
        (CGSize(width: 440, height: 956), 62, 34),
    ]
    let fov: Float = 42

    // MARK: chrome

    func testChromeNeverTouchesInAnyScopeSheetOrTextSize() {
        // Header heights cover one-line titles up to the largest supported text size with a breadcrumb;
        // bottom heights cover the building hint, the floor sheet with and without the path strip, and the
        // room sheet down (collapsed) and up.
        for (screen, top, bottom) in phones {
            for header in [70, 116, 150, 210, 300] as [CGFloat] {
                for sheet in [40, 130, 240, 420, 700] as [CGFloat] {
                    for rail in [0, 6, 12] {
                        let l = ChromeLayout.make(.init(screen: screen, safeTop: top, safeBottom: bottom, headerHeight: header, bottomHeight: sheet, railItems: rail))
                        let tag = "\(screen) header \(header) sheet \(sheet) rail \(rail)"
                        XCTAssertTrue(l.collisions.isEmpty, "\(tag): \(l.collisions)")
                        let safe = CGRect(x: 0, y: top, width: screen.width, height: screen.height - top - bottom)
                        for r in [l.header, l.close, l.layers, l.bottom] + (l.rail.map { [$0] } ?? []) where r.height > 0 {
                            XCTAssertTrue(safe.insetBy(dx: -0.5, dy: -0.5).contains(r), "\(tag): \(r) outside the safe area")
                        }
                        for r in [l.close, l.layers] + (l.rail.map { [$0] } ?? []) {
                            XCTAssertFalse(l.free.intersects(r), "\(tag): the scene's free area runs under \(r)")
                        }
                        XCTAssertGreaterThanOrEqual(l.layers.height, 56, tag)
                        // The view holds the bottom stack to the cap (the room sheet plus the view control can be taller).
                        XCTAssertLessThanOrEqual(l.bottom.height, l.maxBottom + 0.5, tag)
                        XCTAssertLessThanOrEqual(l.layers.maxY + ChromeLayout.gap, l.bottom.maxY - min(sheet, l.maxBottom) + 0.5, tag)
                        switch l.railMode {
                        case let .rail(pitch): XCTAssertGreaterThanOrEqual(pitch, ChromeLayout.minPitch, tag); XCTAssertGreaterThan(rail, 0, tag)
                        case .menu: XCTAssertGreaterThan(rail, 0, tag)
                        case .hidden: XCTAssertEqual(rail, 0, tag)
                        }
                    }
                }
            }
        }
    }

    func testRailKeepsFullGloveTargetsOnEveryPhoneWithTheFloorSheet() {
        // The usual case: six floors, the floor sheet with its path strip, a two-line header.
        for (screen, top, bottom) in phones where screen.height >= 800 {
            let l = ChromeLayout.make(.init(screen: screen, safeTop: top, safeBottom: bottom, headerHeight: 120, bottomHeight: 256, railItems: 6))
            XCTAssertEqual(l.railMode, .rail(pitch: 56), "\(screen)")
        }
    }

    /// The design-1 floor view with a path strip up: the rail was padded from the top by a constant and from the
    /// bottom by a guess at the sheet height (250 + 70), so the layers button ran into the rail's bottom.
    /// Measured from the design-1 screenshot: bottom stack 256 pt above the safe area. The old placement
    /// overlaps; the rule's placement keeps the gap with the same measured sizes.
    func testDesignOneFloorRailTouchedTheLayersButton() {
        let screen = CGSize(width: 402, height: 874), safeTop: CGFloat = 62, safeBottom: CGFloat = 34
        let floorY = screen.height - safeBottom
        let stack: CGFloat = 256
        let railH: CGFloat = 6 * 56 + 12
        let colTop = safeTop + 120, colBottom = floorY - (250 + 70)
        let oldRail = CGRect(x: 402 - 10 - 56, y: (colTop + colBottom) / 2 - railH / 2, width: 56, height: railH)
        let oldLayers = CGRect(x: 402 - 16 - 56, y: floorY - stack - 10 - 56, width: 56, height: 56)
        XCTAssertTrue(oldRail.intersects(oldLayers), "design-1 placement no longer reproduces")
        let l = ChromeLayout.make(.init(screen: screen, safeTop: safeTop, safeBottom: safeBottom, headerHeight: 116, bottomHeight: stack, railItems: 6))
        let rail = try! XCTUnwrap(l.rail)
        XCTAssertGreaterThanOrEqual(l.layers.minY - rail.maxY, ChromeLayout.gap)
        XCTAssertEqual(l.railMode, .rail(pitch: 56))
    }

    // MARK: framing

    func testBuildingFitsTheFreeAreaForAnyProportionsAndPhone() {
        let shapes: [(String, SIMD3<Float>)] = [
            ("long and thin", SIMD3(120, 16, 14)), ("the sample", SIMD3(56, 16, 18)), ("square", SIMD3(40, 18, 40)),
            ("tall", SIMD3(18, 90, 22)), ("tiny", SIMD3(3, 3, 4)),
        ]
        for (screen, top, bottom) in phones {
            let l = ChromeLayout.make(.init(screen: screen, safeTop: top, safeBottom: bottom, headerHeight: 70, bottomHeight: 40, railItems: 6))
            for (name, size) in shapes {
                for (yaw, pitch) in [(Float(-2.25), Float(0.56)), (0.6, 0.38), (-0.62, 0.92)] {
                    let pts = SceneFraming.corners(min: SIMD3(5, 0, -3), max: SIMD3(5, 0, -3) + size)
                    let pose = SceneFraming.fit(pts, yaw: yaw, pitch: pitch, fovYDegrees: fov, viewport: screen, free: l.free)
                    let sb = try! XCTUnwrap(SceneFraming.screenBounds(pts, pose: pose, fovYDegrees: fov, viewport: screen), name)
                    let tag = "\(name) on \(screen) yaw \(yaw)"
                    XCTAssertTrue(l.free.insetBy(dx: -1, dy: -1).contains(sb), "\(tag): \(sb) not inside \(l.free)")
                    XCTAssertEqual(sb.midX, l.free.midX, accuracy: 2, tag)
                    XCTAssertEqual(sb.midY, l.free.midY, accuracy: 2, tag)
                    // As large as fits: one side fills the free area (less the margin).
                    let fill = max(sb.width / (l.free.width - 24), sb.height / (l.free.height - 24))
                    XCTAssertGreaterThan(fill, 0.97, tag)
                }
            }
        }
    }

    /// design-1 framed the building with constants (target, distance). On a Pro phone the sample building ran
    /// under the floor rail and off the right edge, with the top third empty.
    func testDesignOneBuildingFramingRanUnderTheRail() {
        let b = SampleBuilding.sample()
        let screen = CGSize(width: 402, height: 874)
        let l = ChromeLayout.make(.init(screen: screen, safeTop: 62, safeBottom: 34, headerHeight: 62, bottomHeight: 44, railItems: 6))
        let pts = SceneFraming.corners(min: SIMD3(b.footprintMin.x, 0, b.footprintMin.y), max: SIMD3(b.footprintMax.x, b.height, b.footprintMax.y))
        let old = OrbitPose(target: SIMD3(33, 3, 9), yaw: -2.25, pitch: 0.56, distance: 150)
        let before = try! XCTUnwrap(SceneFraming.screenBounds(pts, pose: old, fovYDegrees: fov, viewport: screen))
        XCTAssertGreaterThan(before.maxX, try! XCTUnwrap(l.rail).minX, "design-1 framing no longer reproduces")
        let pose = SceneFraming.fit(pts, yaw: -2.25, pitch: 0.56, fovYDegrees: fov, viewport: screen, free: l.free)
        let after = try! XCTUnwrap(SceneFraming.screenBounds(pts, pose: pose, fovYDegrees: fov, viewport: screen))
        XCTAssertLessThan(after.maxX, try! XCTUnwrap(l.rail).minX)
        XCTAssertTrue(l.free.insetBy(dx: -1, dy: -1).contains(after))
    }

    func testTurningNeverFramesSmallerAndStaysInside() {
        let screen = CGSize(width: 402, height: 874)
        let l = ChromeLayout.make(.init(screen: screen, safeTop: 62, safeBottom: 34, headerHeight: 116, bottomHeight: 154, railItems: 6))
        for size in [SIMD3<Float>(120, 4, 14), SIMD3(56, 4, 18), SIMD3(30, 4, 30)] {
            let pts = SceneFraming.corners(min: .zero, max: size)
            let fixed = SceneFraming.fit(pts, yaw: -2.25, pitch: 0.74, fovYDegrees: fov, viewport: screen, free: l.free)
            let turned = SceneFraming.fitTurning(pts, yaw: -2.25, spread: 0.6, pitch: 0.74, fovYDegrees: fov, viewport: screen, free: l.free)
            XCTAssertLessThanOrEqual(turned.distance, fixed.distance + 1e-3, "\(size)")
            XCTAssertLessThanOrEqual(abs(turned.yaw + 2.25), 0.6 + 1e-4)
            let sb = try! XCTUnwrap(SceneFraming.screenBounds(pts, pose: turned, fovYDegrees: fov, viewport: screen))
            XCTAssertTrue(l.free.insetBy(dx: -1, dy: -1).contains(sb), "\(size)")
        }
    }

    func testProjectionBehindTheCameraIsNil() {
        let pose = OrbitPose(target: .zero, yaw: 0, pitch: 0, distance: 10)
        XCTAssertNil(SceneFraming.project(SIMD3(0, 0, 20), pose: pose, fovYDegrees: fov, viewport: CGSize(width: 400, height: 800)))
        let c = try! XCTUnwrap(SceneFraming.project(.zero, pose: pose, fovYDegrees: fov, viewport: CGSize(width: 400, height: 800)))
        XCTAssertEqual(c.x, 200, accuracy: 0.01); XCTAssertEqual(c.y, 400, accuracy: 0.01)
    }

    // MARK: labels

    /// design-1: a water closet and lavatory label 12 pt apart sat on top of each other.
    func testCloseLabelsNeverOverlap() {
        let size = CGSize(width: 52, height: 20)
        let a = LabelCandidate(id: "WC-1", anchor: CGPoint(x: 200, y: 300), size: size, priority: 2)
        let b = LabelCandidate(id: "LAV-1", anchor: CGPoint(x: 212, y: 304), size: size, priority: 2)
        let naive = [a, b].map { CGRect(x: $0.anchor.x - 26, y: $0.anchor.y - 10, width: 52, height: 20) }
        XCTAssertTrue(naive[0].intersects(naive[1]), "the old centred placement no longer reproduces the overlap")
        let placed = LabelLayout.place([a, b], bounds: CGRect(x: 0, y: 0, width: 400, height: 800), avoid: [])
        XCTAssertEqual(placed.count, 2)
        XCTAssertFalse(placed[0].frame.intersects(placed[1].frame))
        XCTAssertTrue(placed.contains { $0.hasLeader }, "the moved label keeps a leader to its object")
    }

    func testDenseLabelsAreSpreadOrDroppedNeverOverlapping() {
        var rng = SplitMix(seed: 7)
        for phone in phones.map(\.0) {
            for count in [5, 20, 60, 150] {
                let rail = CGRect(x: phone.width - 66, y: 200, width: 56, height: 348)
                let header = CGRect(x: 0, y: 0, width: phone.width, height: 150)
                let labels = (0..<count).map { i in
                    LabelCandidate(id: "L\(i)", anchor: CGPoint(x: CGFloat(rng.next() % UInt64(phone.width)), y: CGFloat(rng.next() % UInt64(phone.height))),
                                   size: CGSize(width: CGFloat(30 + rng.next() % 50), height: 20), priority: Int(rng.next() % 4))
                }
                let bounds = CGRect(origin: .zero, size: phone)
                let placed = LabelLayout.place(labels, bounds: bounds, avoid: [rail, header])
                for (i, p) in placed.enumerated() {
                    XCTAssertTrue(bounds.contains(p.frame), "label cut by the screen edge")
                    XCTAssertFalse(p.frame.intersects(rail), "label under the rail")
                    XCTAssertFalse(p.frame.intersects(header), "label under the header")
                    for q in placed[(i + 1)...] { XCTAssertFalse(p.frame.intersects(q.frame), "\(p.id) overlaps \(q.id)") }
                }
                if count <= 5 { XCTAssertGreaterThanOrEqual(placed.count, labels.filter { !header.contains($0.anchor) && !rail.contains($0.anchor) }.count - 2) }
            }
        }
    }

    func testHigherPriorityKeepsItsPlaceAndLowerIsDroppedFirst() {
        // Room for exactly one label: a 60 × 24 slot.
        let bounds = CGRect(x: 0, y: 0, width: 60, height: 24)
        let low = LabelCandidate(id: "a-low", anchor: CGPoint(x: 30, y: 12), size: CGSize(width: 50, height: 16), priority: 1)
        let high = LabelCandidate(id: "z-high", anchor: CGPoint(x: 30, y: 12), size: CGSize(width: 50, height: 16), priority: 5)
        let placed = LabelLayout.place([low, high], bounds: bounds, avoid: [])
        XCTAssertEqual(placed.map(\.id), ["z-high"])
        XCTAssertEqual(placed.first?.slot, 0)
    }

    func testLabelKeepsItsPreviousSlotSoItDoesNotJump() {
        let c = LabelCandidate(id: "P1", anchor: CGPoint(x: 200, y: 400), size: CGSize(width: 40, height: 20), priority: 1)
        let placed = LabelLayout.place([c], bounds: CGRect(x: 0, y: 0, width: 400, height: 800), avoid: [], previous: ["P1": 3])
        XCTAssertEqual(placed.first?.slot, 3)
    }

    func testLabelsAtTheEdgeAreMovedInsideOrDroppedNeverCut() {
        let c = LabelCandidate(id: "RTU-1", anchor: CGPoint(x: 396, y: 400), size: CGSize(width: 60, height: 22), priority: 3)
        let placed = LabelLayout.place([c], bounds: CGRect(x: 0, y: 0, width: 402, height: 874), avoid: [])
        XCTAssertEqual(placed.count, 1)
        XCTAssertLessThanOrEqual(placed[0].frame.maxX, 402)
    }

    // MARK: depth fog

    func testFogGrowsWithDistanceAndSparesTheNearPart() {
        XCTAssertEqual(DepthFog.step(distance: 10, near: 10, far: 50), 0)
        XCTAssertEqual(DepthFog.step(distance: 20, near: 10, far: 50), 0)
        XCTAssertEqual(DepthFog.step(distance: 50, near: 10, far: 50), DepthFog.steps)
        var last = 0
        for d in stride(from: Float(10), through: 50, by: 0.5) {
            let s = DepthFog.step(distance: d, near: 10, far: 50)
            XCTAssertGreaterThanOrEqual(s, last); last = s
        }
        XCTAssertEqual(DepthFog.step(distance: .nan, near: 1, far: 2), 0)
        XCTAssertEqual(DepthFog.mix(step: DepthFog.steps, maxMix: 0.6), 0.6, accuracy: 1e-6)
    }
}

/// A tiny deterministic generator for the layout fuzz.
struct SplitMix {
    var state: UInt64
    init(seed: UInt64) { state = seed }
    mutating func next() -> UInt64 {
        state &+= 0x9E37_79B9_7F4A_7C15
        var z = state
        z = (z ^ (z >> 30)) &* 0xBF58_476D_1CE5_E9B9
        z = (z ^ (z >> 27)) &* 0x94D0_49BB_1331_11EB
        return z ^ (z >> 31)
    }
}
