// CADaddy Field — per-element metadata as exported by CADaddy (`bim.export.ar`): glTF node `extras`,
// USD `customData`, or rows of the sidecar's optional `elements` list.

import Foundation

/// What the app knows about one exported element. Every field is optional except the id.
public struct ElementInfo: Equatable, Codable, Sendable {
    public var id: String
    public var kind: String?
    public var tag: String?
    public var circuit: String?
    public var panel: String?
    /// Mounting height above the finished floor of its level, metres.
    public var mountingHeight: Double?
    public var catalog: String?
    public var trade: String?
    public var source: String?
    /// The node name in the model file (used to match USDZ entities to sidecar rows).
    public var nodeName: String?
    /// Any other metadata keys, as text, sorted by key.
    public var extra: [String: String]

    public init(id: String, kind: String? = nil, tag: String? = nil, circuit: String? = nil, panel: String? = nil,
                mountingHeight: Double? = nil, catalog: String? = nil, trade: String? = nil, source: String? = nil,
                nodeName: String? = nil, extra: [String: String] = [:]) {
        self.id = id; self.kind = kind; self.tag = tag; self.circuit = circuit; self.panel = panel
        self.mountingHeight = mountingHeight; self.catalog = catalog; self.trade = trade; self.source = source
        self.nodeName = nodeName; self.extra = extra
    }

    /// A short title: "Receptacle R1", or the id when there is nothing better.
    public var title: String {
        let parts = [kind, tag].compactMap { $0 }.filter { !$0.isEmpty }
        return parts.isEmpty ? id : parts.joined(separator: " ")
    }

    /// Context elements (building shell) are shown dimmed, can be hidden or made translucent,
    /// and are not tappable.
    public var isContext: Bool {
        if let t = trade?.lowercased(), Self.contextTrades.contains(t) { return true }
        guard let k = kind?.lowercased() else { return false }
        return Self.contextKinds.contains(where: { k == $0 || k.hasPrefix($0 + " ") || k.hasPrefix($0 + "_") })
    }

    static let contextKinds = ["wall", "walls", "floor", "floors", "slab", "ceiling", "ceilings", "roof", "context", "room", "space", "door", "window", "column", "beam"]
    static let contextTrades = ["context", "architectural-context", "shell"]

    // MARK: parsing

    private static let idKeys = ["id", "element_id", "elementId", "elementID", "guid"]
    private static let kindKeys = ["kind", "type", "category"]
    private static let tagKeys = ["tag", "mark", "label"]
    private static let circuitKeys = ["circuit", "circuit_id", "circuitId"]
    private static let panelKeys = ["panel", "panel_tag", "panelTag"]
    private static let heightMeterKeys = ["mounting_height_m", "mountingHeightM", "height_m", "elevation_m"]
    private static let heightKeys = ["mounting_height", "mountingHeight", "height", "aff", "elevation"]
    private static let catalogKeys = ["catalog", "product", "catalog_number", "catalogNumber", "model"]
    private static let tradeKeys = ["trade", "system", "discipline"]
    private static let sourceKeys = ["source", "provenance", "from"]
    private static let nodeKeys = ["node", "node_name", "nodeName", "name", "prim", "path"]
    private static var knownKeys: Set<String> {
        Set(idKeys + kindKeys + tagKeys + circuitKeys + panelKeys + heightMeterKeys + heightKeys + catalogKeys
            + tradeKeys + sourceKeys + nodeKeys + ["cadaddy", "units"])
    }

