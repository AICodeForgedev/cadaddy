// CADaddy Field — the four views: Architectural | MEP | Floor | Room. One is active at a time. Everything stays in
// the model; this decides only what is drawn. No state draws every object: the active view's allowed set is
// applied last, after every switch.
//
//   Architectural — the whole building's architecture (rooms, walls, windows, doors, floors, roof, stairs). Nothing else.
//   MEP           — the opening view. Floors and one outline as the only context, plus mains and big gear;
//                   Full MEP adds every duct, air terminal and fan, plumbing mains at or over the size limit,
//                   electrical distribution up to every panel (never the branches out of one), and anything that
//                   passes through a floor — except the branches serving an elevator.
//   Floor         — one level: its architecture as pale context, its panels, lights, cabinets and all its MEP
//                   including sprinkler branch lines and heads.
//   Room          — one room: everything in it; furniture only when its switch is on (off by default).
//
// How an element is told apart comes from data, never from a tag or a name: its object class (`part`), its role
// in the system, its nominal size, what it connects to, and its geometry against the level elevations. The size
// limits are rules data (`ViewRules`, JSON), editable per project.

import Foundation
import simd

/// The four views of the always-visible control.
public enum FieldView: String, CaseIterable, Codable, Sendable, Identifiable {
    case architectural, mep, floor, room
    public var id: String { rawValue }
    public var title: String {
        switch self {
        case .architectural: return "Architectural"
        case .mep: return "MEP"
        case .floor: return "Floor"
        case .room: return "Room"
        }
    }

    /// The trade switches the view offers. Furniture only in Room; Cabinets in Floor and Room; Architectural has none.
    public var layers: [FieldLayer] {
        switch self {
        case .architectural: return []
        case .mep: return [.mechanical, .electrical, .plumbing]
        case .floor: return [.mechanical, .electrical, .plumbing, .cabinets]
        case .room: return FieldLayer.allCases
        }
    }
}

/// MEP's two densities. A model opens in mains.
public enum MEPDensity: String, Codable, Sendable { case mains, full }

/// The active view with what it is looking at.
public struct ViewState: Equatable, Sendable {
    public var view: FieldView
    public var density: MEPDensity
    public var level: Int?
    public var room: String?
    public init(view: FieldView, density: MEPDensity = .mains, level: Int? = nil, room: String? = nil) {
        self.view = view; self.density = density; self.level = level; self.room = room
    }
    /// What a model with no saved view opens in.
    public static let opening = ViewState(view: .mep, density: .mains)
}

/// The size limits and thresholds, as data (one JSON object, editable per project).
public struct ViewRules: Codable, Equatable, Sendable {
    /// Plumbing pipe at or over this is a main line and shows in Full MEP; smaller never shows in MEP.
    public var plumbingMainMinInches: Float = 2
    /// A sprinkler pipe of unknown role at or over this is a main (branch lines commonly run up to 2").
    public var fireMainMinInches: Float = 2.5
    /// Plumbing mains at or over this are "large pipe" and show in mains density too.
    public var largePipeMinInches: Float = 2.5
    /// A duct main at or over this (its larger side) is a main duct for the mains density.
    public var mainDuctMinInches: Float = 12
    /// A raceway or cable of unknown connection at or over this counts as a feeder.
    public var feederMinInches: Float = 1
    /// Equipment with at least this volume is "large" (big gear, large mechanical units).
    public var largeEquipmentMinCubicMetres: Float = 1
    /// A run must pass at least this far through a level's elevation to count as crossing the floor.
    public var floorCrossingTolerance: Float = 0.05

    public init() {}

    public static let standard = ViewRules()

    /// Reads a project's rules; any key left out keeps its standard value.
    public static func decode(_ data: Data) throws -> ViewRules {
        let partial = try JSONDecoder().decode(Partial.self, from: data)
        var r = ViewRules()
        if let v = partial.plumbingMainMinInches { r.plumbingMainMinInches = v }
        if let v = partial.fireMainMinInches { r.fireMainMinInches = v }
        if let v = partial.largePipeMinInches { r.largePipeMinInches = v }
        if let v = partial.mainDuctMinInches { r.mainDuctMinInches = v }
        if let v = partial.feederMinInches { r.feederMinInches = v }
        if let v = partial.largeEquipmentMinCubicMetres { r.largeEquipmentMinCubicMetres = v }
        if let v = partial.floorCrossingTolerance { r.floorCrossingTolerance = v }
        return r
    }

