// CADaddy Field — a building's package set on the device: reads one package (checked against the index: present,
// the right size and checksum, the right schema and package), turns its nodes into building elements in the set's
// frame, and keeps what is loaded inside a stated memory budget. Levels and rooms load on demand; what is not in
// view is released first when the budget is reached.
//
// The four views draw exactly the packages building.json names: each package's copy of an element is its own
// object (a level's walls are cut at 4 ft, a room's are clipped to the room), keyed `id|package` for level and
// room packages and by the element id itself in the building packages. Marks and the graph use the element id.

import CryptoKit
import Foundation
import simd

/// One package read from disk: its elements (geometry baked into the building frame) and what it holds in memory.
public struct LoadedPackage: Sendable {
    public var name: String
    public var kind: SetPackage.Kind
    public var elements: [BuildingElement]
    /// Bytes of vertex and index data (what the memory budget counts).
    public var bytes: Int
    /// Nodes left out on purpose: overhead (ceilings and roof, off by default in the floor view) and nodes with no
    /// triangles.
    public var skipped: Int
    public var triangles: Int
}

/// The memory rule for an iPhone: the building packages in view, plus levels and rooms on demand, inside
/// `budgetBytes` of package geometry. When a load goes over, the least recently used level and room packages
/// that are not in view are released first; packages in view are never released.
public struct PackageCache: Sendable {
    /// Default: 320 MB of package geometry. RealityKit's GPU copy is about the same again, so the building screen
    /// stays near 1 GB of footprint at worst, inside what iOS gives a foreground app on a 6 GB iPhone.
    public static let defaultBudget = 320 * 1024 * 1024
    /// At most this many level and room packages kept besides those in view, whatever the budget says.
    public static let maxSpareLevels = 2
    public static let maxSpareRooms = 4

    public var budgetBytes: Int
    public private(set) var loaded: [String: LoadedPackage] = [:]
    private var lastUse: [String: Int] = [:]
    private var clock = 0

    public init(budgetBytes: Int = PackageCache.defaultBudget) { self.budgetBytes = budgetBytes }

    public var bytes: Int { loaded.values.reduce(0) { $0 + $1.bytes } }
    public func has(_ name: String) -> Bool { loaded[name] != nil }

    public mutating func touch(_ names: [String]) { for n in names where loaded[n] != nil { clock += 1; lastUse[n] = clock } }

    /// Adds `p`; returns the packages released to stay inside the budget (never one of `inView`).
    @discardableResult
    public mutating func insert(_ p: LoadedPackage, inView: Set<String>) -> [String] {
        loaded[p.name] = p
        touch([p.name])
        return trim(inView: inView)
    }

    /// Releases what is over the budget or the spare counts, least recently used first, never what is in view.
    public mutating func trim(inView: Set<String>) -> [String] {
        var released: [String] = []
        func spare(_ k: SetPackage.Kind) -> [String] {
            loaded.values.filter { $0.kind == k && !inView.contains($0.name) }.map(\.name).sorted { (lastUse[$0] ?? 0) < (lastUse[$1] ?? 0) }
        }
        for (k, cap) in [(SetPackage.Kind.level, Self.maxSpareLevels), (.room, Self.maxSpareRooms)] {
            let s = spare(k)
            for n in s.prefix(max(s.count - cap, 0)) { loaded[n] = nil; lastUse[n] = nil; released.append(n) }
        }
        while bytes > budgetBytes {
            let candidates = loaded.values.filter { !inView.contains($0.name) }.map(\.name).sorted { (lastUse[$0] ?? 0) < (lastUse[$1] ?? 0) }
            guard let n = candidates.first else { break }
            loaded[n] = nil; lastUse[n] = nil; released.append(n)
        }
        return released
    }

    public mutating func remove(_ name: String) { loaded[name] = nil; lastUse[name] = nil }
}

/// A set's levels, rooms and footprint in the app's building frame, and the package reader.
public struct PackageSetModel: Sendable {
    public let index: PackageSetIndex
    /// The folder holding building.json and the package files.
    public let root: URL
    public let levels: [BuildingLevel]
    public let rooms: [BuildingRoom]
    public let footprintMin: SIMD2<Float>
    public let footprintMax: SIMD2<Float>

