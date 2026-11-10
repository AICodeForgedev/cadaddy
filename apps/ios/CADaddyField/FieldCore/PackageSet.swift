// CADaddy Field — a building's package set (`bim.export.ar {set: true}`): `building.json`, the index the app reads
// (schema `cadaddy.ar.building/1`), and the packages it names. The index says which packages each of the four views
// loads (the app never composes its own combination), where every level and room is, each package's content hash
// (Update pulls only what changed) and the connection graph (follow the system by element id, across floors,
// without loading any package). Everything here is pure data, shared by the app and the tests.

import Foundation
import simd

/// How sure a connection is, as the export says: the documents give it, a rule or default gave it, or nobody says.
public enum LinkState: String, Codable, Sendable, Equatable {
    case known, inferred, unknown

    public var title: String {
        switch self {
        case .known: return "Known"
        case .inferred: return "Inferred"
        case .unknown: return "Unknown"
        }
    }
}

/// One package of the set (a building view, a level or a room).
public struct SetPackage: Equatable, Sendable {
    public enum Kind: Equatable, Sendable { case architectural, mepMains, mepFull, level, room, other }
    public struct File: Equatable, Sendable {
        public var path: String
        public var bytes: Int64
        public var sha256: String
    }
    public var name: String
    public var view: String
    public var level: String?
    public var generic: Bool
    public var fieldAlignment: Bool
    public var glb: File?
    public var sidecar: File?
    public var usdz: File?
    public var contentHash: String
    public var triangles: Int
    public var nodes: Int
    /// The package's own anchor (its origin) in plan feet; room packages only.
    public var anchor: AnchorInfo?
    public var anchorFloorFeet: Double?
    public var isRoom: Bool
    /// What the index's `views` section uses this package for (set when the index is read).
    public var assigned: Kind?

    public var kind: Kind {
        if let assigned { return assigned }
        switch view {
        case "architectural": return .architectural
        case "mep-mains", "mains": return .mepMains
        case "mep-full", "full": return .mepFull
        case "floor", "level": return .level
        case "room": return .room
        default:
            if isRoom { return .room }
            if name.hasPrefix("levels/") { return .level }
            return .other
        }
    }
}

public struct SetLevel: Equatable, Sendable {
    public var name: String
    public var elevationFeet: Double
    public var heightFeet: Double
    /// Outline loops in plan feet.
    public var outline: [[SIMD2<Double>]]
    public var package: String?
}

public struct SetRoom: Equatable, Sendable {
    public var id: String
    public var number: String
    public var name: String
    public var label: String
    public var level: String
    public var minFeet: SIMD2<Double>
    public var maxFeet: SIMD2<Double>
    public var areaSF: Double?
    public var anchor: AnchorInfo?
    public var anchorFloorFeet: Double?
    public var package: String?
}

/// One connection of the graph: what an element is fed by, or what it serves.
public struct GraphLink: Equatable, Sendable {
    public var id: String?
    public var tag: String?
    public var via: String?
    public var circuit: String?
    public var system: String?
    public var why: String?
    public var state: LinkState
}

/// One element of the connection graph. Packages are indices into `PackageSetIndex.allPackages`.
public struct GraphElement: Equatable, Sendable {
    public var id: String
    public var kind: String
    public var tag: String?
    public var group: String
    public var level: String?
    public var system: String?
    public var role: String?
    public var state: LinkState?
    public var packages: [Int32]
    public var fedBy: [GraphLink]
    public var serves: [GraphLink]
}

/// The four views' packages, exactly as the index names them.
public struct SetViews: Equatable, Sendable {
    public var defaultView: FieldView
    public var defaultDensity: MEPDensity
    public var architectural: [String]
    public var mains: [String]
    public var full: [String]
    public var floorByLevel: [String: [String]]
    public var roomByRoom: [String: [String]]
}

/// `building.json`.
public struct PackageSetIndex: Sendable {
    public static let schema = "cadaddy.ar.building/1"

    public var title: String
    public var exported: String?
    public var originPlanFeet: SIMD2<Double>
    public var datumFeet: Double
    public var levels: [SetLevel]
    public var rooms: [SetRoom]
    /// Building and level packages first, then room packages (graph package indices point into this).
    public var allPackages: [SetPackage]
    public var views: SetViews
    /// Node groups: name → on by default.
    public var groups: [String: Bool]
    public var graph: [GraphElement]
    public var links: (known: Int, inferred: Int, unknown: Int)
    /// What the export left out, counted (`left_out`), e.g. pipe of unknown size.
    public var leftOut: [String: Int]

