import simd
import XCTest

/// The four views (Architectural | MEP | Floor | Room): with every switch on, what each draws stays inside its
/// limit and is never everything; and the MEP densities follow the rules (mains, Full MEP, crossing floors,
/// elevators, sprinklers, size limits), decided from data, never from tags.
final class FieldViewsTests: XCTestCase {
    let b = SampleBuilding.sample()
    lazy var f = ViewFilter(b)
    let allOn = LayerPrefs(off: [])

    private var all: Set<String> { Set(b.elements.map(\.id)) }
    private func els(_ ids: Set<String>) -> [BuildingElement] { ids.compactMap { b.element($0) } }
    private var mains: Set<String> { f.drawn(ViewState(view: .mep, density: .mains), prefs: allOn) }
    private var fullMEP: Set<String> { f.drawn(ViewState(view: .mep, density: .full), prefs: allOn) }
    private func floor(_ i: Int) -> Set<String> { f.drawn(ViewState(view: .floor, level: i), prefs: allOn) }
    private func room(_ n: String, prefs: LayerPrefs? = nil) -> Set<String> {
        f.drawn(ViewState(view: .room, level: b.room(n)?.level, room: n), prefs: prefs ?? allOn)
    }

    /// Every view, every density, every level and room the sample has.
    private var everyState: [ViewState] {
        var out = [ViewState(view: .architectural), ViewState(view: .mep, density: .mains), ViewState(view: .mep, density: .full)]
        for lv in b.levels { out.append(ViewState(view: .floor, level: lv.index)) }
        for r in b.rooms { out.append(ViewState(view: .room, level: r.level, room: r.number)) }
        return out
    }

    // MARK: the hard rule

    func testEveryViewWithEverySwitchOnStaysInsideItsLimitAndIsNeverEverything() {
        for st in everyState {
            let d = f.drawn(st, prefs: allOn)
            XCTAssertNotEqual(d, all, "\(st) draws every object")
            XCTAssertLessThan(d.count, all.count / 2, "\(st)")
            for e in els(d) { XCTAssertTrue(f.allows(e, st, prefs: allOn), "\(st) draws \(e.title) outside its limit") }
        }
    }

    func testTheLimitIsAppliedAfterTheViewsOwnSet() {
        // Even an element the view's own set would want is dropped when the limit says no: furniture with its
        // switch off, in the very room it stands in.
        let n = "207"
        let bed = try! XCTUnwrap(b.elements.first { $0.room == n && $0.part == .furniture })
        let st = ViewState(view: .room, level: 2, room: n)
        XCTAssertFalse(f.allows(bed, st, prefs: LayerPrefs.standard))
        XCTAssertFalse(f.drawn(st, prefs: LayerPrefs.standard).contains(bed.id))
        XCTAssertTrue(f.drawn(st, prefs: allOn).contains(bed.id))
    }

    func testNoViewOffersShowAll() {
        XCTAssertEqual(FieldView.allCases.map(\.title), ["Architectural", "MEP", "Floor", "Room"])
        XCTAssertEqual(ViewState.opening, ViewState(view: .mep, density: .mains))
    }

    // MARK: Architectural

    func testArchitecturalDrawsOnlyArchitecture() {
        let d = els(f.drawn(ViewState(view: .architectural), prefs: allOn))
        XCTAssertFalse(d.isEmpty)
        for e in d { XCTAssertEqual(e.system, .architecture, e.title) }
        for part in [ElementPart.door, .window, .stair] { XCTAssertTrue(d.contains { $0.part == part }, "\(part)") }
    }

    // MARK: MEP

    func testMEPHasNoArchitectureAndNoDevicesExceptElevatorDisconnects() {
        for d in [mains, fullMEP] {
            for e in els(d) {
                XCTAssertNotEqual(e.system, .architecture, e.title)
                XCTAssertFalse([.cabinets, .furniture].contains(e.system), e.title)
                XCTAssertFalse([.device, .light, .control, .fixture, .sprinklerHead, .door, .window, .stair].contains(e.part), e.title)
                if e.part == .disconnect { XCTAssertTrue(b.downstream(of: e.id).contains { b.element($0)?.system == .conveying }, e.title) }
            }
        }
    }