    public init(root: URL) throws {
        let url = root.appendingPathComponent("building.json")
        guard FileManager.default.fileExists(atPath: url.path) else { throw FieldError("This folder has no building.json.") }
        let size = (try? FileManager.default.attributesOfItem(atPath: url.path)[.size] as? NSNumber)?.int64Value ?? 0
        guard size <= FieldLimits.maxBuildingIndexBytes else { throw FieldError("building.json is too large to open on this device.") }
        let data: Data
        do { data = try Data(contentsOf: url, options: .mappedIfSafe) } catch { throw FieldError("building.json can't be read on this device.") }
        try self.init(index: PackageSetIndex.parse(data), root: root)
    }

    public init(index: PackageSetIndex, root: URL) {
        self.index = index
        self.root = root
        let k = Float(Units.metersPerFoot)
        var levels: [BuildingLevel] = []
        for (i, l) in index.levels.enumerated() {
            levels.append(BuildingLevel(index: i, name: l.name, short: Self.shortName(l.name), elevation: Float(l.elevationFeet - index.datumFeet) * k, height: Float(l.heightFeet) * k))
        }
        self.levels = levels
        var rooms: [BuildingRoom] = []
        for r in index.rooms {
            guard let li = index.levelIndex(r.level) else { continue }
            let a = index.modelPoint(planFeet: r.minFeet, elevationFeet: 0), b = index.modelPoint(planFeet: r.maxFeet, elevationFeet: 0)
            let pkg = r.package.flatMap { index.package($0) }
            rooms.append(BuildingRoom(number: r.id, name: r.name, level: li, min: SIMD2(min(a.x, b.x), min(a.z, b.z)), max: SIMD2(max(a.x, b.x), max(a.z, b.z)),
                                      shownNumber: r.number.isEmpty ? nil : r.number, hasPackage: pkg?.fieldAlignment ?? false))
        }
        self.rooms = rooms
        var lo = SIMD2<Float>(repeating: .greatestFiniteMagnitude), hi = -lo
        for l in index.levels { for loop in l.outline { for p in loop { let q = index.modelPoint(planFeet: p, elevationFeet: 0); lo = simd_min(lo, SIMD2(q.x, q.z)); hi = simd_max(hi, SIMD2(q.x, q.z)) } } }
        if lo.x > hi.x { for r in rooms { lo = simd_min(lo, r.min); hi = simd_max(hi, r.max) } }
        if lo.x > hi.x { lo = .zero; hi = SIMD2(10, 10) }
        footprintMin = lo
        footprintMax = hi
    }

    /// What the floor rail shows: "Level 2" → "2", "L1" → "1", "Roof" → "Roof".
    static func shortName(_ n: String) -> String {
        let t = n.trimmingCharacters(in: .whitespaces)
        for p in ["level ", "lvl ", "floor ", "l"] where t.lowercased().hasPrefix(p) {
            let rest = String(t.dropFirst(p.count)).trimmingCharacters(in: .whitespaces)
            if !rest.isEmpty, rest.count <= 4, rest.first?.isNumber == true { return rest }
        }
        return t.count <= 5 ? t : String(t.prefix(4))
    }

    /// What a model opens with: the default view's packages and the architectural one (the reveal's exterior).
    public static func openingPackages(_ index: PackageSetIndex) -> [String] {
        var names = index.packages(for: ViewState(view: index.views.defaultView == .architectural ? .architectural : .mep, density: index.views.defaultDensity))
        for n in index.views.architectural where !names.contains(n) { names.append(n) }
        return names
    }

    /// Reads a set and its opening packages off the main thread. The default view's packages must load (otherwise
    /// the set does not open, and says why); the architectural one may fail (the reveal is then skipped, said).
    public static func open(root: URL) async throws -> (PackageSetModel, [LoadedPackage], [String: String]) {
        try await Task.detached(priority: .userInitiated) {
            let m = try PackageSetModel(root: root)
            var out: [LoadedPackage] = [], failed: [String: String] = [:]
            let must = Set(m.index.packages(for: ViewState(view: m.index.views.defaultView == .architectural ? .architectural : .mep, density: m.index.views.defaultDensity)))
            for n in openingPackages(m.index) {
                do { out.append(try m.load(n)) } catch {
                    let msg = (error as? FieldError)?.message ?? "\(m.title(of: n)) can't be read."
                    if must.contains(n) { throw FieldError(msg) }
                    failed[n] = msg
                }
            }
            return (m, out, failed)
        }.value
    }

