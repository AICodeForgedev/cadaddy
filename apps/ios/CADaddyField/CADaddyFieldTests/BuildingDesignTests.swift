import simd
import XCTest

/// The building / floor / room prototype: the made-up sample building, follow-the-system tracing, the
/// M/E/P/Cabinets/Furniture layers, and the motion maths.
final class BuildingDesignTests: XCTestCase {
    let b = SampleBuilding.sample()

    func testRailOrderIsRoofDownToSite() {
        XCTAssertEqual(b.railLevels.map(\.short), ["Roof", "4", "3", "2", "1", "Site"])
    }

    func testEveryElementSitsOnALevelAndInsideItsRoom() {
        XCTAssertGreaterThan(b.elements.count, 1000)
        for e in b.elements {
            XCTAssertNotNil(b.level(e.level), e.id)
            guard let r = e.room else { continue }
            guard let room = b.room(r) else { return XCTFail("\(e.id) names a missing room \(r)") }
            XCTAssertEqual(room.level, e.level, e.id)
            if e.role != .main && e.role != .equipment {
                XCTAssertTrue(room.contains(e.shape.anchor, margin: 0.05), "\(e.title) in \(r) at \(e.shape.anchor)")
            }
        }
    }

    func testIdsAreUniqueAndUpstreamLinksResolve() {
        XCTAssertEqual(Set(b.elements.map(\.id)).count, b.elements.count)
        for e in b.elements { if let u = e.upstream { XCTAssertNotNil(b.element(u), "\(e.id) → \(u)") } }
    }

    func testRoomNumbersAreUnique() {
        XCTAssertEqual(Set(b.rooms.map(\.number)).count, b.rooms.count)
    }

    func testOutletTracesUpToTheGear() {
        let outlet = try! XCTUnwrap(b.elements.first { $0.kind == "Receptacle" && $0.level == 2 })
        guard case let .chain(steps) = b.trace(outlet.id) else { return XCTFail("no chain") }
        XCTAssertEqual(steps.map(\.link), ["Outlet", "Circuit", "Panel", "Feeder", "Gear", "Transformer"])
        XCTAssertEqual(steps.first?.id, outlet.id)
        // The chain crosses floors: the outlet on level 2, the gear on level 1, the transformer on site.
        XCTAssertEqual(steps.map(\.level), [2, 2, 2, 2, 1, 0])
    }

    func testMechanicalAndPlumbingTraceTheSameWay() {
        let diffuser = try! XCTUnwrap(b.elements.first { $0.kind == "Supply Diffuser" })
        guard case let .chain(m) = b.trace(diffuser.id) else { return XCTFail("no chain") }
        XCTAssertEqual(m.map(\.link), ["Diffuser", "Branch", "Main", "Riser", "Unit"])
        let lav = try! XCTUnwrap(b.elements.first { $0.kind == "Lavatory" })
        guard case let .chain(p) = b.trace(lav.id) else { return XCTFail("no chain") }
        XCTAssertEqual(p.map(\.link), ["Fixture", "Branch", "Main", "Riser", "Heater"])
    }

    func testUnconnectedElementSaysSoInsteadOfAFalsePath() {
        let data = try! XCTUnwrap(b.elements.first { $0.kind == "Data Outlet" })
        guard case let .noKnownConnection(msg) = b.trace(data.id) else { return XCTFail("a path was invented") }
        XCTAssertTrue(msg.contains("No known connection"))
        XCTAssertEqual(b.focus(data.id).lit, [data.id])
        let bed = try! XCTUnwrap(b.elements.first { $0.kind == "Bed" })
        guard case .noKnownConnection = b.trace(bed.id) else { return XCTFail("furniture has no system") }
    }

    func testPanelLightsEverythingItFeedsAndNothingElse() {
        let panel = try! XCTUnwrap(b.elements.first { $0.kind == "Panelboard" && $0.tag == "2LA" })
        let f = b.focus(panel.id)
        XCTAssertTrue(f.showsDownstream)
        let fed = b.elements.filter { $0.panel == "2LA" }
        XCTAssertFalse(fed.isEmpty)
        for e in fed { XCTAssertTrue(f.lit.contains(e.id), e.title) }
        let other = b.elements.filter { $0.panel == "3LA" }
        for e in other { XCTAssertFalse(f.lit.contains(e.id), e.title) }
        XCTAssertEqual(f.steps.map(\.link), ["Panel", "Feeder", "Gear", "Transformer"])
    }

    func testTraceStopsOnACycle() {
        var els = b.elements
        guard let i = els.firstIndex(where: { $0.tag == "MSB" }), let o = els.firstIndex(where: { $0.kind == "Receptacle" }) else { return XCTFail() }
        els[i].upstream = els[o].id   // a broken model: the gear "fed by" an outlet
        let bad = SampleBuilding(name: "Loop", levels: b.levels, rooms: b.rooms, elements: els, footprintMin: b.footprintMin, footprintMax: b.footprintMax, bay: b.bay)
        guard case let .chain(steps) = bad.trace(els[o].id) else { return XCTFail() }
        XCTAssertEqual(Set(steps.map(\.id)).count, steps.count)
        XCTAssertLessThanOrEqual(steps.count, 32)
        XCTAssertFalse(bad.downstream(of: els[o].id).isEmpty)  // terminates
    }

