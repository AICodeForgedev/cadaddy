// CADaddy Field — a made-up four-storey sample building for the building / floor / room design prototype.
// Everything here is invented: the name, room numbers, panel and equipment tags, sizes and catalog numbers.
// It is generated from a handful of generic rules (bay width, storey heights, one riser shaft, one home run per
// room), so it stays small enough to read and big enough to show the experience at every scope.
//
// Frame: metres, Y up. X runs along the building's length (0…56), Z across its depth (0…18).

import Foundation
import simd

/// A building system. Colours, layers and tracing are per system.
public enum BuildingSystem: String, CaseIterable, Sendable {
    case architecture, electrical, lowVoltage = "low-voltage", mechanical, plumbing, fire, cabinets, furniture
    /// Elevators: the hoistway and its machine. Its power (disconnect, feeder, branches) is electrical.
    case conveying
}

/// What an element physically is, as an export's object class says it (not its tag or name). The view rules
/// (FieldViews) decide what each view draws from this, the role, the size and the connections.
public enum ElementPart: String, Sendable, CaseIterable {
    case transformer, switchgear, panel, disconnect, mechanicalUnit, fan, airTerminal, pump, waterHeater, service
    case device, light, control, fixture, sprinklerHead
    case cableTray, raceway, duct, pipe
    case hoistway, elevatorMachine
    case door, window, stair, casework, furniture
    /// A package set's architecture: walls, floor plates and ceilings, a room's floor tile, the solid exterior the
    /// opening sweeps away, and MEP's one outline.
    case wall, slab, roomTile, exterior, outline
}

/// What an element is to its system: big source equipment, a main or feeder, a branch, or an end device.
public enum ElementRole: String, Sendable {
    case equipment, main, branch, terminal, fixture, casework, furniture, shell
    /// The model does not say what it is to its system (no role given).
    case unknown
}

/// The closest scope at which an element is drawn. Building shows only mains and big equipment; a floor adds its
/// branches, panels, lights and rooms; a room adds everything in it.
public enum DetailScope: Int, Comparable, Sendable {
    case building = 0, floor = 1, room = 2
    public static func < (a: DetailScope, b: DetailScope) -> Bool { a.rawValue < b.rawValue }
}

public enum RunProfile: Equatable, Sendable {
    case round(radius: Float)
    case rect(width: Float, height: Float)
}

/// Triangles from an exported package, already in the building frame (metres, Y up). A reference type: the
/// vertex arrays are shared, never copied with the element, and released with the package that brought them.
public final class ElementMesh: Equatable, Sendable {
    public let positions: [SIMD3<Float>]
    public let normals: [SIMD3<Float>]
    public let indices: [UInt32]
    /// Linear RGBA base colour of the first material.
    public let color: SIMD4<Float>
    public let min: SIMD3<Float>
    public let max: SIMD3<Float>

    public init(positions: [SIMD3<Float>], normals: [SIMD3<Float>], indices: [UInt32], color: SIMD4<Float>) {
        self.positions = positions; self.normals = normals; self.indices = indices; self.color = color
        var lo = SIMD3<Float>(repeating: .greatestFiniteMagnitude), hi = -lo
        for p in positions { lo = simd_min(lo, p); hi = simd_max(hi, p) }
        if positions.isEmpty { lo = .zero; hi = .zero }
        min = lo; max = hi
    }

    /// Bytes held by the vertex and index arrays (the memory budget counts these).
    public var bytes: Int { positions.count * 16 + normals.count * 16 + indices.count * 4 }
    public var triangleCount: Int { indices.count / 3 }

    public static func == (a: ElementMesh, b: ElementMesh) -> Bool { a === b }
}

public enum ElementShape: Equatable, Sendable {
    /// An axis-aligned solid (equipment, devices, fixtures, furniture).
    case box(center: SIMD3<Float>, size: SIMD3<Float>)
    /// A run of duct, pipe or conduit through its points, listed from the source to the load.
    case run(points: [SIMD3<Float>], profile: RunProfile)
    /// The exported triangles of a package's node (a real building's object).
    case mesh(ElementMesh)

    /// Where the camera looks and where a label sits: the box centre or the run's middle point.
    public var anchor: SIMD3<Float> {
        switch self {
        case let .box(c, _): return c
        case let .run(p, _):
            guard let first = p.first else { return .zero }
            return p.count > 1 ? (p[p.count / 2 - (p.count % 2 == 0 ? 1 : 0)] + p[p.count / 2]) / 2 : first
        case let .mesh(m): return (m.min + m.max) / 2
        }
    }

    public var bounds: (min: SIMD3<Float>, max: SIMD3<Float>) {
        switch self {
        case let .box(c, s): return (c - s / 2, c + s / 2)
        case let .run(p, profile):
            let r: Float
            switch profile { case let .round(rr): r = rr; case let .rect(w, h): r = max(w, h) / 2 }
            var lo = SIMD3<Float>(repeating: .greatestFiniteMagnitude), hi = -lo
            for q in p { lo = simd_min(lo, q); hi = simd_max(hi, q) }
            if p.isEmpty { return (.zero, .zero) }
            return (lo - r, hi + r)
        case let .mesh(m): return (m.min, m.max)
        }
    }
}

public struct BuildingLevel: Equatable, Sendable {
    public var index: Int
    /// "Level 2", "Roof", "Site".
    public var name: String
    /// What the floor rail shows: "2", "Roof", "Site".
    public var short: String
    /// Finished floor elevation, metres.
    public var elevation: Float
    /// Floor-to-floor height (0 for Site and Roof).
    public var height: Float
}

public struct BuildingRoom: Equatable, Sendable, Identifiable {
    public var id: String { number }
    /// The room's key (the sample's room number; a package set's room id).
    public var number: String
    public var name: String
    public var level: Int
    /// Plan rectangle (x, z) min and max.
    public var min: SIMD2<Float>
    public var max: SIMD2<Float>
    /// The number printed on the plans when the key is not it (a package set's rooms).
    public var shownNumber: String?
    /// True when this room has its own package (a real room: it can be aligned in the field).
    public var hasPackage = false

    public init(number: String, name: String, level: Int, min: SIMD2<Float>, max: SIMD2<Float>, shownNumber: String? = nil, hasPackage: Bool = false) {
        self.number = number; self.name = name; self.level = level; self.min = min; self.max = max; self.shownNumber = shownNumber; self.hasPackage = hasPackage
    }

    public var displayNumber: String { shownNumber ?? number }

    public var center: SIMD2<Float> { (min + max) / 2 }
    public func contains(_ p: SIMD3<Float>, margin: Float = 0.01) -> Bool {
        p.x >= min.x - margin && p.x <= max.x + margin && p.z >= min.y - margin && p.z <= max.y + margin
    }
}