    private var packageByName: [String: Int] = [:]
    private var graphByID: [String: Int] = [:]
    /// Element id → the elements whose `fed_by` names it (downstream), by graph index.
    private var fedFrom: [String: [Int32]] = [:]
    private var roomByID: [String: Int] = [:]

    public func package(_ name: String) -> SetPackage? { packageByName[name].map { allPackages[$0] } }
    public func packageIndex(_ name: String) -> Int? { packageByName[name] }
    public func element(_ id: String) -> GraphElement? { graphByID[id].map { graph[$0] } }
    public func room(id: String) -> SetRoom? { roomByID[id].map { rooms[$0] } }
    public func levelIndex(_ name: String?) -> Int? { name.flatMap { n in levels.firstIndex { $0.name == n } } }
    public func downstreamIndices(of id: String) -> [Int32] { fedFrom[id] ?? [] }

    /// The packages a view state loads, exactly as `views` names them.
    public func packages(for s: ViewState) -> [String] {
        switch s.view {
        case .architectural: return views.architectural
        case .mep: return s.density == .full ? views.full : views.mains
        case .floor:
            guard let i = s.level, levels.indices.contains(i) else { return [] }
            return views.floorByLevel[levels[i].name] ?? []
        case .room:
            guard let r = s.room else { return [] }
            return views.roomByRoom[r] ?? []
        }
    }

    // MARK: frame

    /// Plan feet → the set's model frame (metres, Y up): x = plan x − origin x, z = −(plan y − origin y).
    public func modelPoint(planFeet p: SIMD2<Double>, elevationFeet e: Double) -> SIMD3<Float> {
        let k = Units.metersPerFoot
        return SIMD3(Float((p.x - originPlanFeet.x) * k), Float((e - datumFeet) * k), Float(-(p.y - originPlanFeet.y) * k))
    }

    /// Where a room package's origin (its anchor) sits in the building frame.
    public func roomOrigin(_ p: SetPackage) -> SIMD3<Float>? {
        guard let a = p.anchor, let x = a.planX, let y = a.planY else { return nil }
        let scale = (AnchorInfo.metersPerUnit(a.planUnits) ?? Units.metersPerFoot) / Units.metersPerFoot
        return modelPoint(planFeet: SIMD2(x * scale, y * scale), elevationFeet: p.anchorFloorFeet ?? 0)
    }

    // MARK: parsing