    /// Reads metadata from a JSON object (node extras, customData or a sidecar row). Keys are matched
    /// leniently (snake_case or camelCase); a nested `"cadaddy": {...}` object is merged in.
    /// Returns nil when the object carries no element id and no kind (plain glTF extras).
    public static func parse(_ object: Any?, fallbackID: String? = nil, nodeName: String? = nil) -> ElementInfo? {
        guard var dict = object as? [String: Any] else { return nil }
        if let inner = dict["cadaddy"] as? [String: Any] {
            for (k, v) in inner { dict[k] = v }
        }
        let id = text(dict, idKeys)
        let kind = text(dict, kindKeys)
        guard id != nil || kind != nil else { return nil }
        guard let finalID = id ?? fallbackID ?? nodeName else { return nil }
        var height: Double?
        if let m = number(dict, heightMeterKeys) {
            height = m
        } else if let raw = first(dict, heightKeys) {
            if let n = raw as? NSNumber, !isJSONBool(raw) {
                let v = n.doubleValue
                // Exported in metres; values over 30 are taken as inches (a mis-unit guard, not a guess at feet).
                if v.isFinite { height = v > 30 ? Units.meters(fromInches: v) : v }
            } else if let s = raw as? String {
                height = Units.parseLength(s)
            }
        }
        var extra: [String: String] = [:]
        let known = knownKeys
        for (k, v) in dict where !known.contains(k) && extra.count < 64 {
            extra[String(k.prefix(64))] = describe(v)
        }
        return ElementInfo(id: finalID.clipped(), kind: kind, tag: text(dict, tagKeys), circuit: text(dict, circuitKeys),
                           panel: text(dict, panelKeys), mountingHeight: height, catalog: text(dict, catalogKeys),
                           trade: text(dict, tradeKeys)?.lowercased(), source: text(dict, sourceKeys),
                           nodeName: (text(dict, nodeKeys) ?? nodeName)?.clipped(), extra: extra)
    }

    /// Guesses kind and tag from a node name ("Receptacle_R12" → kind Receptacle, tag R12) when the
    /// file carries no metadata (a USDZ without a sidecar element list).
    public static func fromNodeName(_ name: String) -> ElementInfo {
        let cleaned = name.replacingOccurrences(of: "_", with: " ").trimmingCharacters(in: .whitespaces)
        let parts = cleaned.split(separator: " ", maxSplits: 1).map(String.init)
        return ElementInfo(id: name.clipped(), kind: parts.first, tag: parts.count > 1 ? parts[1] : nil, nodeName: name.clipped(),
                           extra: ["metadata": "from node name only"])
    }

    private static func first(_ d: [String: Any], _ keys: [String]) -> Any? {
        for k in keys { if let v = d[k], !(v is NSNull) { return v } }
        return nil
    }

    static func text(_ d: [String: Any], _ keys: [String]) -> String? {
        guard let v = first(d, keys) else { return nil }
        let s = describe(v).trimmingCharacters(in: .whitespacesAndNewlines)
        return s.isEmpty ? nil : s.clipped()
    }

    static func number(_ d: [String: Any], _ keys: [String]) -> Double? {
        guard let v = first(d, keys) else { return nil }
        if let n = v as? NSNumber, !isJSONBool(v) { let x = n.doubleValue; return x.isFinite ? x : nil }
        if let s = v as? String { return Units.parseLength(s) }
        return nil
    }

    /// True for JSON true/false (Swift bridges NSNumber 0/1 to Bool, so `is Bool` is not enough).
    static func isJSONBool(_ v: Any) -> Bool {
        guard let n = v as? NSNumber else { return false }
        return CFGetTypeID(n) == CFBooleanGetTypeID()
    }

    static func describe(_ v: Any) -> String {
        switch v {
        case let s as String: return s.clipped()
        case let n as NSNumber:
            if CFGetTypeID(n) == CFBooleanGetTypeID() { return n.boolValue ? "true" : "false" }
            return n.stringValue
        case let a as [Any]: return a.prefix(16).map(describe).joined(separator: ", ").clipped()
        case let d as [String: Any]:
            return d.keys.sorted().prefix(16).map { "\($0)=\(describe(d[$0] ?? ""))" }.joined(separator: "; ").clipped()
        default: return ""
        }
    }
}