public struct BuildingElement: Equatable, Sendable, Identifiable {
    public var id: String
    public var kind: String
    public var tag: String
    public var system: BuildingSystem
    public var role: ElementRole
    public var level: Int
    public var room: String?
    public var shape: ElementShape
    /// Mounting height above its level's floor, metres.
    public var mountingHeight: Float?
    public var catalog: String?
    public var circuit: String?
    public var panel: String?
    /// Size as the trade says it: "2000 A", "24×14 duct", "3/4\" EMT".
    public var size: String?
    /// The element that feeds this one (nil = a source, or no known connection).
    public var upstream: String?
    public var detail: DetailScope
    /// What it is (the export's object class).
    public var part: ElementPart
    /// Nominal size in inches for runs (pipe, raceway, the larger side of a duct); nil = the model does not say.
    public var nominal: Float?
    /// For a run: the element it ends at when that is not one of the elements naming it upstream (a feeder to a
    /// unit whose own upstream is its duct system). nil = only the upstream links say what it connects to.
    public var load: String?
    /// The step name in the follow-the-system path strip ("Outlet", "Circuit", "Panel", "Feeder", "Gear").
    public var link: String
    /// The export's element id (marks, the graph). The same as `id` except where one element is in several
    /// packages with its own geometry in each (a level's or a room's copy): there `id` names the copy.
    public var sourceID: String
    /// The package this copy came from (nil = the sample).
    public var package: String?
    /// The package node group's layer when the export gives one (it decides the switches, e.g. fire protection
    /// rides with Mechanical in an export); nil = the system's own layer.
    public var groupLayer: FieldLayer?
    /// A shell group node (always on, no switch).
    public var isBaseGroup = false
    /// The export's own words when it marks the object as inferred (no role and no size, shown anyway).
    public var inferredNote: String?

    public init(id: String, kind: String, tag: String, system: BuildingSystem, role: ElementRole, level: Int, room: String?, shape: ElementShape,
                mountingHeight: Float?, catalog: String?, circuit: String?, panel: String?, size: String?, upstream: String?, detail: DetailScope,
                part: ElementPart, nominal: Float?, load: String?, link: String, sourceID: String? = nil, package: String? = nil,
                groupLayer: FieldLayer? = nil, isBaseGroup: Bool = false, inferredNote: String? = nil) {
        self.id = id; self.kind = kind; self.tag = tag; self.system = system; self.role = role; self.level = level; self.room = room
        self.shape = shape; self.mountingHeight = mountingHeight; self.catalog = catalog; self.circuit = circuit; self.panel = panel
        self.size = size; self.upstream = upstream; self.detail = detail; self.part = part; self.nominal = nominal; self.load = load
        self.link = link; self.sourceID = sourceID ?? id; self.package = package; self.groupLayer = groupLayer; self.isBaseGroup = isBaseGroup
        self.inferredNote = inferredNote
    }

    public var title: String { "\(kind) \(tag)" }

    /// The trade switch this element answers to (nil = the shell, always on).
    public var layer: FieldLayer? {
        if isBaseGroup { return nil }
        return groupLayer ?? FieldLayer.of(system)
    }
}

public struct SampleBuilding: Sendable {
    public var name: String
    public var levels: [BuildingLevel]
    public var rooms: [BuildingRoom]
    public var elements: [BuildingElement]
    /// Building outline (x, z) for the shell and the shadow.
    public var footprintMin: SIMD2<Float>
    public var footprintMax: SIMD2<Float>
    /// Bay width of the exterior grid (the reveal sweeps bay by bay).
    public var bay: Float

    /// True for a real building opened from its package set (false for the design preview's invented sample).
    public var isPackageSet = false

    private var byID: [String: Int] = [:]
    private var children: [String: [String]] = [:]
    private var bySource: [String: [Int]] = [:]
    private var roomIndex: [String: Int] = [:]
    private var inRoom: [String: [Int]] = [:]

    public init(name: String, levels: [BuildingLevel], rooms: [BuildingRoom], elements: [BuildingElement],
                footprintMin: SIMD2<Float>, footprintMax: SIMD2<Float>, bay: Float, isPackageSet: Bool = false) {
        self.name = name; self.levels = levels; self.rooms = rooms; self.elements = elements
        self.footprintMin = footprintMin; self.footprintMax = footprintMax; self.bay = bay; self.isPackageSet = isPackageSet
        for (i, e) in elements.enumerated() where byID[e.id] == nil { byID[e.id] = i }
        for e in elements { if let u = e.upstream { children[u, default: []].append(e.id) } }
        for (i, e) in elements.enumerated() {
            if e.sourceID != e.id || isPackageSet { bySource[e.sourceID, default: []].append(i) }
            if let r = e.room { inRoom[r, default: []].append(i) }
        }
        for (i, r) in rooms.enumerated() where roomIndex[r.number] == nil { roomIndex[r.number] = i }
    }

    public func element(_ id: String) -> BuildingElement? { byID[id].flatMap { elements.indices.contains($0) ? elements[$0] : nil } }
    /// Every loaded copy of the export's element `source` (the sample: the element itself).
    public func copies(of source: String) -> [BuildingElement] {
        if let list = bySource[source] { return list.compactMap { elements.indices.contains($0) ? elements[$0] : nil } }
        return element(source).map { [$0] } ?? []
    }
    public func index(of id: String) -> Int? { byID[id] }
    public func feeds(_ id: String) -> [String] { children[id] ?? [] }
    public func level(_ i: Int) -> BuildingLevel? { levels.indices.contains(i) ? levels[i] : nil }
    public func room(_ number: String) -> BuildingRoom? { roomIndex[number].map { rooms[$0] } }
    public func rooms(on level: Int) -> [BuildingRoom] { rooms.filter { $0.level == level } }
    public func elements(in room: String) -> [BuildingElement] { (inRoom[room] ?? []).compactMap { elements.indices.contains($0) ? elements[$0] : nil } }

    /// Rail order, top to bottom: Roof, 4, 3, 2, 1, Site.
    public var railLevels: [BuildingLevel] { levels.reversed() }

    /// Guest-type rooms (everything but stairs, shafts and service rooms) — the "rooms" count in the stat line.
    public var occupiedRoomCount: Int { rooms.filter { !SampleBuilding.serviceNames.contains($0.name) }.count }
    static let serviceNames: Set<String> = ["Stair", "Electrical", "Shaft", "Mechanical", "Corridor", "Elevator"]

    public var height: Float { (levels.last?.elevation ?? 0) + 1.5 }
}

// MARK: - The generator

extension SampleBuilding {
    public static let sampleName = "Sample Building A"

    /// The made-up sample: four storeys, a roof with two rooftop units, a site transformer, one riser shaft.
    public static func sample() -> SampleBuilding {
        var g = Generator()
        g.build()
        return SampleBuilding(name: sampleName, levels: g.levels, rooms: g.rooms, elements: g.elements,
                              footprintMin: SIMD2(0, 0), footprintMax: SIMD2(Generator.length, Generator.depth), bay: Generator.bay)
    }

    struct Generator {
        static let length: Float = 56, depth: Float = 18, bay: Float = 4
        static let corridorSouth: Float = 7.6, corridorNorth: Float = 10.4
        static let shaftX: Float = 28   // the riser shaft and electrical closet bay (x 28…32, north side)

        var levels: [BuildingLevel] = []
        var rooms: [BuildingRoom] = []
        var elements: [BuildingElement] = []
        private var serial = 0

        mutating func id(_ prefix: String) -> String { serial += 1; return "\(prefix)-\(1000 + serial)" }

        mutating func add(_ kind: String, _ tag: String, _ system: BuildingSystem, _ role: ElementRole, level: Int, room: String? = nil,
                          _ shape: ElementShape, height: Float? = nil, catalog: String? = nil, circuit: String? = nil, panel: String? = nil,
                          size: String? = nil, upstream: String? = nil, detail: DetailScope, link: String,
                          part: ElementPart? = nil, nominal: Float? = nil, load: String? = nil) -> String {
            let prefix: String
            switch system {
            case .electrical, .lowVoltage: prefix = "E"
            case .mechanical: prefix = "M"
            case .plumbing: prefix = "P"
            case .fire: prefix = "F"
            case .cabinets: prefix = "C"
            case .furniture: prefix = "N"
            case .architecture: prefix = "A"
            case .conveying: prefix = "V"
            }
            let i = id(prefix)
            elements.append(BuildingElement(id: i, kind: kind, tag: tag, system: system, role: role, level: level, room: room, shape: shape,
                                            mountingHeight: height, catalog: catalog, circuit: circuit, panel: panel, size: size,
                                            upstream: upstream, detail: detail, part: part ?? Self.part(kind: kind, system: system, role: role, shape: shape),
                                            nominal: nominal, load: load, link: link))
            return i
        }