    func testFullMEPHasNoBranchCircuitOrSmallPipeUnlessItCrossesAFloor() {
        let c = f.classifier
        for e in els(fullMEP) {
            if e.part == .raceway, ViewClassifier.connection(e, b) == .branch {
                XCTAssertTrue(c.crossing.contains(e.id), "branch circuit \(e.title) in Full MEP")
            }
            if e.system == .plumbing, e.part == .pipe {
                XCTAssertTrue((e.nominal ?? 0) >= c.rules.plumbingMainMinInches || c.crossing.contains(e.id) || c.isInferred(e), e.title)
            }
        }
        // There are branch circuits and small pipes in the model, and none made it.
        XCTAssertTrue(b.elements.contains { $0.part == .raceway && $0.role == .branch && !fullMEP.contains($0.id) })
        XCTAssertTrue(b.elements.contains { $0.system == .plumbing && ($0.nominal ?? 9) < 2 && !fullMEP.contains($0.id) })
    }

    func testFullMEPDrawsEveryDuctPanelFeederAndCrossing() {
        let full = fullMEP
        for e in b.elements {
            let why: String?
            if e.part == .duct || e.part == .airTerminal || e.part == .fan { why = "every duct, air terminal and fan" }
            else if e.part == .panel { why = "every panel" }
            else if e.part == .raceway, ViewClassifier.connection(e, b) == .feeder { why = "every feeder and riser to a panel or equipment" }
            else if e.part == .disconnect { why = "elevator disconnect" }
            else if e.part == .mechanicalUnit { why = "units incl. outdoor air" }
            else if f.classifier.crossing.contains(e.id), !f.classifier.elevatorBranches.contains(e.id) { why = "crosses a floor" }
            else { why = nil }
            if let why { XCTAssertTrue(full.contains(e.id), "\(e.title) missing from Full MEP (\(why))") }
        }
        XCTAssertTrue(b.elements.contains { $0.kind == "Outdoor Air Unit" && full.contains($0.id) })
    }

    func testEveryBathroomExhaustFanAndItsDuctAreInFullMEP() {
        let fans = b.elements.filter { $0.part == .fan }
        XCTAssertGreaterThan(fans.count, 40)
        for fan in fans {
            XCTAssertTrue(fullMEP.contains(fan.id), fan.title)
            guard case let .chain(steps) = b.trace(fan.id) else { return XCTFail("fan not connected") }
            for s in steps { XCTAssertTrue(fullMEP.contains(s.id), "\(s.tag) above \(fan.title)") }
            XCTAssertFalse(mains.contains(fan.id), "a bath fan is not big gear")
        }
    }

    func testMainsIsTheBigStuffOnly() {
        let m = els(mains)
        for part in [ElementPart.transformer, .switchgear, .cableTray, .waterHeater, .pump] {
            XCTAssertTrue(m.contains { $0.part == part }, "\(part)")
        }
        // House panels yes, sub-panels no (told from the distribution: fed from gear vs from a panel).
        let panels = b.elements.filter { $0.part == .panel }
        XCTAssertTrue(panels.contains { mains.contains($0.id) })
        XCTAssertTrue(panels.contains { !mains.contains($0.id) && fullMEP.contains($0.id) })
        XCTAssertTrue(mains.isSubset(of: fullMEP))
        XCTAssertLessThan(mains.count, fullMEP.count)
    }

    // MARK: elevators

    func testEachElevatorHasItsHoistwayMachineDisconnectAndFeederInBothDensities() {
        let hoist = b.elements.filter { $0.part == .hoistway }
        XCTAssertEqual(hoist.count, 1)
        for d in [mains, fullMEP] {
            XCTAssertTrue(hoist.allSatisfy { d.contains($0.id) })
            let machine = try! XCTUnwrap(b.elements.first { $0.part == .elevatorMachine })
            XCTAssertTrue(d.contains(machine.id))
            guard case let .chain(steps) = b.trace(machine.id) else { return XCTFail("machine not connected") }
            // Machine ← feeder ← disconnect ← feeder ← gear: all drawn.
            for s in steps where b.element(s.id)?.system != .conveying { XCTAssertTrue(d.contains(s.id), "\(s.link) \(s.tag)") }
            XCTAssertTrue(steps.contains { b.element($0.id)?.part == .disconnect })
        }
    }

