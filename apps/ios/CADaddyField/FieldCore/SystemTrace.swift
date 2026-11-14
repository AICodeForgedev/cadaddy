// CADaddy Field — "follow the system": what feeds an element, and what an element feeds.
// Pure logic over BuildingElement.upstream links, shared by the app and the tests.

import Foundation

public struct TraceStep: Equatable, Sendable {
    public var id: String
    /// The role in the chain: "Outlet", "Circuit", "Panel", "Feeder", "Gear".
    public var link: String
    /// The tag: "R1", "2LA-7", "2LA", "MSB".
    public var tag: String
    public var level: Int
    /// How the link from this step to the next (its source) is known; nil = the sample, or the path's last step.
    public var state: LinkState?

    public init(id: String, link: String, tag: String, level: Int, state: LinkState? = nil) {
        self.id = id; self.link = link; self.tag = tag; self.level = level; self.state = state
    }
}

public enum TraceResult: Equatable, Sendable {
    /// From the tapped element up to the source, in that order.
    case chain([TraceStep])
    /// The model has no upstream and nothing downstream for this element. Said plainly; no false path.
    case noKnownConnection(String)
}

/// What a tap lights: the elements to highlight, and the path strip to show.
public struct SystemFocus: Equatable, Sendable {
    public var tapped: String
    public var lit: Set<String>
    public var trace: TraceResult
    /// True when the tapped element feeds others (a panel, gear, unit): everything downstream is lit.
    public var showsDownstream: Bool
    /// Said at the end of the path when it stops at a link nobody gives ("Unknown: …").
    public var note: String?
    /// How many objects the tapped one feeds (from the graph or the sample's links).
    public var downstreamCount: Int

    public init(tapped: String, lit: Set<String>, trace: TraceResult, showsDownstream: Bool, note: String? = nil, downstreamCount: Int = 0) {
        self.tapped = tapped; self.lit = lit; self.trace = trace; self.showsDownstream = showsDownstream; self.note = note; self.downstreamCount = downstreamCount
    }

    public var steps: [TraceStep] { if case let .chain(s) = trace { return s } else { return [] } }
}

extension SampleBuilding {
    static let maxTraceDepth = 32
    static let maxDownstream = 20_000

    /// Walks upstream from `id` to the source. Stops at a cycle or at `maxTraceDepth`.
    public func trace(_ id: String) -> TraceResult {
        guard let start = element(id) else { return .noKnownConnection("This object is not in the model.") }
        if start.upstream == nil && feeds(id).isEmpty {
            return .noKnownConnection(start.role == .furniture || start.role == .casework
                ? "\(start.kind) is not connected to a system."
                : "No known connection: the model does not say what feeds this \(start.kind.lowercased()).")
        }
        var steps: [TraceStep] = []
        var seen = Set<String>()
        var cur: BuildingElement? = start
        while let e = cur, steps.count < Self.maxTraceDepth, !seen.contains(e.id) {
            seen.insert(e.id)
            steps.append(TraceStep(id: e.id, link: e.link, tag: e.tag, level: e.level))
            cur = e.upstream.flatMap { element($0) }
        }
        return .chain(steps)
    }

    /// Everything fed from `id`, directly or through other elements (not including `id`).
    public func downstream(of id: String) -> Set<String> {
        var out = Set<String>()
        var stack = feeds(id)
        while let n = stack.popLast(), out.count < Self.maxDownstream {
            guard out.insert(n).inserted else { continue }
            stack.append(contentsOf: feeds(n))
        }
        return out
    }

    /// The tap rule: equipment and mains light everything they feed; anything else traces up to its source.
    public func focus(_ id: String) -> SystemFocus {
        let t = trace(id)
        let kids = downstream(of: id)
        let isSource = !kids.isEmpty && (element(id)?.role == .equipment || element(id)?.role == .main)
        var lit = Set<String>([id])
        if case let .chain(s) = t { lit.formUnion(s.map(\.id)) }
        if isSource { lit.formUnion(kids) }
        return SystemFocus(tapped: id, lit: lit, trace: t, showsDownstream: isSource, downstreamCount: kids.count)
    }
}

// MARK: - Trade layers (the M / E / P / Cabinets / Furniture menu)