        /// The sample's object class from its kind (the sample is generated, so this table is the "export").
        static func part(kind: String, system: BuildingSystem, role: ElementRole, shape: ElementShape) -> ElementPart {
            switch kind {
            case "Transformer": return .transformer
            case "Switchboard": return .switchgear
            case "Panelboard": return .panel
            case "Disconnect": return .disconnect
            case "Rooftop Unit", "Outdoor Air Unit": return .mechanicalUnit
            case "Exhaust Fan": return .fan
            case "Supply Diffuser": return .airTerminal
            case "Fire Pump", "Jockey Pump": return .pump
            case "Water Heater": return .waterHeater
            case "Fire Service": return .service
            case "Luminaire", "Cab Light", "Pit Light": return .light
            case "Thermostat": return .control
            case "Sprinkler Head": return .sprinklerHead
            case "Cable Tray": return .cableTray
            case "Hoistway": return .hoistway
            case "Elevator Machine": return .elevatorMachine
            default: break
            }
            switch (system, role) {
            case (.cabinets, _): return .casework
            case (.furniture, _): return .furniture
            case (.plumbing, .fixture): return .fixture
            case (.mechanical, .main), (.mechanical, .branch): return .duct
            case (.plumbing, _), (.fire, _): if case .run = shape { return .pipe }; return .fixture
            case (.electrical, .main), (.electrical, .branch), (.lowVoltage, .main), (.lowVoltage, .branch): return .raceway
            default: return .device
            }
        }

        mutating func build() {
            levels = [
                BuildingLevel(index: 0, name: "Site", short: "Site", elevation: 0, height: 0),
                BuildingLevel(index: 1, name: "Level 1", short: "1", elevation: 0, height: 4.5),
                BuildingLevel(index: 2, name: "Level 2", short: "2", elevation: 4.5, height: 3.3),
                BuildingLevel(index: 3, name: "Level 3", short: "3", elevation: 7.8, height: 3.3),
                BuildingLevel(index: 4, name: "Level 4", short: "4", elevation: 11.1, height: 3.3),
                BuildingLevel(index: 5, name: "Roof", short: "Roof", elevation: 14.4, height: 0),
            ]
            makeRooms()
            makeArchitecture()
            makeElectrical()
            makeElevator()
            makeMechanical()
            makeExhaust()
            makePlumbing()
            makeFire()
            makeRoomContents()
        }

        // MARK: rooms

        /// The elevator bay: south side, x 12…16 on the guest floors (inside the lobby on level 1).
        static let elevatorX: Float = 12
        static let hoistMin = SIMD3<Float>(12.6, -1.5, 4.4), hoistMax = SIMD3<Float>(15.4, 15.6, 7.3)

        mutating func makeRooms() {
            let L = Self.length, cs = Self.corridorSouth, cn = Self.corridorNorth, D = Self.depth
            // Level 1: public rooms on the south side, service rooms on the north side, guest rooms east.
            rooms.append(BuildingRoom(number: "100", name: "Lobby", level: 1, min: SIMD2(0, 0), max: SIMD2(16, cs)))
            rooms.append(BuildingRoom(number: "102", name: "Breakfast", level: 1, min: SIMD2(16, 0), max: SIMD2(28, cs)))
            var n = 104
            for x in stride(from: Float(28), to: L, by: Self.bay) {
                rooms.append(BuildingRoom(number: "\(n)", name: "Guest Room", level: 1, min: SIMD2(x, 0), max: SIMD2(x + Self.bay, cs))); n += 1
            }
            rooms.append(BuildingRoom(number: "1S1", name: "Stair", level: 1, min: SIMD2(0, cn), max: SIMD2(4, D)))
            rooms.append(BuildingRoom(number: "130", name: "Electrical", level: 1, min: SIMD2(4, cn), max: SIMD2(12, D)))
            rooms.append(BuildingRoom(number: "131", name: "Mechanical", level: 1, min: SIMD2(12, cn), max: SIMD2(20, D)))
            rooms.append(BuildingRoom(number: "132", name: "Laundry", level: 1, min: SIMD2(20, cn), max: SIMD2(28, D)))
            rooms.append(BuildingRoom(number: "1E", name: "Shaft", level: 1, min: SIMD2(Self.shaftX, cn), max: SIMD2(Self.shaftX + Self.bay, D)))
            n = 114
            for x in stride(from: Float(32), to: L - Self.bay, by: Self.bay) {
                rooms.append(BuildingRoom(number: "\(n)", name: "Guest Room", level: 1, min: SIMD2(x, cn), max: SIMD2(x + Self.bay, D))); n += 1
            }
            rooms.append(BuildingRoom(number: "1S2", name: "Stair", level: 1, min: SIMD2(L - 4, cn), max: SIMD2(L, D)))
            rooms.append(BuildingRoom(number: "1C", name: "Corridor", level: 1, min: SIMD2(0, cs), max: SIMD2(L, cn)))
            // Levels 2-4: guest rooms both sides, stairs at the ends, the shaft bay in the middle of the north side,
            // the elevator bay on the south side.
            for lv in 2...4 {
                var s = 1
                for x in stride(from: Float(0), to: L, by: Self.bay) {
                    if x == Self.elevatorX {
                        rooms.append(BuildingRoom(number: "\(lv)EV", name: "Elevator", level: lv, min: SIMD2(x, 0), max: SIMD2(x + Self.bay, cs)))
                    } else {
                        rooms.append(BuildingRoom(number: String(format: "%d%02d", lv, s), name: "Guest Room", level: lv, min: SIMD2(x, 0), max: SIMD2(x + Self.bay, cs)))
                    }
                    s += 1
                }
                for x in stride(from: Float(0), to: L, by: Self.bay) {
                    if x == 0 || x == L - Self.bay {
                        rooms.append(BuildingRoom(number: "\(lv)S\(x == 0 ? 1 : 2)", name: "Stair", level: lv, min: SIMD2(x, cn), max: SIMD2(x + Self.bay, D)))
                    } else if x == Self.shaftX {
                        rooms.append(BuildingRoom(number: "\(lv)E", name: "Electrical", level: lv, min: SIMD2(x, cn), max: SIMD2(x + Self.bay, D)))
                    } else {
                        rooms.append(BuildingRoom(number: String(format: "%d%02d", lv, s), name: "Guest Room", level: lv, min: SIMD2(x, cn), max: SIMD2(x + Self.bay, D))); s += 1
                    }
                }
                rooms.append(BuildingRoom(number: "\(lv)C", name: "Corridor", level: lv, min: SIMD2(0, cs), max: SIMD2(L, cn)))
            }
        }

        func guestRooms(_ level: Int) -> [BuildingRoom] { rooms.filter { $0.level == level && $0.name == "Guest Room" } }
        func elev(_ l: Int) -> Float { levels.indices.contains(l) ? levels[l].elevation : 0 }
        func ceiling(_ l: Int) -> Float { elev(l) + (levels.indices.contains(l) ? levels[l].height : 3.3) - 0.45 }

        // MARK: architecture (doors, windows, stairs: the Architectural view's objects)