    private struct Partial: Codable {
        var plumbingMainMinInches: Float?, fireMainMinInches: Float?, largePipeMinInches: Float?, mainDuctMinInches: Float?
        var feederMinInches: Float?, largeEquipmentMinCubicMetres: Float?, floorCrossingTolerance: Float?
    }
}

/// The closest view an element is drawn in. Each element has exactly one tier.
public enum ViewTier: Int, Comparable, Sendable, CaseIterable {
    /// Architectural view only (and pale context on its floor).
    case architecture
    /// MEP in both densities (and its floor, and the room it serves).
    case mains
    /// MEP when Full MEP is on (and its floor, and the room it serves).
    case fullMEP
    /// Its floor (and its room).
    case floor
    /// Its room only.
    case room
    public static func < (a: ViewTier, b: ViewTier) -> Bool { a.rawValue < b.rawValue }
}

/// What a run of raceway connects to, from the distribution data.
public enum Connection: Equatable, Sendable {
    /// It reaches a panel or other equipment.
    case feeder
    /// It reaches outlets, lights or small devices.
    case branch
    /// The model does not say.
    case unknown
}

/// Every element's tier, computed once for a building and a set of rules.
public struct ViewClassifier: Sendable {
    public let rules: ViewRules
    public private(set) var tiers: [String: ViewTier] = [:]
    /// Electrical branches, devices and cables that serve an elevator (never drawn in MEP, even crossing floors).
    public private(set) var elevatorBranches: Set<String> = []
    /// Runs that pass through a floor.
    public private(set) var crossing: Set<String> = []

    public init(_ b: SampleBuilding, rules: ViewRules = .standard) {
        self.rules = rules
        let hoistways = b.elements.filter { $0.part == .hoistway }.map { $0.shape.bounds }
        let floors = b.levels.filter { $0.index > 0 }.map(\.elevation)
        var cache: [String: ViewTier] = [:]
        for e in b.elements where Self.crossesFloor(e, floors: floors, tolerance: rules.floorCrossingTolerance) { crossing.insert(e.id) }
        for e in b.elements where Self.servesElevator(e, b, hoistways: hoistways) { elevatorBranches.insert(e.id) }
        for e in b.elements { _ = tier(e, b, &cache) }
        tiers = cache
    }

    public func tier(_ id: String) -> ViewTier? { tiers[id] }

    // MARK: rules

    /// True when a run passes through one of the level elevations (from its geometry, not its level property).
    public static func crossesFloor(_ e: BuildingElement, floors: [Float], tolerance: Float) -> Bool {
        guard case let .run(points, _) = e.shape, points.count >= 2 else { return false }
        var lo = Float.greatestFiniteMagnitude, hi = -Float.greatestFiniteMagnitude
        for p in points { lo = min(lo, p.y); hi = max(hi, p.y) }
        return floors.contains { lo < $0 - tolerance && hi > $0 + tolerance }
    }

    /// An electrical branch, cable or device inside a hoistway, or one whose loads are inside it (the car's
    /// lighting, the traveling cable, pit lights and outlets). Feeders and disconnects are not branches.
    static func servesElevator(_ e: BuildingElement, _ b: SampleBuilding, hoistways: [(min: SIMD3<Float>, max: SIMD3<Float>)]) -> Bool {
        guard !hoistways.isEmpty, e.system == .electrical || e.system == .lowVoltage else { return false }
        if [.disconnect, .panel, .transformer, .switchgear].contains(e.part) { return false }
        func inside(_ p: SIMD3<Float>) -> Bool {
            hoistways.contains { h in p.x >= h.min.x - 0.05 && p.x <= h.max.x + 0.05 && p.y >= h.min.y - 0.05 && p.y <= h.max.y + 0.05 && p.z >= h.min.z - 0.05 && p.z <= h.max.z + 0.05 }
        }
        if e.part == .raceway, connection(e, b) == .feeder { return false }
        switch e.shape {
        case let .box(c, _): if inside(c) { return true }
        case let .run(points, _): if let last = points.last, inside(last) { return true }
        }
        // A branch is the elevator's when what it ends at is.
        for k in b.downstream(of: e.id) {
            guard let d = b.element(k), d.role == .terminal || d.role == .branch else { continue }
            if case let .box(c, _) = d.shape, inside(c) { return true }
            if case let .run(points, _) = d.shape, points.contains(where: inside) { return true }
        }
        return false
    }