    /// An empty building: levels, rooms and footprint, no elements yet.
    public func building(with packages: [LoadedPackage]) -> SampleBuilding {
        let order: [SetPackage.Kind] = [.mepMains, .mepFull, .architectural, .other, .level, .room]
        var seen = Set<String>()
        var els: [BuildingElement] = []
        els.reserveCapacity(packages.reduce(0) { $0 + $1.elements.count })
        for p in packages.sorted(by: { (order.firstIndex(of: $0.kind) ?? 9, $0.name) < (order.firstIndex(of: $1.kind) ?? 9, $1.name) }) {
            for e in p.elements {
                // Each package's copy is its own object: an element two building packages share (a floor is MEP's
                // context and Architectural's slab) keeps the id in the first and is keyed by its package in the next.
                if seen.insert(e.id).inserted { els.append(e); continue }
                var copy = e
                copy.id = "\(e.sourceID)|\(p.name)"
                if seen.insert(copy.id).inserted { els.append(copy) }
            }
        }
        let len = footprintMax.x - footprintMin.x
        return SampleBuilding(name: index.title, levels: levels, rooms: rooms, elements: els, footprintMin: footprintMin, footprintMax: footprintMax,
                              bay: min(max(len / 14, 2), 8), isPackageSet: true)
    }

    /// The package's name as the app says it ("Level 2", "Room 101", "the MEP mains model").
    public func title(of name: String) -> String {
        guard let p = index.package(name) else { return name }
        switch p.kind {
        case .architectural: return "The architectural model"
        case .mepMains: return "The MEP mains model"
        case .mepFull: return "The full MEP model"
        case .level: return p.level.map { "Level \(Self.shortName($0) == $0 ? $0 : Self.shortName($0))" } ?? "A level"
        case .room:
            if let r = index.rooms.first(where: { $0.package == name }) { return "Room \(r.label)" }
            return "A room"
        case .other: return name
        }
    }

    // MARK: reading a package

    /// Reads, checks and converts one package. Every failure is a plain message naming the package; nothing else
    /// of the building is touched.
    public func load(_ name: String) throws -> LoadedPackage {
        let what = title(of: name)
        guard let p = index.package(name) else { throw FieldError("\(what) isn't in this building's index.") }
        guard let f = p.glb else { throw FieldError("\(what) has no model in this building's index.") }
        let url = root.appendingPathComponent(f.path)
        guard FileManager.default.fileExists(atPath: url.path) else {
            throw FieldError("\(what) is missing on this phone. Update to fetch it again; the rest of the building still works.")
        }
        let data: Data
        do { data = try Data(contentsOf: url, options: .mappedIfSafe) } catch { throw FieldError("\(what) can't be read on this phone.") }
        guard Int64(data.count) <= FieldLimits.maxModelBytes else { throw FieldError("\(what) is too large to open on this phone.") }
        guard Int64(data.count) == f.bytes, Self.sha256(data) == f.sha256 else {
            throw FieldError("\(what) is damaged (it doesn't match its checksum). Update to fetch it again; the rest of the building still works.")
        }
        let model: FieldModel
        do { model = try GLBReader.read(data: data) } catch {
            throw FieldError("\(what) can't be read: \((error as? FieldError)?.message ?? "the file is damaged").")
        }
        guard model.header["schema"] == "cadaddy.ar/1", let named = model.header["package.name"] else {
            throw FieldError("\(what) was written by an older version of CADaddy. Export the building again and update; the rest of the building still works.")
        }
        guard named == name else { throw FieldError("\(what)'s file holds another package (\(named)). Update to fetch it again.") }
        return convert(model, package: p)
    }

    static func sha256(_ d: Data) -> String { SHA256.hash(data: d).map { String(format: "%02x", $0) }.joined() }