        /// Window sill and head above the floor, and the pier width, per bay (the storefront is the south half of level 1).
        static func windowOpening(level: Int, south: Bool, x1: Float, height h: Float) -> (sill: Float, head: Float, pier: Float) {
            let storefront = level == 1 && south && x1 <= length / 2
            return storefront ? (0.35, h - 0.7, 0.18) : (0.9, 2.45, 0.55)
        }

        mutating func makeArchitecture() {
            let L = Self.length, D = Self.depth
            for lv in 1...4 {
                let e = elev(lv), h = levels[lv].height
                // Windows: one per bay on both long faces.
                for k in 0..<Int((L / Self.bay).rounded()) {
                    let x0 = Float(k) * Self.bay, x1 = x0 + Self.bay
                    for south in [true, false] {
                        let o = Self.windowOpening(level: lv, south: south, x1: x1, height: h)
                        let z: Float = south ? 0.16 : D - 0.16
                        _ = add(o.sill < 0.5 ? "Storefront" : "Window", "W-\(lv)\(String(format: "%02d", k + 1))\(south ? "S" : "N")", .architecture, .shell, level: lv,
                                .box(center: SIMD3((x0 + x1) / 2, e + (o.sill + o.head) / 2, z), size: SIMD3(x1 - x0 - 2 * o.pier, o.head - o.sill, 0.06)),
                                detail: .building, link: "Window", part: .window)
                    }
                }
                // Doors: each room's door in its corridor wall (the gap the floor's walls leave).
                for r in rooms where r.level == lv && r.name != "Corridor" {
                    let south = r.max.y <= D / 2
                    let z = south ? r.max.y : r.min.y
                    _ = add("Door", "D-\(r.number)", .architecture, .shell, level: lv, room: r.number,
                            .box(center: SIMD3(r.max.x - 0.9, e + 1.05, z + (south ? -0.04 : 0.04)), size: SIMD3(0.9, 2.1, 0.05)),
                            height: 0, detail: .building, link: "Door", part: .door)
                }
                // Stairs: one flight volume per stair room and level.
                for r in rooms where r.level == lv && r.name == "Stair" {
                    _ = add("Stair", "ST-\(r.number)", .architecture, .shell, level: lv, room: r.number,
                            .box(center: SIMD3(r.center.x, e + h / 2, r.center.y), size: SIMD3(r.max.x - r.min.x - 0.6, h, r.max.y - r.min.y - 1.0)),
                            height: 0, detail: .building, link: "Stair", part: .stair)
                }
            }
        }

        // MARK: electrical

        var msbID = ""
        var panelIDs: [Int: String] = [:]
        var feederSlot: Float = 0
        mutating func slot() -> Float { feederSlot += 0.13; return feederSlot }

        mutating func makeElectrical() {
            let shaft = Self.shaftX
            let xf = add("Transformer", "T1", .electrical, .equipment, level: 0, .box(center: SIMD3(-5, 0.95, 13.5), size: SIMD3(2.2, 1.9, 1.9)),
                         catalog: "SAMPLE-PADMOUNT-750", size: "750 kVA, 480Y/277 V", detail: .building, link: "Transformer")
            let msb = add("Switchboard", "MSB", .electrical, .equipment, level: 1, room: "130", .box(center: SIMD3(8, 1.15, 17.35), size: SIMD3(4.2, 2.3, 0.9)),
                          catalog: "SAMPLE-SWBD-2000", size: "2000 A, 480Y/277 V", upstream: xf, detail: .building, link: "Gear")
            msbID = msb
            _ = add("Service", "S1", .electrical, .main, level: 0,
                    .run(points: [SIMD3(-5, 0.3, 13.5), SIMD3(-5, -0.9, 13.5), SIMD3(-5, -0.9, 17.0), SIMD3(6.5, -0.9, 17.0), SIMD3(6.5, 0.3, 17.0)],
                         profile: .round(radius: 0.11)),
                    size: "4 sets 4\" PVC", upstream: xf, detail: .building, link: "Service", nominal: 4, load: msb)
            for lv in 1...4 {
                let pz: Float = 17.82
                let px: Float = shaft + 2.6
                let ph = elev(lv) + 1.35
                let panelTag = "\(lv)LA"
                let s = slot()
                let feeder = add("Feeder", "F-\(panelTag)", .electrical, .main, level: lv,
                                 .run(points: [SIMD3(8 + s, 2.3, 17.35), SIMD3(8 + s, 3.95, 17.35), SIMD3(shaft + 0.4 + s, 3.95, 17.35),
                                               SIMD3(shaft + 0.4 + s, ph + 1.0, 17.35), SIMD3(px - 0.2, ph + 1.0, 17.35), SIMD3(px - 0.2, ph + 0.75, pz)],
                                      profile: .round(radius: 0.065)),
                                 size: "4#3/0 + 1#6 G, 2\" EMT", upstream: msb, detail: .building, link: "Feeder", nominal: 2)
                let panel = add("Panelboard", panelTag, .electrical, .equipment, level: lv, room: lv == 1 ? "1E" : "\(lv)E",
                                .box(center: SIMD3(px, ph, pz), size: SIMD3(0.56, 1.5, 0.2)), height: 1.35,
                                catalog: "SAMPLE-PB-225-42", size: "225 A, 42 circuits", upstream: feeder, detail: .building, link: "Panel")
                panelIDs[lv] = panel
                // The corridor cable tray the home runs ride in (a main: it shows at building scale).
                let trayY = ceiling(lv) + 0.42
                _ = add("Cable Tray", "CT-\(lv)", .electrical, .main, level: lv,
                        .run(points: [SIMD3(px, trayY, pz - 0.3), SIMD3(px, trayY, 9.1), SIMD3(1.2, trayY, 9.1), SIMD3(54.8, trayY, 9.1)],
                             profile: .rect(width: 0.3, height: 0.1)),
                        size: "12\" ladder tray", upstream: panel, detail: .building, link: "Tray", nominal: 12)
                // One home run per guest room, along the corridor ceiling and into the room.
                var circuit = 1
                for r in guestRooms(lv) {
                    let c = r.center
                    let ceil = ceiling(lv) + 0.25
                    let side: Float = r.max.y <= Self.corridorSouth + 0.01 ? Self.corridorSouth - 0.2 : Self.corridorNorth + 0.2
                    let lane: Float = 9.0 + Float(circuit % 6) * 0.06
                    let tag = "\(panelTag)-\(circuit)"
                    let run = add("Branch circuit", tag, .electrical, .branch, level: lv, room: nil,
                                  .run(points: [SIMD3(px, ph + 0.75, pz - 0.1), SIMD3(px, ceil, pz - 0.3), SIMD3(px, ceil, lane), SIMD3(c.x + 0.6, ceil, lane),
                                                SIMD3(c.x + 0.6, ceil, side), SIMD3(c.x + 0.6, ceil, c.y)],
                                       profile: .round(radius: 0.012)),
                                  circuit: tag, panel: panelTag, size: "2#12 + 1#12 G, 3/4\" EMT", upstream: panel, detail: .floor, link: "Circuit", nominal: 0.75)
                    makeRoomDevices(r, circuit: tag, panel: panelTag, upstream: run)
                    circuit += 2
                }
            }
            // A small house panel in the laundry, fed from the level 1 panel (a sub-panel: Full MEP, not mains).
            if let la = panelIDs[1] {
                let sub = add("Feeder", "F-1LB", .electrical, .main, level: 1,
                              .run(points: [SIMD3(30.6, 2.1, 17.72), SIMD3(30.6, 3.9, 17.72), SIMD3(24.0, 3.9, 17.72), SIMD3(24.0, 2.1, 17.82)],
                                   profile: .round(radius: 0.04)),
                              size: "3#6 + 1#10 G, 1-1/4\" EMT", upstream: la, detail: .floor, link: "Feeder", nominal: 1.25)
                let lb = add("Panelboard", "1LB", .electrical, .equipment, level: 1, room: "132",
                             .box(center: SIMD3(24.0, 1.35, 17.82), size: SIMD3(0.5, 1.1, 0.18)), height: 1.35,
                             catalog: "SAMPLE-PB-100-24", size: "100 A, 24 circuits", upstream: sub, detail: .floor, link: "Panel")
                let br = add("Branch circuit", "1LB-1", .electrical, .branch, level: 1,
                             .run(points: [SIMD3(24.0, 1.9, 17.8), SIMD3(24.0, 3.6, 17.8), SIMD3(21.0, 3.6, 17.8), SIMD3(21.0, 1.1, 17.85)], profile: .round(radius: 0.012)),
                             circuit: "1LB-1", panel: "1LB", size: "2#12 + 1#12 G, 3/4\" EMT", upstream: lb, detail: .floor, link: "Circuit", nominal: 0.75)
                _ = add("Receptacle", "R1", .electrical, .terminal, level: 1, room: "132", .box(center: SIMD3(21.0, 1.07, 17.9), size: SIMD3(0.08, 0.12, 0.05)), height: 1.07,
                        catalog: "SAMPLE-DUPLEX-20A-TR", circuit: "1LB-1", panel: "1LB", upstream: br, detail: .room, link: "Outlet")
            }
            // Rooftop units on their own feeders from the switchboard (their loads are linked once the units exist).
            for (i, x) in [Float(18), 38].enumerated() {
                let s = slot()
                let tag = "RTU-\(i + 1)"
                let f = add("Feeder", "F-\(tag)", .electrical, .main, level: 5,
                            .run(points: [SIMD3(8 + s, 2.3, 17.35), SIMD3(8 + s, 3.95, 17.35), SIMD3(shaft + 0.4 + s, 3.95, 17.35),
                                          SIMD3(shaft + 0.4 + s, elev(5) + 0.6, 17.35), SIMD3(x, elev(5) + 0.6, 17.35), SIMD3(x, elev(5) + 0.6, 10.6)],
                                 profile: .round(radius: 0.055)),
                            size: "3#4 + 1#8 G, 1-1/4\" EMT", upstream: msb, detail: .building, link: "Feeder", nominal: 1.25)
                feederIDs[tag] = f
            }
            let s = slot()
            feederIDs["DOAS-1"] = add("Feeder", "F-DOAS-1", .electrical, .main, level: 5,
                                      .run(points: [SIMD3(8 + s, 2.3, 17.35), SIMD3(8 + s, 3.95, 17.35), SIMD3(shaft + 0.4 + s, 3.95, 17.35),
                                                    SIMD3(shaft + 0.4 + s, elev(5) + 0.6, 17.35), SIMD3(48, elev(5) + 0.6, 17.35), SIMD3(48, elev(5) + 0.6, 10.2)],
                                           profile: .round(radius: 0.05)),
                                      size: "3#6 + 1#10 G, 1\" EMT", upstream: msb, detail: .building, link: "Feeder", nominal: 1)
        }