    /// Reads `building.json`. A file of another kind, a newer schema, or an older export with no `views` section
    /// throws a plain message; nothing is guessed.
    public static func parse(_ data: Data) throws -> PackageSetIndex {
        let raw: Raw
        do { raw = try JSONDecoder().decode(Raw.self, from: data) } catch {
            throw FieldError("building.json can't be read (damaged or not a CADaddy building index).")
        }
        guard let schema = raw.schema else { throw FieldError("This folder's building.json is not a CADaddy building index.") }
        guard schema == Self.schema else {
            if schema.hasPrefix("cadaddy.ar.building/") {
                throw FieldError("This building was exported by a newer CADaddy (\(schema)). Update CADaddy Field to open it.")
            }
            throw FieldError("This folder's building.json is not a CADaddy building index.")
        }
        guard let v = raw.views, v.architectural?.packages != nil || v.mep?.densities != nil else {
            throw FieldError("This building was exported by an older version of CADaddy, before the four views. Export it again with bim.export.ar {set: true} and update.")
        }
        var out = PackageSetIndex(
            title: (raw.title ?? "Building").clipped(),
            exported: raw.exported,
            originPlanFeet: SIMD2(raw.frame?.origin_plan_ft?.first ?? 0, raw.frame?.origin_plan_ft?.dropFirst().first ?? 0),
            datumFeet: raw.frame?.datum_elevation_ft ?? 0,
            levels: [], rooms: [], allPackages: [],
            views: SetViews(defaultView: .mep, defaultDensity: .mains, architectural: [], mains: [], full: [], floorByLevel: [:], roomByRoom: [:]),
            groups: [:], graph: [], links: (0, 0, 0), leftOut: [:])
        for l in (raw.levels ?? []).prefix(FieldLimits.maxSetLevels) {
            guard let name = l.name else { continue }
            let loops = (l.outline ?? []).prefix(64).map { loop in loop.prefix(10_000).compactMap { p in p.count >= 2 && p[0].isFinite && p[1].isFinite ? SIMD2(p[0], p[1]) : nil } }
            out.levels.append(SetLevel(name: name.clipped(), elevationFeet: finite(l.elevation_ft) ?? 0, heightFeet: max(finite(l.height_ft) ?? 0, 0), outline: loops, package: l.package))
        }
        out.levels.sort { $0.elevationFeet < $1.elevationFeet }
        for r in (raw.rooms ?? []).prefix(FieldLimits.maxSetRooms) {
            guard let id = r.id, let lv = r.level else { continue }
            let mn = r.bounds?.min ?? [], mx = r.bounds?.max ?? []
            guard mn.count >= 2, mx.count >= 2, mn.allSatisfy(\.isFinite), mx.allSatisfy(\.isFinite) else { continue }
            out.rooms.append(SetRoom(id: id, number: (r.number ?? "").clipped(), name: (r.name ?? "").clipped(), label: (r.label ?? r.number ?? id).clipped(),
                                     level: lv, minFeet: SIMD2(min(mn[0], mx[0]), min(mn[1], mx[1])), maxFeet: SIMD2(max(mn[0], mx[0]), max(mn[1], mx[1])),
                                     areaSF: finite(r.area_sf), anchor: r.anchor.map(\.info), anchorFloorFeet: finite(r.anchor?.floor_elevation_ft), package: r.package))
        }
        func pkg(_ p: RawPackage, room: Bool) -> SetPackage? {
            guard let name = p.name, UpdateManifest.safeRelativeName(name) != nil else { return nil }
            func file(_ f: RawFile?) -> SetPackage.File? {
                guard let f, let path = f.path, UpdateManifest.safeRelativeName(path) != nil, let b = f.bytes, b >= 0, let s = f.sha256 else { return nil }
                return SetPackage.File(path: path, bytes: b, sha256: s.lowercased())
            }
            return SetPackage(name: name, view: p.view ?? "", level: p.level, generic: p.generic ?? false, fieldAlignment: p.field_alignment ?? false,
                              glb: file(p.files?.glb), sidecar: file(p.files?.sidecar), usdz: file(p.files?.usdz), contentHash: p.content_hash ?? "",
                              triangles: max(p.triangles ?? 0, 0), nodes: max(p.nodes ?? 0, 0), anchor: p.frame?.anchor?.info,
                              anchorFloorFeet: finite(p.frame?.anchor?.floor_elevation_ft), isRoom: room)
        }
        for p in (raw.packages ?? []).prefix(FieldLimits.maxSetPackages) { if let s = pkg(p, room: false) { out.allPackages.append(s) } }
        for p in (raw.room_packages ?? []).prefix(FieldLimits.maxSetPackages) { if let s = pkg(p, room: true) { out.allPackages.append(s) } }
        for (i, p) in out.allPackages.enumerated() where out.packageByName[p.name] == nil { out.packageByName[p.name] = i }

        out.views.defaultView = FieldView(rawValue: v.default?.view ?? "mep") ?? .mep
        out.views.defaultDensity = MEPDensity(rawValue: v.default?.density ?? v.mep?.default_density ?? "mains") ?? .mains
        out.views.architectural = v.architectural?.packages ?? []
        out.views.mains = v.mep?.densities?["mains"] ?? []
        out.views.full = v.mep?.densities?["full"] ?? out.views.mains
        out.views.floorByLevel = v.floor?.by_level ?? [:]
        out.views.roomByRoom = v.room?.by_room ?? [:]
        // Each package's use, from the views section (both MEP packages say view "mep"; the index tells them apart).
        func assign(_ names: [String], _ k: SetPackage.Kind) {
            for n in names { if let i = out.packageByName[n], out.allPackages[i].assigned == nil { out.allPackages[i].assigned = k } }
        }
        assign(out.views.mains, .mepMains)
        assign(out.views.full, .mepFull)
        assign(out.views.architectural, .architectural)
        assign(out.views.floorByLevel.values.flatMap { $0 }, .level)
        assign(out.views.roomByRoom.values.flatMap { $0 }, .room)
        for (k, g) in raw.groups ?? [:] { out.groups[k] = g.default_on ?? true }
        out.leftOut = (raw.left_out ?? [:]).compactMapValues { $0.int }
        for (i, r) in out.rooms.enumerated() where out.roomByID[r.id] == nil { out.roomByID[r.id] = i }

        // The graph, compacted: package names to indices, links to small structs.
        let g = raw.graph?.elements ?? []
        out.graph.reserveCapacity(min(g.count, FieldLimits.maxSetGraphElements))
        func link(_ l: RawLink) -> GraphLink {
            GraphLink(id: l.id, tag: l.tag, via: l.via, circuit: l.circuit?.text, system: l.system, why: l.why, state: LinkState(rawValue: l.link ?? "") ?? .unknown)
        }
        for e in g.prefix(FieldLimits.maxSetGraphElements) {
            guard let id = e.id else { continue }
            let pk = (e.packages ?? []).prefix(64).compactMap { out.packageByName[$0].map(Int32.init) }
            out.graph.append(GraphElement(id: id, kind: e.kind ?? "", tag: e.tag, group: e.group ?? "base", level: e.level, system: e.system, role: e.role,
                                          state: e.link.flatMap(LinkState.init(rawValue:)), packages: pk,
                                          fedBy: (e.fed_by ?? []).prefix(64).map(link), serves: (e.serves ?? []).prefix(64).map(link)))
        }
        for (i, e) in out.graph.enumerated() {
            if out.graphByID[e.id] == nil { out.graphByID[e.id] = i }
            for f in e.fedBy { if let up = f.id, up != e.id { out.fedFrom[up, default: []].append(Int32(i)) } }
        }
        let l = raw.graph?.links
        out.links = (l?.known ?? 0, l?.inferred ?? 0, l?.unknown ?? 0)
        return out
    }