    func testBuildingScopeShowsOnlyMainsAndBigEquipment() {
        for e in b.elements where e.detail == .building {
            XCTAssertTrue(e.role == .main || e.role == .equipment, e.title)
        }
        XCTAssertTrue(b.elements.contains { $0.detail == .building && $0.kind == "Switchboard" })
        XCTAssertTrue(b.elements.filter { $0.role == .terminal }.allSatisfy { $0.detail != .building })
    }

    func testNoRealJobTextInTheSample() {
        // Everything invented: catalog numbers say SAMPLE or GENERIC; the name is the made-up one.
        XCTAssertEqual(b.name, "Sample Building A")
        for e in b.elements { if let c = e.catalog { XCTAssertTrue(c.hasPrefix("SAMPLE") || c == "GENERIC", c) } }
    }

    // MARK: layers

    func testLayerOfEverySystem() {
        XCTAssertEqual(FieldLayer.of(.electrical), .electrical)
        XCTAssertEqual(FieldLayer.of(.lowVoltage), .electrical)
        XCTAssertEqual(FieldLayer.of(.fire), .plumbing)
        XCTAssertEqual(FieldLayer.of(.furniture), .furniture)
        XCTAssertNil(FieldLayer.of(.architecture))
        XCTAssertEqual(FieldLayer.of(trade: "fire-alarm", kind: nil), .electrical)
        XCTAssertEqual(FieldLayer.of(trade: "Mechanical", kind: nil), .mechanical)
        XCTAssertEqual(FieldLayer.of(trade: nil, kind: "Vanity"), .cabinets)
        XCTAssertNil(FieldLayer.of(trade: "context", kind: "Wall"))
    }

    func testLayerPrefsPersistAndCarryToAR() {
        let d = UserDefaults(suiteName: "layers-test-\(UUID().uuidString)")!
        var p = LayerPrefs.load(d)
        XCTAssertEqual(p.off, [])
        p.set(.mechanical, on: false); p.set(.furniture, on: false)
        p.save(d)
        let q = LayerPrefs.load(d)
        XCTAssertEqual(q, p)
        XCTAssertFalse(q.shows(.mechanical)); XCTAssertTrue(q.shows(.architecture)); XCTAssertTrue(q.shows(.fire))
        XCTAssertEqual(q.hiddenTrades(["electrical", "mechanical", "context"]), ["mechanical"])
        XCTAssertTrue(FieldLayer.cabinets.isDimmed(at: .building))
        XCTAssertFalse(FieldLayer.cabinets.isDimmed(at: .floor))
        XCTAssertFalse(FieldLayer.electrical.isDimmed(at: .building))
    }

    // MARK: motion

    func testDesignSpringSettlesWithinSevenTenths() {
        let s = Spring.design
        XCTAssertEqual(s.response, 0.55); XCTAssertEqual(s.dampingFraction, 0.85)
        XCTAssertLessThan(s.settleTime(tolerance: 0.02), 0.75)
        // A long frame does not blow it up.
        var x: Float = 0, v: Float = 0
        s.step(&x, &v, toward: 1, dt: 2)
        XCTAssertEqual(x, 1, accuracy: 0.01)
    }

    func testFloorRailLiftsTheFloorsAboveTheThumb() {
        let n = b.levels.count
        XCTAssertEqual(FloorLift.offsets(levels: n, focus: nil, gap: 8, spread: 1), Array(repeating: 0, count: n))
        let f = FloorLift.focus(railFraction: 0.6, levels: n)   // thumb at level 2
        XCTAssertEqual(FloorLift.detent(f, levels: n), 2)
        let o = FloorLift.offsets(levels: n, focus: Float(2), gap: 8, spread: 1)
        XCTAssertEqual(o[2], 2, accuracy: 0.01)            // the focus floor only spreads
        XCTAssertEqual(o[3] - o[2], 9, accuracy: 0.01)     // the one above opens the gap
        XCTAssertEqual(o[1], 1, accuracy: 0.01)
        for i in 1..<n { XCTAssertGreaterThan(o[i], o[i - 1]) }
        XCTAssertEqual(FloorLift.focus(railFraction: 0, levels: n), Float(n - 1))
        XCTAssertEqual(FloorLift.focus(railFraction: .nan, levels: n), Float(n - 1))
    }

    func testRevealSweepGoesSolidToClear() {
        XCTAssertEqual(RevealSweep.solidOpacity(x: 10, minX: 0, maxX: 56, progress: 0, band: 4), 1, accuracy: 0.001)
        XCTAssertEqual(RevealSweep.solidOpacity(x: 10, minX: 0, maxX: 56, progress: 1, band: 4), 0, accuracy: 0.001)
        let mid = (0...56).map { RevealSweep.solidOpacity(x: Float($0), minX: 0, maxX: 56, progress: 0.5, band: 4) }
        XCTAssertEqual(mid.first ?? -1, 0, accuracy: 0.001); XCTAssertEqual(mid.last ?? -1, 1, accuracy: 0.001)
        for i in 1..<mid.count { XCTAssertGreaterThanOrEqual(mid[i], mid[i - 1]) }
    }
}