        var feederIDs: [String: String] = [:]

        mutating func makeRoomDevices(_ r: BuildingRoom, circuit: String, panel: String, upstream: String) {
            let lv = r.level
            let e = elev(lv)
            let south = r.max.y <= Self.corridorSouth + 0.01
            let window: Float = south ? r.min.y + 0.12 : r.max.y - 0.12
            let x0 = r.min.x, x1 = r.max.x
            let deviceSize = SIMD3<Float>(0.08, 0.12, 0.05)
            let dev: [(String, String, SIMD3<Float>, Float, String)] = [
                ("Receptacle", "R1", SIMD3(x0 + 0.1, 0.4572, r.center.y + (south ? -1.2 : 1.2)), 0.4572, "SAMPLE-DUPLEX-20A-TR"),
                ("Receptacle", "R2", SIMD3(x1 - 0.1, 0.4572, r.center.y + (south ? -1.8 : 1.8)), 0.4572, "SAMPLE-DUPLEX-20A-TR"),
                ("Receptacle", "R3", SIMD3(r.center.x, 0.4572, window), 0.4572, "SAMPLE-DUPLEX-20A-TR"),
                ("GFCI Receptacle", "G1", SIMD3(x0 + 0.1, 1.0668, south ? Self.corridorSouth - 1.6 : Self.corridorNorth + 1.6), 1.0668, "SAMPLE-GFCI-20A-TR"),
                ("Switch", "S1", SIMD3(x1 - 0.1, 1.2192, south ? Self.corridorSouth - 0.4 : Self.corridorNorth + 0.4), 1.2192, "SAMPLE-SW-1P-20A"),
            ]
            for (kind, tag, p, h, cat) in dev {
                _ = add(kind, tag, .electrical, .terminal, level: lv, room: r.number, .box(center: SIMD3(p.x, e + p.y, p.z), size: deviceSize), height: h,
                        catalog: cat, circuit: circuit, panel: panel, upstream: upstream, detail: .room, link: kind.contains("Switch") ? "Switch" : "Outlet")
            }
            // Lights: shown on the floor view as warm points.
            let ch = ceiling(lv) - e
            for (tag, dz) in [("L1", Float(south ? -1.4 : 1.4)), ("L2", Float(south ? 1.8 : -1.8))] {
                _ = add("Luminaire", tag, .electrical, .terminal, level: lv, room: r.number,
                        .box(center: SIMD3(r.center.x, e + ch - 0.04, r.center.y + dz), size: SIMD3(0.18, 0.06, 0.18)), height: ch,
                        catalog: "SAMPLE-DL-6IN-27K", circuit: circuit, panel: panel, upstream: upstream, detail: .floor, link: "Light")
            }
            // A data outlet with no upstream in the model: the honest "no known connection" case.
            _ = add("Data Outlet", "D1", .lowVoltage, .terminal, level: lv, room: r.number,
                    .box(center: SIMD3(x1 - 0.1, e + 0.4572, r.center.y + (south ? -1.0 : 1.0)), size: deviceSize), height: 0.4572,
                    catalog: "SAMPLE-CAT6-2P", detail: .room, link: "Outlet")
        }

        // MARK: elevator