    func testNoElevatorBranchIsInAnyMEPViewEvenCrossingFloors() {
        let branches = f.classifier.elevatorBranches
        XCTAssertGreaterThanOrEqual(branches.count, 4)   // car light circuit, traveling cable, car light, pit light circuit + light
        XCTAssertTrue(branches.contains { f.classifier.crossing.contains($0) }, "the sample has an elevator branch that crosses a floor")
        for id in branches {
            XCTAssertFalse(mains.contains(id)); XCTAssertFalse(fullMEP.contains(id), b.element(id)?.title ?? id)
        }
    }

    // MARK: fire protection

    func testSprinklersMainsInMEPHeadsAndBranchesOnTheFloor() {
        let heads = b.elements.filter { $0.part == .sprinklerHead }
        let fireBranches = b.elements.filter { $0.system == .fire && $0.part == .pipe && $0.role == .branch }
        XCTAssertFalse(heads.isEmpty); XCTAssertFalse(fireBranches.isEmpty)
        for e in heads + fireBranches where !f.classifier.crossing.contains(e.id) {
            XCTAssertFalse(fullMEP.contains(e.id), e.title)
        }
        for e in b.elements where e.system == .fire && (e.role == .main || e.part == .pump) {
            XCTAssertTrue(mains.contains(e.id), e.title)
        }
        for lv in 1...4 {
            let fl = floor(lv)
            for e in heads + fireBranches where e.level == lv { XCTAssertTrue(fl.contains(e.id), "\(e.title) not on its floor") }
            for e in heads where e.level != lv { XCTAssertFalse(fl.contains(e.id)) }
        }
    }

    // MARK: size and connection rules (synthetic edge cases)

    private func run(_ id: String, _ system: BuildingSystem, _ role: ElementRole, nominal: Float?, upstream: String? = nil, ys: (Float, Float) = (2, 2)) -> BuildingElement {
        BuildingElement(id: id, kind: "Run", tag: id, system: system, role: role, level: 1, room: nil,
                        shape: .run(points: [SIMD3(1, ys.0, 1), SIMD3(5, ys.1, 1)], profile: .round(radius: 0.02)),
                        mountingHeight: nil, catalog: nil, circuit: nil, panel: nil, size: nil, upstream: upstream, detail: .floor,
                        part: system == .electrical ? .raceway : .pipe, nominal: nominal, load: nil, link: "Run")
    }