    private static func finite(_ d: Double?) -> Double? { d.flatMap { $0.isFinite ? $0 : nil } }

    // MARK: raw JSON (every key optional: a missing key is a missing value, never a crash)

    struct Raw: Decodable {
        var schema: String?
        var title: String?
        var exported: String?
        var frame: RawFrame?
        var levels: [RawLevel]?
        var rooms: [RawRoom]?
        var packages: [RawPackage]?
        var room_packages: [RawPackage]?
        var views: RawViews?
        var groups: [String: RawGroup]?
        var graph: RawGraph?
        var left_out: [String: Lenient]?
    }
    struct RawFrame: Decodable { var origin_plan_ft: [Double]?; var datum_elevation_ft: Double? }
    struct RawLevel: Decodable { var name: String?; var elevation_ft: Double?; var height_ft: Double?; var outline: [[[Double]]]?; var package: String? }
    struct RawBounds: Decodable { var min: [Double]?; var max: [Double]? }
    struct RawAnchor: Decodable {
        var kind: String?, name: String?, plan_x: Double?, plan_y: Double?, plan_units: String?, level: String?, room: String?, floor_elevation_ft: Double?
        var info: AnchorInfo { AnchorInfo(kind: kind ?? "named_point", name: name ?? "anchor", planX: plan_x, planY: plan_y, planUnits: plan_units ?? "ft", level: level, room: room) }
    }
    struct RawRoom: Decodable {
        var id: String?, number: String?, name: String?, label: String?, level: String?, bounds: RawBounds?, area_sf: Double?, anchor: RawAnchor?, package: String?
    }
    struct RawFile: Decodable { var path: String?; var bytes: Int64?; var sha256: String? }
    struct RawFiles: Decodable { var glb: RawFile?; var sidecar: RawFile?; var usdz: RawFile? }
    struct RawPackageFrame: Decodable { var anchor: RawAnchor? }
    struct RawPackage: Decodable {
        var name: String?, view: String?, level: String?, generic: Bool?, field_alignment: Bool?, files: RawFiles?, content_hash: String?
        var triangles: Int?, nodes: Int?, frame: RawPackageFrame?
    }
    struct RawViewList: Decodable { var packages: [String]? }
    struct RawDefault: Decodable { var view: String?; var density: String? }
    struct RawMEP: Decodable { var default_density: String?; var densities: [String: [String]]? }
    struct RawFloor: Decodable { var by_level: [String: [String]]? }
    struct RawRoomViews: Decodable { var by_room: [String: [String]]? }
    struct RawViews: Decodable {
        var `default`: RawDefault?, architectural: RawViewList?, mep: RawMEP?, floor: RawFloor?, room: RawRoomViews?
    }
    struct RawGroup: Decodable { var default_on: Bool?; var always_on: Bool? }
    struct RawLinkCounts: Decodable { var known: Int?; var inferred: Int?; var unknown: Int? }
    struct RawLink: Decodable { var id: String?, tag: String?, via: String?, circuit: Lenient?, system: String?, why: String?, link: String? }
    struct RawElement: Decodable {
        var id: String?, kind: String?, tag: String?, group: String?, level: String?, system: String?, role: String?, link: String?
        var packages: [String]?, fed_by: [RawLink]?, serves: [RawLink]?
    }
    struct RawGraph: Decodable { var elements: [RawElement]?; var links: RawLinkCounts? }