        /// One traction elevator: the hoistway volume, its machine at the top, a disconnect on the top floor fed
        /// from the switchboard, and the branches that serve the car and the pit (which no MEP view draws).
        mutating func makeElevator() {
            let lo = Self.hoistMin, hi = Self.hoistMax
            let c = (lo + hi) / 2
            let hoist = add("Hoistway", "EL-1", .conveying, .equipment, level: 1,
                            .box(center: c, size: hi - lo), catalog: "SAMPLE-HOISTWAY", size: "2,500 lb, 350 fpm", detail: .building, link: "Elevator")
            let roof = elev(5)
            let s = slot()
            let feeder = add("Feeder", "F-ED-1", .electrical, .main, level: 4,
                             .run(points: [SIMD3(8 + s, 2.3, 17.35), SIMD3(8 + s, 3.95, 17.35), SIMD3(Self.shaftX + 0.4 + s, 3.95, 17.35),
                                           SIMD3(Self.shaftX + 0.4 + s, elev(4) + 2.9, 17.35), SIMD3(Self.shaftX + 0.4 + s, elev(4) + 2.9, 7.75),
                                           SIMD3(15.7, elev(4) + 2.9, 7.75), SIMD3(15.7, elev(4) + 1.8, 7.42)],
                                  profile: .round(radius: 0.045)),
                             size: "3#2 + 1#8 G, 1-1/4\" EMT", upstream: msbID, detail: .building, link: "Feeder", nominal: 1.25)
            let disc = add("Disconnect", "ED-1", .electrical, .equipment, level: 4, room: "4EV",
                           .box(center: SIMD3(15.7, elev(4) + 1.5, 7.42), size: SIMD3(0.35, 0.5, 0.2)), height: 1.5,
                           catalog: "SAMPLE-DISC-100A", size: "100 A fused", upstream: feeder, detail: .building, link: "Disconnect")
            let machineFeed = add("Feeder", "F-EM-1", .electrical, .main, level: 4,
                                  .run(points: [SIMD3(15.7, elev(4) + 1.75, 7.42), SIMD3(15.7, roof + 0.6, 7.42), SIMD3(14.8, roof + 0.6, 6.6)],
                                       profile: .round(radius: 0.04)),
                                  size: "3#2 + 1#8 G, 1-1/4\" EMT", upstream: disc, detail: .building, link: "Feeder", nominal: 1.25)
            _ = add("Elevator Machine", "EM-1", .conveying, .equipment, level: 5,
                    .box(center: SIMD3(c.x, roof + 0.55, c.z), size: SIMD3(1.4, 0.9, 1.2)),
                    catalog: "SAMPLE-MRL-MACHINE", size: "25 hp", upstream: machineFeed, detail: .building, link: "Machine")
            _ = hoist
            // The car's light circuit: from the top-floor panel to the hoistway, then the traveling cable to the car.
            if let p4 = panelIDs[4] {
                let px = Self.shaftX + 2.6
                let cab = add("Branch circuit", "4LA-41", .electrical, .branch, level: 4,
                              .run(points: [SIMD3(px, elev(4) + 2.1, 17.7), SIMD3(px, ceiling(4) + 0.3, 17.4), SIMD3(px, ceiling(4) + 0.3, 7.9),
                                            SIMD3(14.0, ceiling(4) + 0.3, 7.9), SIMD3(14.0, ceiling(4) + 0.3, 7.0)],
                                   profile: .round(radius: 0.012)),
                              circuit: "4LA-41", panel: "4LA", size: "2#12 + 1#12 G, 3/4\" EMT", upstream: p4, detail: .floor, link: "Circuit", nominal: 0.75)
                let travel = add("Traveling Cable", "TC-1", .electrical, .branch, level: 2,
                                 .run(points: [SIMD3(14.0, ceiling(4) + 0.3, 7.0), SIMD3(14.0, elev(3) + 1.6, 7.0), SIMD3(13.4, elev(2) + 2.6, 6.2)],
                                      profile: .round(radius: 0.02)),
                                 circuit: "4LA-41", panel: "4LA", size: "traveling cable", upstream: cab, detail: .floor, link: "Cable", nominal: 1.25)
                _ = add("Cab Light", "CL-1", .electrical, .terminal, level: 2,
                        .box(center: SIMD3(14.0, elev(2) + 2.5, 5.85), size: SIMD3(0.5, 0.05, 0.5)), height: 2.5,
                        catalog: "SAMPLE-CAB-LED", circuit: "4LA-41", panel: "4LA", upstream: travel, detail: .floor, link: "Light")
            }
            // The pit light: from the level 1 panel down into the pit (it passes through the floor).
            if let p1 = panelIDs[1] {
                let px = Self.shaftX + 2.6
                let pit = add("Branch circuit", "1LA-41", .electrical, .branch, level: 1,
                              .run(points: [SIMD3(px, 2.1, 17.7), SIMD3(px, ceiling(1) + 0.3, 17.4), SIMD3(px, ceiling(1) + 0.3, 7.9),
                                            SIMD3(15.2, ceiling(1) + 0.3, 7.9), SIMD3(15.2, ceiling(1) + 0.3, 7.15), SIMD3(15.2, -1.0, 7.15)],
                                   profile: .round(radius: 0.012)),
                              circuit: "1LA-41", panel: "1LA", size: "2#12 + 1#12 G, 3/4\" EMT", upstream: p1, detail: .floor, link: "Circuit", nominal: 0.75)
                _ = add("Pit Light", "PL-1", .electrical, .terminal, level: 1,
                        .box(center: SIMD3(15.2, -1.1, 6.9), size: SIMD3(0.12, 0.12, 0.08)),
                        catalog: "SAMPLE-PIT-LED", circuit: "1LA-41", panel: "1LA", upstream: pit, detail: .floor, link: "Light")
            }
        }

        // MARK: mechanical

        var rtuIDs: [String] = []
        mutating func makeMechanical() {
            let roof = elev(5)
            for (i, x) in [Float(18), 38].enumerated() {
                let tag = "RTU-\(i + 1)"
                let id = add("Rooftop Unit", tag, .mechanical, .equipment, level: 5, .box(center: SIMD3(x, roof + 0.95, 9), size: SIMD3(4.6, 1.9, 2.3)),
                             catalog: "SAMPLE-RTU-15T", size: "15 ton, 6,000 CFM", upstream: nil, detail: .building, link: "Unit")
                rtuIDs.append(id)
                linkFeeder(tag, to: id)
                // Supply riser down the shaft beside the corridor.
                let rx = x + 2.0
                let riser = add("Supply Riser", "SR-\(i + 1)", .mechanical, .main, level: 1,
                                .run(points: [SIMD3(x + 2.3, roof + 0.95, 9), SIMD3(rx, roof + 0.95, 9.0), SIMD3(rx, 1.0 + 3.6, 9.0)],
                                     profile: .rect(width: 0.7, height: 0.5)),
                                size: "28×20 duct", upstream: id, detail: .building, link: "Riser", nominal: 28)
                for lv in 1...4 {
                    let y = ceiling(lv) + 0.18
                    let xs: (Float, Float) = i == 0 ? (1.5, 28.0) : (28.0, 54.5)
                    let main = add("Supply Duct", "SD-\(lv)\(i == 0 ? "W" : "E")", .mechanical, .main, level: lv,
                                   .run(points: [SIMD3(rx, y, 9.0), SIMD3(rx, y, 8.6), SIMD3(xs.0, y, 8.6), SIMD3(xs.1, y, 8.6)],
                                        profile: .rect(width: 0.6, height: 0.32)),
                                   size: "24×12 duct", upstream: riser, detail: .building, link: "Main", nominal: 24)
                    for r in guestRooms(lv) where r.center.x >= xs.0 && r.center.x < xs.1 {
                        let south = r.max.y <= Self.corridorSouth + 0.01
                        let dz: Float = south ? -0.5 : 0.5
                        let target = SIMD3<Float>(r.center.x - 0.6, y, r.center.y + dz)
                        let br = add("Branch Duct", "BD-\(r.number)", .mechanical, .branch, level: lv, room: nil,
                                     .run(points: [SIMD3(target.x, y, 8.6), target], profile: .rect(width: 0.25, height: 0.2)),
                                     size: "10×8 duct", upstream: main, detail: .floor, link: "Branch", nominal: 10)
                        let ch = ceiling(lv) - elev(lv)
                        _ = add("Supply Diffuser", "SD1", .mechanical, .terminal, level: lv, room: r.number,
                                .box(center: SIMD3(target.x, ceiling(lv) - 0.02, target.z), size: SIMD3(0.6, 0.06, 0.6)), height: ch,
                                catalog: "SAMPLE-DIFF-24", size: "150 CFM", upstream: br, detail: .floor, link: "Diffuser")
                        _ = add("Thermostat", "T1", .mechanical, .terminal, level: lv, room: r.number,
                                .box(center: SIMD3(r.max.x - 0.08, elev(lv) + 1.22, r.center.y + (south ? 0.6 : -0.6)), size: SIMD3(0.05, 0.09, 0.09)),
                                height: 1.22, catalog: "SAMPLE-TSTAT", upstream: br, detail: .room, link: "Control")
                    }
                }
            }
            // A dedicated outdoor air unit with its riser and a corridor outdoor-air duct on every floor.
            let doas = add("Outdoor Air Unit", "DOAS-1", .mechanical, .equipment, level: 5, .box(center: SIMD3(48, roof + 0.8, 9.6), size: SIMD3(3.6, 1.6, 2.0)),
                           catalog: "SAMPLE-DOAS-3000", size: "3,000 CFM outdoor air", detail: .building, link: "Unit")
            linkFeeder("DOAS-1", to: doas)
            let ox: Float = 50.6
            let oaRiser = add("Outdoor Air Riser", "OAR-1", .mechanical, .main, level: 1,
                              .run(points: [SIMD3(49.8, roof + 0.8, 9.6), SIMD3(ox, roof + 0.8, 10.1), SIMD3(ox, ceiling(1) + 0.12, 10.1)],
                                   profile: .rect(width: 0.4, height: 0.3)),
                              size: "16×12 duct", upstream: doas, detail: .building, link: "Riser", nominal: 16)
            for lv in 1...4 {
                let y = ceiling(lv) + 0.12
                _ = add("Outdoor Air Duct", "OA-\(lv)", .mechanical, .main, level: lv,
                        .run(points: [SIMD3(ox, y, 10.1), SIMD3(2.0, y, 10.1)], profile: .rect(width: 0.3, height: 0.25)),
                        size: "12×10 duct", upstream: oaRiser, detail: .building, link: "Main", nominal: 12)
            }
        }