    func testRunOfUnknownConnectionIsDecidedBySizeAndUnknownPipeSizeIsNeverGuessed() {
        let levels = [BuildingLevel(index: 0, name: "Site", short: "Site", elevation: 0, height: 0),
                      BuildingLevel(index: 1, name: "Level 1", short: "1", elevation: 0, height: 4),
                      BuildingLevel(index: 2, name: "Level 2", short: "2", elevation: 4, height: 4)]
        let els = [
            run("big", .electrical, .main, nominal: 1.25),
            run("small", .electrical, .main, nominal: 0.5),
            run("cross", .electrical, .branch, nominal: 0.5, ys: (2, 6)),   // passes through level 2's floor
            run("p2", .plumbing, .main, nominal: 2),
            run("p1", .plumbing, .main, nominal: 1.5),
            run("pnil", .plumbing, .main, nominal: nil),
            run("pq", .plumbing, .unknown, nominal: nil),      // neither role nor size: shown, inferred
            run("pu", .plumbing, .unknown, nominal: 1.5),      // role unknown: size decides
            run("pv", .plumbing, .unknown, nominal: 2),
            run("fbig", .fire, .terminal, nominal: 3),     // a sprinkler pipe of unknown role: size decides
            run("fsmall", .fire, .terminal, nominal: 2),
        ]
        let sb = SampleBuilding(name: "Synthetic", levels: levels, rooms: [], elements: els, footprintMin: .zero, footprintMax: SIMD2(10, 10), bay: 4)
        let vf = ViewFilter(sb)
        let full = vf.drawn(ViewState(view: .mep, density: .full), prefs: allOn)
        XCTAssertTrue(full.contains("big")); XCTAssertFalse(full.contains("small"))
        XCTAssertTrue(full.contains("cross"))
        // Role decides first (Q6431): a main shows whatever its size, and with no size; size decides only when the
        // role is unknown; with neither, Full MEP shows it marked inferred (never dropped).
        XCTAssertTrue(full.contains("p2")); XCTAssertTrue(full.contains("p1")); XCTAssertTrue(full.contains("pnil"))
        XCTAssertTrue(full.contains("pq")); XCTAssertFalse(full.contains("pu")); XCTAssertTrue(full.contains("pv"))
        XCTAssertEqual(vf.classifier.unknownSizeCount(sb), 2)
        XCTAssertEqual(sb.elements.filter(vf.classifier.isInferred).map(\.id), ["pq"])
        XCTAssertTrue(full.contains("fbig")); XCTAssertFalse(full.contains("fsmall"))
        // A project can move the limits (rules data).
        var r = ViewRules(); r.plumbingMainMinInches = 1.5; r.feederMinInches = 2
        let moved = ViewFilter(sb, rules: r).drawn(ViewState(view: .mep, density: .full), prefs: allOn)
        XCTAssertTrue(moved.contains("pu")); XCTAssertFalse(moved.contains("big"))
        let decoded = try! ViewRules.decode(Data(#"{"plumbingMainMinInches": 3}"#.utf8))
        XCTAssertEqual(decoded.plumbingMainMinInches, 3); XCTAssertEqual(decoded.fireMainMinInches, 2.5)
    }

    func testTheSamplesPipeWithNoRoleOrSizeShowsInFullMEPMarkedInferred() {
        // Design-2 left it out with a note (owner review: wrong). It shows, marked inferred, and the note says so.
        XCTAssertEqual(f.classifier.unknownSizeCount(b), 1)
        let pipe = try! XCTUnwrap(b.elements.first { $0.part == .pipe && $0.nominal == nil })
        XCTAssertTrue(fullMEP.contains(pipe.id))
        XCTAssertTrue(f.classifier.isInferred(pipe))
        XCTAssertEqual(f.classifier.inferredCount(b), 1)
    }

    // MARK: Floor and Room

    func testFloorDrawsOnlyItsLevelWithPanelsLightsCabinetsAndNoFurniture() {
        let fl = els(floor(2))
        XCTAssertTrue(fl.allSatisfy { $0.level == 2 })
        XCTAssertTrue(fl.contains { $0.part == .light }); XCTAssertTrue(fl.contains { $0.part == .panel }); XCTAssertTrue(fl.contains { $0.system == .cabinets })
        XCTAssertFalse(fl.contains { $0.system == .furniture })
        XCTAssertFalse(fl.contains { $0.part == .device })
    }

    func testFurnitureOnlyInRoomAndOnlyWhenSwitchedOn() {
        for st in everyState where st.view != .room {
            XCTAssertFalse(els(f.drawn(st, prefs: allOn)).contains { $0.system == .furniture }, "\(st)")
        }
        let n = "207"
        XCTAssertFalse(els(room(n, prefs: .standard)).contains { $0.system == .furniture })
        XCTAssertTrue(els(room(n)).contains { $0.system == .furniture })
        let r = els(room(n))
        XCTAssertTrue(r.contains { $0.part == .device }); XCTAssertTrue(r.contains { $0.part == .fixture })
        for e in r { XCTAssertTrue(e.room == n || f.serving(n).contains(e.id), e.title) }
    }

    func testEveryObjectIsReachableInSomeView() {
        // Nothing is lost: all of it stays in the model, and every element is drawn by at least one view.
        var seen = Set<String>()
        for st in everyState { seen.formUnion(f.drawn(st, prefs: allOn)) }
        let missing = b.elements.filter { !seen.contains($0.id) }
        // Only the unsized pipe (left out, counted) and room-less floor items on Site may sit outside every view.
        for e in missing { XCTAssertTrue(e.nominal == nil && e.part == .pipe, "\(e.title) is in no view") }
    }
}