    /// A JSON value read as text or a whole number, whatever its type (a circuit may be "7" or 7).
    struct Lenient: Decodable {
        var text: String?
        var int: Int?
        init(from d: Decoder) throws {
            let c = try d.singleValueContainer()
            if let s = try? c.decode(String.self) { text = s; int = Int(s) } else if let i = try? c.decode(Int.self) { int = i; text = String(i) } else if let x = try? c.decode(Double.self), x.isFinite { text = String(x); int = Int(exactly: x.rounded()) } else { text = nil; int = nil }
        }
    }
}

// MARK: - Follow the system on the graph

extension PackageSetIndex {
    static let maxTraceDepth = 32
    static let maxDownstream = 20_000

    /// The step name in the path strip, from the element's role and kind.
    static func stepName(_ e: GraphElement) -> String {
        switch e.role ?? "" {
        case "gear": return "Gear"
        case "panel": return "Panel"
        case "feeder": return "Feeder"
        case "riser": return "Riser"
        case "main": return "Main"
        case "branch": return "Branch"
        case "hoistway": return "Elevator"
        case "elevator_disconnect", "disconnect": return "Disconnect"
        case "equipment": return "Equipment"
        default: return e.kind.isEmpty ? "Object" : e.kind.replacingOccurrences(of: "_", with: " ").capitalized
        }
    }

    /// From `id` up to its source, by element id, across floors. Each step says how the link to the next one is
    /// known (from the element's `fed_by`). A link with no element (a panel named but not drawn) is the last step,
    /// by its tag; a link nobody gives ends the path with a plain "unknown" note, never a guess.
    public func trace(_ id: String) -> (steps: [TraceStep], note: String?) {
        guard var cur = element(id) else { return ([], "This object is not in the building's index.") }
        var steps: [TraceStep] = []
        var seen = Set<String>()
        while steps.count < Self.maxTraceDepth, seen.insert(cur.id).inserted {
            let up = cur.fedBy.first { $0.id != nil && $0.id != cur.id } ?? cur.fedBy.first
            steps.append(TraceStep(id: cur.id, link: Self.stepName(cur), tag: cur.tag ?? cur.kind, level: levelIndex(cur.level) ?? -1, state: up?.state ?? .unknown))
            guard let up else {
                let isSource = ["gear", "equipment"].contains(cur.role ?? "") || cur.kind == "switchboard" || cur.kind == "transformer"
                if steps.count == 1 || !isSource {
                    let what = cur.tag.map { "\(cur.kind.replacingOccurrences(of: "_", with: " ")) \($0)" } ?? cur.kind.replacingOccurrences(of: "_", with: " ")
                    return (steps, "Unknown: the documents don't say what feeds \(what).")
                }
                steps[steps.count - 1].state = nil
                return (steps, nil)
            }
            if let next = up.id.flatMap({ element($0) }) {
                cur = next
            } else {
                // Named by tag, not an element of the model (or not kept): the last step, not tappable.
                var why = ""
                if let c = up.circuit { why = " circuit \(c)" }
                steps.append(TraceStep(id: "", link: up.tag == nil ? "Source" : "Panel", tag: (up.tag ?? up.via ?? "?") + why, level: -1, state: nil))
                return (steps, up.state == .unknown ? "Unknown: \(up.why ?? "the documents don't say").": nil)
            }
        }
        return (steps, nil)
    }

    /// Everything fed from `id`, directly or through others, by graph index (not including `id`).
    public func downstream(of id: String) -> Set<String> {
        var out = Set<String>()
        var stack = downstreamIndices(of: id)
        while let n = stack.popLast(), out.count < Self.maxDownstream {
            let e = graph[Int(n)]
            guard out.insert(e.id).inserted else { continue }
            stack.append(contentsOf: downstreamIndices(of: e.id))
        }
        return out
    }

    /// The tap rule on the graph: equipment and mains light what they feed; anything else traces to its source.
    public func focus(_ id: String) -> SystemFocus {
        let (steps, note) = trace(id)
        let kids = downstream(of: id)
        let role = element(id)?.role ?? ""
        let isSource = !kids.isEmpty && ["gear", "panel", "equipment", "feeder", "main", "riser", "hoistway", "elevator_disconnect"].contains(role)
        var lit = Set<String>([id])
        lit.formUnion(steps.map(\.id).filter { !$0.isEmpty })
        if isSource { lit.formUnion(kids) }
        let trace: TraceResult = steps.count <= 1 && note != nil && !isSource ? .noKnownConnection(note ?? "") : .chain(steps)
        return SystemFocus(tapped: id, lit: lit, trace: trace, showsDownstream: isSource, note: steps.count > 1 || isSource ? note : nil, downstreamCount: kids.count)
    }
}