    func convert(_ model: FieldModel, package p: SetPackage) -> LoadedPackage {
        let kind = p.kind
        let offset: SIMD3<Float> = kind == .room ? (index.roomOrigin(p) ?? .zero) : .zero
        let roomID = kind == .room ? index.rooms.first(where: { $0.package == p.name })?.id : nil
        let detail: DetailScope = kind == .room ? .room : (kind == .level ? .floor : .building)
        let packageLevel = index.levelIndex(p.level)
        var out: [BuildingElement] = []
        var bytes = 0, skipped = 0, tris = 0
        out.reserveCapacity(model.elements.count)
        for fe in model.elements {
            let info = fe.info
            let x = info.extra
            if x["overhead"] == "true" { skipped += 1; continue }
            guard let mesh = Self.bake(fe, model, offset: offset) else { skipped += 1; continue }
            bytes += mesh.bytes
            tris += mesh.triangleCount
            let m = Self.meaning(info)
            let key = kind == .level || kind == .room ? "\(info.id)|\(p.name)" : info.id
            let level = index.levelIndex(x["level"]) ?? packageLevel ?? 0
            out.append(BuildingElement(
                id: key, kind: m.kindTitle, tag: info.tag ?? x["type_mark"] ?? "", system: m.system, role: m.role, level: level, room: roomID,
                shape: .mesh(mesh), mountingHeight: info.mountingHeight.map(Float.init), catalog: info.catalog, circuit: info.circuit, panel: info.panel,
                size: x["size"], upstream: nil, detail: detail, part: m.part, nominal: nil, load: nil, link: m.link, sourceID: info.id, package: p.name,
                groupLayer: m.layer, isBaseGroup: m.base, inferredNote: x["inferred"]))
        }
        return LoadedPackage(name: p.name, kind: kind, elements: out, bytes: bytes, skipped: skipped, triangles: tris)
    }

    /// The element's triangles in the building frame (its node transform, each part's, then the package offset).
    static func bake(_ e: FieldElement, _ model: FieldModel, offset: SIMD3<Float>) -> ElementMesh? {
        var pos: [SIMD3<Float>] = [], nor: [SIMD3<Float>] = [], idx: [UInt32] = []
        var color: SIMD4<Float>?
        for part in e.parts {
            guard model.meshes.indices.contains(part.mesh) else { continue }
            let m = e.transform * part.transform
            let n3 = simd_float3x3(SIMD3(m.columns.0.x, m.columns.0.y, m.columns.0.z), SIMD3(m.columns.1.x, m.columns.1.y, m.columns.1.z), SIMD3(m.columns.2.x, m.columns.2.y, m.columns.2.z))
            for prim in model.meshes[part.mesh].primitives {
                guard pos.count + prim.positions.count < Int(UInt32.max) else { break }
                let base = UInt32(pos.count)
                if color == nil { color = prim.color }
                for q in prim.positions { let w = m * SIMD4(q, 1); pos.append(SIMD3(w.x, w.y, w.z) + offset) }
                if prim.normals.count == prim.positions.count {
                    for n in prim.normals { let v = n3 * n; let l = simd_length(v); nor.append(l > 1e-6 ? v / l : SIMD3(0, 1, 0)) }
                } else {
                    nor.append(contentsOf: repeatElement(SIMD3(0, 1, 0), count: prim.positions.count))
                }
                for i in prim.indices where Int(i) < prim.positions.count { idx.append(base + i) }
            }
        }
        guard !idx.isEmpty, pos.allSatisfy({ $0.x.isFinite && $0.y.isFinite && $0.z.isFinite }) else { return nil }
        return ElementMesh(positions: pos, normals: nor, indices: idx, color: color ?? SIMD4(0.8, 0.8, 0.8, 1))
    }

    struct Meaning {
        var system: BuildingSystem
        var role: ElementRole
        var part: ElementPart
        var layer: FieldLayer?
        var base: Bool
        var kindTitle: String
        var link: String
    }