    /// What a run connects to: a feeder reaches equipment; a branch reaches terminals; otherwise unknown.
    public static func connection(_ e: BuildingElement, _ b: SampleBuilding) -> Connection {
        var targets = b.feeds(e.id).compactMap { b.element($0) }
        if let l = e.load, let t = b.element(l) { targets.append(t) }
        if targets.isEmpty { return .unknown }
        if targets.contains(where: { $0.role == .equipment }) { return .feeder }
        return .branch
    }

    /// Big gear and big units: by volume (rules data).
    func isLarge(_ e: BuildingElement) -> Bool {
        let bb = e.shape.bounds
        let v = bb.max - bb.min
        return v.x * v.y * v.z >= rules.largeEquipmentMinCubicMetres
    }

    /// A panel fed straight from gear (not from another panel) is a house panel.
    func isHousePanel(_ e: BuildingElement, _ b: SampleBuilding) -> Bool {
        var cur = e.upstream.flatMap { b.element($0) }
        var depth = 0
        while let c = cur, depth < 8 {
            if c.role == .equipment { return c.part != .panel }
            cur = c.upstream.flatMap { b.element($0) }
            depth += 1
        }
        return true
    }

    private func tier(_ e: BuildingElement, _ b: SampleBuilding, _ cache: inout [String: ViewTier]) -> ViewTier {
        if let t = cache[e.id] { return t }
        let t = compute(e, b, &cache)
        cache[e.id] = t
        return t
    }

    private func compute(_ e: BuildingElement, _ b: SampleBuilding, _ cache: inout [String: ViewTier]) -> ViewTier {
        let local: ViewTier = e.detail == .room ? .room : .floor
        switch e.system {
        case .architecture: return .architecture
        case .cabinets: return .floor
        case .furniture: return .room
        case .conveying: return .mains
        default: break
        }
        if elevatorBranches.contains(e.id) { return local }
        let crosses = crossing.contains(e.id)
        switch e.system {
        case .electrical, .lowVoltage:
            switch e.part {
            case .transformer, .switchgear: return .mains
            case .panel: return isHousePanel(e, b) ? .mains : .fullMEP
            case .disconnect:
                // An elevator's disconnect shows in both densities; other disconnects with Full MEP.
                let feedsElevator = b.downstream(of: e.id).contains { b.element($0)?.system == .conveying }
                return feedsElevator ? .mains : .fullMEP
            case .cableTray: return .mains
            case .raceway:
                switch Self.connection(e, b) {
                case .feeder:
                    // A feeder to gear, a house panel or large equipment is main raceway; to anything else, Full.
                    var targets = b.feeds(e.id).compactMap { b.element($0) }
                    if let l = e.load, let t = b.element(l) { targets.append(t) }
                    let reachesMains = targets.contains { $0.role == .equipment && tier($0, b, &cache) == .mains }
                    return reachesMains ? .mains : .fullMEP
                case .branch:
                    return crosses ? .fullMEP : local
                case .unknown:
                    if let n = e.nominal, n >= rules.feederMinInches { return .fullMEP }
                    return crosses ? .fullMEP : local
                }
            default:
                if e.role == .equipment { return isLarge(e) ? .mains : .fullMEP }
                return local
            }
        case .mechanical:
            switch e.part {
            case .mechanicalUnit: return isLarge(e) ? .mains : .fullMEP
            case .fan, .airTerminal: return .fullMEP
            case .duct:
                if e.role == .main, let n = e.nominal, n >= rules.mainDuctMinInches { return .mains }
                return .fullMEP   // every duct shows in Full MEP
            default:
                if e.role == .equipment { return isLarge(e) ? .mains : .fullMEP }
                return local
            }
        case .plumbing:
            switch e.part {
            case .pipe:
                guard let n = e.nominal else { return local }   // unknown size: never guessed (counted in the note)
                if e.role == .main, n >= rules.largePipeMinInches { return .mains }
                if e.role != .branch, n >= rules.plumbingMainMinInches { return .fullMEP }
                return crosses ? .fullMEP : local
            default:
                if e.role == .equipment { return .mains }
                return local
            }
        case .fire:
            switch e.part {
            case .pipe:
                let isMain: Bool
                switch e.role {
                case .main: isMain = true
                case .branch: isMain = false
                default: isMain = (e.nominal ?? 0) >= rules.fireMainMinInches
                }
                if isMain { return .mains }
                return crosses ? .fullMEP : local
            case .sprinklerHead: return .floor
            default:
                if e.role == .equipment { return .mains }
                return local
            }
        default:
            return local
        }
    }
}