public enum FieldLayer: String, CaseIterable, Codable, Sendable, Identifiable {
    case mechanical = "M", electrical = "E", plumbing = "P", cabinets = "Cabinets", furniture = "Furniture"
    public var id: String { rawValue }

    public var title: String {
        switch self {
        case .mechanical: return "Mechanical"
        case .electrical: return "Electrical"
        case .plumbing: return "Plumbing"
        case .cabinets: return "Cabinets"
        case .furniture: return "Furniture"
        }
    }

    /// The layer a system belongs to. Fire protection rides with P; fire alarm and low voltage with E.
    /// Architecture (the ghost shell) has no layer: it is always there, quietly.
    public static func of(_ s: BuildingSystem) -> FieldLayer? {
        switch s {
        case .electrical, .lowVoltage: return .electrical
        case .mechanical: return .mechanical
        case .plumbing, .fire: return .plumbing
        case .cabinets: return .cabinets
        case .furniture: return .furniture
        case .architecture, .conveying: return nil
        }
    }

    /// The layer of an exported package element, from its trade text (and kind when the trade says nothing).
    public static func of(trade: String?, kind: String?) -> FieldLayer? {
        let t = (trade ?? "").lowercased()
        let k = (kind ?? "").lowercased()
        for (words, layer) in tradeWords where words.contains(where: { t.contains($0) }) { return layer }
        for (words, layer) in kindWords where words.contains(where: { k.contains($0) }) { return layer }
        return nil
    }

    static let tradeWords: [([String], FieldLayer)] = [
        (["furniture", "ff&e", "ffe"], .furniture),
        (["casework", "cabinet", "millwork"], .cabinets),
        (["mechanical", "hvac"], .mechanical),
        (["plumbing", "fire-protection", "fire protection", "sprinkler"], .plumbing),
        (["electrical", "low-voltage", "low voltage", "fire-alarm", "fire alarm", "telecom", "data", "lighting"], .electrical),
    ]
    static let kindWords: [([String], FieldLayer)] = [
        (["bed", "desk", "chair", "sofa", "table", "dresser", "nightstand"], .furniture),
        (["cabinet", "vanity", "counter", "wardrobe", "casework"], .cabinets),
        (["duct", "diffuser", "grille", "thermostat", "rtu", "ahu"], .mechanical),
        (["pipe", "lavatory", "water closet", "sink", "shower", "sprinkler"], .plumbing),
        (["receptacle", "switch", "luminaire", "panel", "conduit", "outlet"], .electrical),
    ]

    /// At building scope cabinets and furniture have nothing to show, so their switches are dimmed, not hidden.
    public func isDimmed(at scope: DetailScope) -> Bool { scope == .building && (self == .cabinets || self == .furniture) }
}

/// Which layers are switched off. One value for every screen (building, floor, room, AR), kept on the device.
public struct LayerPrefs: Equatable, Codable, Sendable {
    public var off: Set<FieldLayer> = []
    public init(off: Set<FieldLayer> = []) { self.off = off }

    public static let defaultsKey = "field.layers.off"

    public func isOn(_ l: FieldLayer) -> Bool { !off.contains(l) }
    public mutating func set(_ l: FieldLayer, on: Bool) { if on { off.remove(l) } else { off.insert(l) } }

    /// Shown unless its layer is off. Elements with no layer (the shell) always show.
    public func shows(_ s: BuildingSystem) -> Bool { FieldLayer.of(s).map(isOn) ?? true }

    /// A device that never saved its switches: everything on except furniture (off by default; it only exists
    /// in the Room view).
    public static let standard = LayerPrefs(off: [.furniture])

    public static func load(_ d: UserDefaults = .standard) -> LayerPrefs {
        guard let raw = d.stringArray(forKey: defaultsKey) else { return standard }
        return LayerPrefs(off: Set(raw.compactMap(FieldLayer.init(rawValue:))))
    }

    public func save(_ d: UserDefaults = .standard) {
        d.set(off.map(\.rawValue).sorted(), forKey: Self.defaultsKey)
    }

    /// The package trades to hide in AR for these prefs.
    public func hiddenTrades(_ trades: [String], kindsByTrade: [String: [String]] = [:]) -> Set<String> {
        Set(trades.filter { t in
            let layer = FieldLayer.of(trade: t, kind: kindsByTrade[t]?.first)
            return layer.map { off.contains($0) } ?? false
        })
    }
}