        /// Names a unit as the load of its feeder (the unit's own upstream is its duct system).
        mutating func linkFeeder(_ tag: String, to unit: String) {
            guard let f = feederIDs[tag], let fi = elements.firstIndex(where: { $0.id == f }) else { return }
            elements[fi].load = unit
            if let ui = elements.firstIndex(where: { $0.id == unit }) {
                elements[ui].size = (elements[ui].size ?? "") + " · fed by \(elements[fi].tag)"
            }
        }

        /// Bathroom exhaust: a fan in every guest bath, a short duct to the stack, and one riser per stack of baths
        /// up through the roof. The risers cross floors, which is why Full MEP shows them.
        mutating func makeExhaust() {
            let roof = elev(5)
            struct Stack { var x: Float; var south: Bool; var lowest: Int }
            var stacks: [String: Stack] = [:]
            var order: [String] = []
            for lv in 1...4 {
                for r in guestRooms(lv) {
                    let south = r.max.y <= Self.corridorSouth + 0.01
                    let key = "\(r.min.x)|\(south)"
                    if stacks[key] == nil { stacks[key] = Stack(x: r.min.x, south: south, lowest: lv); order.append(key) }
                }
            }
            var riserIDs: [String: String] = [:]
            for (k, key) in order.enumerated() {
                guard let st = stacks[key] else { continue }
                let rz: Float = st.south ? Self.corridorSouth - 0.35 : Self.corridorNorth + 0.35
                let rx = st.x + 0.3
                riserIDs[key] = add("Exhaust Riser", "ER-\(k + 1)", .mechanical, .main, level: st.lowest,
                                    .run(points: [SIMD3(rx, ceiling(st.lowest) + 0.05, rz), SIMD3(rx, roof + 0.7, rz)], profile: .round(radius: 0.12)),
                                    size: "10\" round duct", detail: .building, link: "Riser", nominal: 10)
            }
            for lv in 1...4 {
                for r in guestRooms(lv) {
                    let south = r.max.y <= Self.corridorSouth + 0.01
                    guard let riser = riserIDs["\(r.min.x)|\(south)"] else { continue }
                    let rz: Float = south ? Self.corridorSouth - 0.35 : Self.corridorNorth + 0.35
                    let fz: Float = south ? Self.corridorSouth - 1.2 : Self.corridorNorth + 1.2
                    let y = ceiling(lv) + 0.05
                    let duct = add("Exhaust Duct", "ED-\(r.number)", .mechanical, .branch, level: lv,
                                   .run(points: [SIMD3(r.min.x + 1.2, y, fz), SIMD3(r.min.x + 1.2, y, rz), SIMD3(r.min.x + 0.3, y, rz)], profile: .round(radius: 0.075)),
                                   size: "6\" round duct", upstream: riser, detail: .floor, link: "Duct", nominal: 6)
                    _ = add("Exhaust Fan", "EF-1", .mechanical, .terminal, level: lv, room: r.number,
                            .box(center: SIMD3(r.min.x + 1.2, ceiling(lv) - 0.1, fz), size: SIMD3(0.3, 0.2, 0.3)), height: ceiling(lv) - elev(lv),
                            catalog: "SAMPLE-EF-80", size: "80 CFM", upstream: duct, detail: .floor, link: "Fan")
                }
            }
        }

        // MARK: plumbing

        mutating func makePlumbing() {
            let wh = add("Water Heater", "WH-1", .plumbing, .equipment, level: 1, room: "131", .box(center: SIMD3(14.5, 0.95, 16.8), size: SIMD3(0.9, 1.9, 0.9)),
                         catalog: "SAMPLE-WH-199", size: "199 MBH, 100 gal", detail: .building, link: "Heater")
            let rx: Float = 21.0
            let riser = add("Water Riser", "HW-R1", .plumbing, .main, level: 1,
                            .run(points: [SIMD3(14.5, 1.9, 16.8), SIMD3(14.5, 4.1, 16.8), SIMD3(rx, 4.1, 16.8), SIMD3(rx, 4.1, 10.7), SIMD3(rx, elev(4) + 3.0, 10.7)],
                                 profile: .round(radius: 0.045)),
                            size: "2-1/2\" copper", upstream: wh, detail: .building, link: "Riser", nominal: 2.5)
            for lv in 1...4 {
                let y = ceiling(lv) + 0.05
                let main = add("Water Main", "HW-\(lv)", .plumbing, .main, level: lv,
                               .run(points: [SIMD3(rx, y, 10.7), SIMD3(rx, y, 9.4), SIMD3(1.5, y, 9.4), SIMD3(54.5, y, 9.4)],
                                    profile: .round(radius: 0.035)),
                               size: "2\" copper", upstream: riser, detail: .floor, link: "Main", nominal: 2)
                for r in guestRooms(lv) {
                    let south = r.max.y <= Self.corridorSouth + 0.01
                    // The bath is the corridor end of the room, on its low-x side.
                    let bz: Float = south ? Self.corridorSouth - 1.1 : Self.corridorNorth + 1.1
                    let bx = r.min.x + 0.5
                    let br = add("Water Branch", "HW-\(r.number)", .plumbing, .branch, level: lv, room: nil,
                                 .run(points: [SIMD3(bx, y, 9.4), SIMD3(bx, y, bz), SIMD3(bx, elev(lv) + 1.0, bz)], profile: .round(radius: 0.012)),
                                 size: "3/4\" copper", upstream: main, detail: .floor, link: "Branch", nominal: 0.75)
                    let fx: [(String, String, SIMD3<Float>, SIMD3<Float>, String)] = [
                        ("Lavatory", "LAV-1", SIMD3(bx + 0.1, elev(lv) + 0.85, bz + (south ? 0.45 : -0.45)), SIMD3(0.5, 0.18, 0.42), "SAMPLE-LAV-UM"),
                        ("Water Closet", "WC-1", SIMD3(bx + 0.9, elev(lv) + 0.38, bz + (south ? 0.5 : -0.5)), SIMD3(0.38, 0.76, 0.7), "SAMPLE-WC-FT"),
                        ("Shower", "SH-1", SIMD3(bx + 0.4, elev(lv) + 0.05, bz - (south ? 0.6 : -0.6)), SIMD3(0.9, 0.1, 0.9), "SAMPLE-SHWR-36"),
                    ]
                    for (kind, tag, c, s, cat) in fx {
                        _ = add(kind, tag, .plumbing, .fixture, level: lv, room: r.number, .box(center: c, size: s), height: c.y - elev(lv) - s.y / 2,
                                catalog: cat, size: "1/2\" supply", upstream: br, detail: .room, link: "Fixture")
                    }
                }
            }
            // A laundry drain the model gives neither a role nor a size: Full MEP shows it, marked inferred, and
            // says so in a note; no view guesses its size.
            _ = add("Drain", "LD-1", .plumbing, .unknown, level: 1,
                    .run(points: [SIMD3(22.5, -0.3, 12.0), SIMD3(26.5, -0.3, 12.0)], profile: .round(radius: 0.04)),
                    detail: .floor, link: "Drain")
        }