extension ViewClassifier {
    /// Runs and pipes with no size that MEP leaves out (for the visible note).
    public func unknownSizeCount(_ b: SampleBuilding) -> Int {
        b.elements.filter { ($0.system == .plumbing || $0.system == .fire) && $0.part == .pipe && $0.nominal == nil }.count
    }
}

// MARK: - What a view may draw, and what it does draw

/// The active view's limit and what is drawn inside it. The renderer shows exactly `drawn`; the tests check it.
public struct ViewFilter: Sendable {
    public let building: SampleBuilding
    public let classifier: ViewClassifier

    private let servingByRoom: [String: Set<String>]

    public init(_ b: SampleBuilding, rules: ViewRules = .standard) {
        building = b
        classifier = ViewClassifier(b, rules: rules)
        var serving: [String: Set<String>] = [:]
        for e in b.elements {
            guard let n = e.room, let r = b.room(n), let u = e.upstream, let up = b.element(u) else { continue }
            if up.level == r.level, up.room == nil || up.room == n { serving[n, default: []].insert(u) }
        }
        servingByRoom = serving
    }

    /// The elements that serve a room from the same floor (its circuits, branch ducts and pipes), which stay
    /// drawn in the Room view.
    public func serving(_ room: String) -> Set<String> { servingByRoom[room] ?? [] }

    /// The view's limit: nothing outside this set is ever drawn, whatever the switches say.
    public func allows(_ e: BuildingElement, _ s: ViewState, prefs: LayerPrefs) -> Bool {
        guard let t = classifier.tier(e.id) else { return false }
        switch s.view {
        case .architectural:
            return t == .architecture
        case .mep:
            return t == .mains || (s.density == .full && t == .fullMEP)
        case .floor:
            guard let l = s.level, e.level == l else { return false }
            return t != .room && e.system != .furniture
        case .room:
            guard let n = s.room, let r = building.room(n) else { return false }
            if e.system == .furniture { return e.room == n && prefs.isOn(.furniture) }
            return e.room == n || (e.level == r.level && serving(n).contains(e.id))
        }
    }

    /// What the view draws with these switches: the view's own set, then the trade switches, then the limit
    /// last, so no switch, saved setting or older file can get past it.
    public func drawn(_ s: ViewState, prefs: LayerPrefs) -> Set<String> {
        var out = Set<String>()
        let offered = Set(s.view.layers)
        for e in building.elements {
            guard let t = classifier.tier(e.id) else { continue }
            // 1. The view's own contents.
            let wanted: Bool
            switch s.view {
            case .architectural: wanted = t == .architecture
            case .mep: wanted = t == .mains || (s.density == .full && t == .fullMEP)
            case .floor: wanted = e.level == s.level && (t == .mains || t == .fullMEP || t == .floor)
            case .room:
                guard let n = s.room else { wanted = false; break }
                // The room's own walls are drawn from its outline; its architecture objects are not repeated.
                wanted = (e.room == n && t != .architecture) || serving(n).contains(e.id)
            }
            guard wanted else { continue }
            // 2. The trade switches the view offers (a switch the view does not offer is off for its trade
            //    when that trade defaults off, e.g. furniture).
            if let layer = FieldLayer.of(e.system) {
                if offered.contains(layer) { if !prefs.isOn(layer) { continue } } else if layer == .furniture || layer == .cabinets { continue }
            }
            // 3. The limit, last.
            guard allows(e, s, prefs: prefs) else { continue }
            out.insert(e.id)
        }
        return out
    }
}