    /// What a node is, from the export's own words (group, trade, kind, role, category), never a tag or a name.
    static func meaning(_ info: ElementInfo) -> Meaning {
        let x = info.extra
        let group = (x["group"] ?? "").lowercased()
        let trade = (info.trade ?? "").lowercased()
        let kind = (info.kind ?? "").lowercased()
        let role = (x["role"] ?? "").lowercased()
        let category = (x["category"] ?? "").lowercased()
        let layer: FieldLayer?
        switch group {
        case "mechanical": layer = .mechanical
        case "electrical": layer = .electrical
        case "plumbing": layer = .plumbing
        case "cabinets": layer = .cabinets
        case "furniture": layer = .furniture
        case "base": layer = nil
        default: layer = FieldLayer.of(trade: info.trade, kind: info.kind)
        }
        let base = group == "base" || (group.isEmpty && layer == nil)
        var system: BuildingSystem
        if role == "hoistway" || kind == "hoistway" { system = .conveying } else if trade.contains("fire") && trade.contains("protect") || category.contains("sprinkler") {
            system = .fire
        } else {
            switch layer {
            case .mechanical?: system = .mechanical
            case .electrical?: system = trade.contains("low") || trade.contains("alarm") || trade.contains("telecom") || trade.contains("data") ? .lowVoltage : .electrical
            case .plumbing?: system = .plumbing
            case .cabinets?: system = .cabinets
            case .furniture?: system = .furniture
            case nil: system = .architecture
            }
        }
        let part: ElementPart
        switch (kind, role) {
        case ("room", _), (_, "room_tile"): part = .roomTile
        case ("exterior", _), (_, "exterior"): part = .exterior
        case ("outline", _), (_, "outline"): part = .outline
        case ("wall", _), ("parapet", _), ("railing", _), ("solid", _): part = .wall
        case ("floor", _), ("ceiling", _), ("roof", _), ("slab", _): part = .slab
        case ("door", _): part = .door
        case ("window", _): part = .window
        case ("stair", _), ("stairs", _): part = .stair
        case (_, "hoistway"), ("hoistway", _): part = .hoistway
        case (_, "elevator_machine"): part = .elevatorMachine
        case (_, "elevator_disconnect"), (_, "disconnect"), ("disconnect", _): part = .disconnect
        case (_, "panel"), ("panel", _), ("panelboard", _): part = .panel
        case (_, "gear"), ("switchboard", _), ("switchgear", _): part = .switchgear
        case ("transformer", _): part = .transformer
        case ("conduit", _), ("raceway", _), ("cable", _), ("wire", _): part = .raceway
        case ("cable_tray", _), ("tray", _): part = .cableTray
        case ("duct", _): part = .duct
        case ("pipe", _): part = .pipe
        case ("light_fixture", _), ("luminaire", _), ("light", _): part = .light
        case ("casework", _): part = .casework
        case ("furniture", _): part = .furniture
        default:
            if category.contains("air terminal") { part = .airTerminal } else if category.contains("sprinkler") || kind.contains("sprinkler") { part = .sprinklerHead } else if group == "plumbing" && (kind == "fixture" || category.contains("fixture")) {
                part = .fixture
            } else if kind == "equipment" || role == "equipment" {
                part = system == .mechanical ? (category.contains("fan") ? .fan : .mechanicalUnit) : (category.contains("pump") || kind.contains("pump") ? .pump : .mechanicalUnit)
            } else { part = .device }
        }
        let r: ElementRole
        switch role {
        case "main", "riser", "feeder": r = .main
        case "branch": r = .branch
        case "gear", "panel", "equipment", "hoistway", "elevator_disconnect", "disconnect", "elevator_machine": r = .equipment
        default:
            switch part {
            case .casework: r = .casework
            case .furniture: r = .furniture
            case .fixture: r = .fixture
            case .transformer, .switchgear, .panel, .disconnect, .mechanicalUnit, .fan, .pump, .hoistway, .elevatorMachine: r = .equipment
            case .duct, .pipe, .raceway, .cableTray: r = .unknown
            default: r = base ? .shell : .terminal
            }
        }
        let titles: [ElementPart: String] = [.panel: "Panel", .switchgear: "Switchboard", .transformer: "Transformer", .disconnect: "Disconnect",
                                             .hoistway: "Elevator", .light: "Light", .raceway: "Conduit", .duct: "Duct", .pipe: "Pipe", .roomTile: "Room"]
        let title = titles[part] ?? (kind.isEmpty ? "Object" : kind.replacingOccurrences(of: "_", with: " ").capitalized)
        let link: String
        switch (r, part) {
        case (.equipment, .panel): link = "Panel"
        case (.equipment, .switchgear), (.equipment, .transformer): link = "Gear"
        case (.equipment, _): link = "Equipment"
        case (.main, _): link = role == "feeder" ? "Feeder" : (role == "riser" ? "Riser" : "Main")
        case (.branch, _): link = "Branch"
        default: link = title
        }
        return Meaning(system: system, role: r, part: part, layer: layer, base: base, kindTitle: title, link: link)
    }
}