        // MARK: fire protection

        mutating func makeFire() {
            let service = add("Fire Service", "FS-1", .fire, .equipment, level: 0, .box(center: SIMD3(-2.2, 0.6, 15.2), size: SIMD3(0.9, 1.2, 0.6)),
                              catalog: "SAMPLE-BFP-6", size: "6\" double check", detail: .building, link: "Service")
            let lead = add("Fire Service Main", "FSM-1", .fire, .main, level: 1,
                           .run(points: [SIMD3(-2.2, 0.6, 15.2), SIMD3(-2.2, -0.8, 15.2), SIMD3(16.5, -0.8, 15.2), SIMD3(16.5, -0.8, 14.5), SIMD3(16.5, 0.2, 14.5)],
                                profile: .round(radius: 0.08)),
                           size: "6\" ductile iron", upstream: service, detail: .building, link: "Main", nominal: 6)
            let pump = add("Fire Pump", "FP-1", .fire, .equipment, level: 1, room: "131", .box(center: SIMD3(16.5, 0.6, 14.5), size: SIMD3(1.6, 1.2, 0.8)),
                           catalog: "SAMPLE-FP-500", size: "500 gpm", upstream: lead, detail: .building, link: "Pump")
            let rx: Float = 18.6
            let riser = add("Sprinkler Riser", "FR-1", .fire, .main, level: 1,
                            .run(points: [SIMD3(17.3, 0.9, 14.5), SIMD3(rx, 0.9, 14.5), SIMD3(rx, 0.9, 17.6), SIMD3(rx, elev(4) + 3.1, 17.6)],
                                 profile: .round(radius: 0.075)),
                            size: "6\" steel", upstream: pump, detail: .building, link: "Riser", nominal: 6)
            for lv in 1...4 {
                let y = ceiling(lv) + 0.32
                let main = add("Sprinkler Main", "FM-\(lv)", .fire, .main, level: lv,
                               .run(points: [SIMD3(rx, y, 17.6), SIMD3(rx, y, 9.8), SIMD3(1.0, y, 9.8), SIMD3(55.0, y, 9.8)], profile: .round(radius: 0.05)),
                               size: "4\" steel", upstream: riser, detail: .building, link: "Main", nominal: 4)
                for r in guestRooms(lv) {
                    let bx = r.center.x + 0.3
                    let br = add("Sprinkler Branch", "FB-\(r.number)", .fire, .branch, level: lv, room: nil,
                                 .run(points: [SIMD3(bx, y, 9.8), SIMD3(bx, y, r.center.y)], profile: .round(radius: 0.016)),
                                 size: "1\" steel", upstream: main, detail: .floor, link: "Branch", nominal: 1)
                    _ = add("Sprinkler Head", "SP-1", .fire, .terminal, level: lv, room: r.number,
                            .box(center: SIMD3(bx, ceiling(lv) - 0.02, r.center.y), size: SIMD3(0.06, 0.05, 0.06)), height: ceiling(lv) - elev(lv),
                            catalog: "SAMPLE-HEAD-PEND-QR", size: "K5.6, 155°F", upstream: br, detail: .floor, link: "Head")
                }
            }
        }

        // MARK: cabinets and furniture

        mutating func makeRoomContents() {
            for lv in 1...4 {
                let e = elev(lv)
                for r in guestRooms(lv) {
                    let south = r.max.y <= Self.corridorSouth + 0.01
                    // Depth measured from the window wall into the room.
                    func at(_ fromLeft: Float, _ fromWindow: Float, _ y: Float) -> SIMD3<Float> {
                        SIMD3(r.min.x + fromLeft, e + y, south ? r.min.y + fromWindow : r.max.y - fromWindow)
                    }
                    func size(_ alongX: Float, _ h: Float, _ alongDepth: Float) -> SIMD3<Float> { SIMD3(alongX, h, alongDepth) }
                    let cab: [(String, String, SIMD3<Float>, SIMD3<Float>)] = [
                        ("Vanity", "VAN-1", SIMD3(r.min.x + 0.6, e + 0.43, (south ? Self.corridorSouth - 1.1 : Self.corridorNorth + 1.1) + (south ? 0.45 : -0.45)), size(0.9, 0.86, 0.55)),
                        ("Wardrobe", "WR-1", at(3.6, 5.6, 1.05), size(0.6, 2.1, 1.2)),
                        ("TV Console", "TC-1", at(3.7, 2.6, 0.38), size(0.45, 0.76, 1.6)),
                    ]
                    // Cabinets show on the floor and in the room (their switch is in both).
                    for (kind, tag, c, s) in cab {
                        _ = add(kind, tag, .cabinets, .casework, level: lv, room: r.number, .box(center: c, size: s), height: c.y - e - s.y / 2,
                                catalog: "SAMPLE-CASEWORK", size: String(format: "%.0f″ wide", s.x / 0.0254), detail: .floor, link: "Cabinet")
                    }
                    let furn: [(String, String, SIMD3<Float>, SIMD3<Float>)] = [
                        ("Bed", "BED-1", at(1.2, 2.4, 0.3), size(2.05, 0.6, 1.6)),
                        ("Headboard", "HB-1", at(0.12, 2.4, 0.55), size(0.08, 1.1, 1.75)),
                        ("Nightstand", "NS-1", at(0.3, 1.25, 0.28), size(0.45, 0.56, 0.45)),
                        ("Nightstand", "NS-2", at(0.3, 3.55, 0.28), size(0.45, 0.56, 0.45)),
                        ("Desk", "DK-1", at(3.6, 1.0, 0.38), size(0.6, 0.76, 1.2)),
                        ("Chair", "CH-1", at(3.1, 1.0, 0.45), size(0.5, 0.9, 0.5)),
                        ("Lounge Chair", "LC-1", at(1.0, 0.6, 0.4), size(0.8, 0.8, 0.8)),
                    ]
                    for (kind, tag, c, s) in furn {
                        _ = add(kind, tag, .furniture, .furniture, level: lv, room: r.number, .box(center: c, size: s), height: c.y - e - s.y / 2,
                                catalog: "GENERIC", size: String(format: "%.0f″ × %.0f″", s.x / 0.0254, s.z / 0.0254), detail: .room, link: "Furniture")
                    }
                }
            }
        }
    }
}
