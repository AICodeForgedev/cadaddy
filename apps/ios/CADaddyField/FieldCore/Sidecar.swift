// CADaddy Field — the anchor sidecar .json written next to the model by `bim.export.ar`.
//
// Contract (Q6207 `arexport`): the sidecar names the export anchor — what it is (room corner, gridline
// intersection or named point), where it is, and its plan coordinates — so the model's origin can be
// matched to one real floor point. The model's floor is at y = 0, metres, Y up.
// The reader is lenient about key spelling; see README "Package format".

import Foundation
import simd

public struct AnchorInfo: Equatable, Codable, Sendable {
    /// room_corner | grid_intersection | named_point | unknown
    public var kind: String
    public var name: String?
    /// Plan coordinates of the anchor in the source drawing (drawing units as exported).
    public var planX: Double?
    public var planY: Double?
    public var planUnits: String?
    public var level: String?
    public var room: String?

    public init(kind: String, name: String? = nil, planX: Double? = nil, planY: Double? = nil, planUnits: String? = nil,
                level: String? = nil, room: String? = nil) {
        self.kind = kind; self.name = name; self.planX = planX; self.planY = planY; self.planUnits = planUnits
        self.level = level; self.room = room
    }

    enum CodingKeys: String, CodingKey { case kind, name, planX = "plan_x", planY = "plan_y", planUnits = "plan_units", level, room }

    public var displayKind: String {
        switch kind {
        case "room_corner": return "Room corner"
        case "grid_intersection": return "Gridline intersection"
        case "named_point": return "Named point"
        default: return "Model origin"
        }
    }
}

/// A named reference point in model space (metres) the user can pick for two-point alignment.
public struct ReferencePoint: Equatable, Codable, Sendable {
    public var name: String
    public var position: SIMD3<Float>
    public init(name: String, position: SIMD3<Float>) { self.name = name; self.position = position }
}

public struct Sidecar: Equatable, Sendable {
    public var formatVersion: Int?
    public var modelFile: String?
    public var anchor: AnchorInfo
    public var upAxis: String
    public var metersPerUnit: Double
    public var elements: [ElementInfo]
    public var points: [ReferencePoint]
    public var title: String?
    /// Keys the reader did not understand (reported, not fatal).
    public var notes: [String]

    public init(anchor: AnchorInfo, formatVersion: Int? = nil, modelFile: String? = nil, upAxis: String = "Y",
                metersPerUnit: Double = 1, elements: [ElementInfo] = [], points: [ReferencePoint] = [], title: String? = nil,
                notes: [String] = []) {
        self.anchor = anchor; self.formatVersion = formatVersion; self.modelFile = modelFile; self.upAxis = upAxis
        self.metersPerUnit = metersPerUnit; self.elements = elements; self.points = points; self.title = title; self.notes = notes
    }

    public static let empty = Sidecar(anchor: AnchorInfo(kind: "unknown"))

    /// Parses sidecar JSON. Refuses non-JSON, non-object roots and files over the cap; tolerates
    /// missing optional parts.
    public static func parse(_ data: Data) throws -> Sidecar {
        guard Int64(data.count) <= FieldLimits.maxSidecarBytes else { throw FieldError("The sidecar .json is too large.") }
        let root: Any
        do { root = try JSONSerialization.jsonObject(with: data, options: []) } catch {
            throw FieldError("The sidecar .json is not valid JSON.")
        }
        guard var dict = root as? [String: Any] else { throw FieldError("The sidecar .json is not a JSON object.") }
        if let inner = dict["cadaddy"] as? [String: Any] { for (k, v) in inner where dict[k] == nil { dict[k] = v } }
        return try parse(object: dict)
    }

    /// Parses from an already-decoded object (also used for glTF `asset.extras` / scene extras).
    public static func parse(object dict: [String: Any]) throws -> Sidecar {
        var notes: [String] = []
        var anchor = AnchorInfo(kind: "unknown")
        if let a = dict["anchor"] as? [String: Any] {
            anchor = parseAnchor(a)
        } else if dict["anchor"] != nil {
            notes.append("anchor is not an object")
        }
        let up = (ElementInfo.text(dict, ["up_axis", "upAxis", "up"]) ?? "Y").uppercased()
        guard up == "Y" || up == "+Y" else { throw FieldError("The package says its up axis is \(up); CADaddy Field reads Y-up exports.") }
        var mpu = ElementInfo.number(dict, ["meters_per_unit", "metersPerUnit"]) ?? 1
        if let u = ElementInfo.text(dict, ["units", "unit"])?.lowercased() {
            switch u {
            case "m", "meter", "meters", "metre", "metres": mpu = 1
            case "mm", "millimeter", "millimeters": mpu = 0.001
            case "cm": mpu = 0.01
            case "ft", "feet", "foot": mpu = Units.metersPerFoot
            case "in", "inch", "inches": mpu = Units.metersPerInch
            default: notes.append("unknown units \(u); metres assumed")
            }
        }
        guard mpu.isFinite, mpu > 0, mpu < 1000 else { throw FieldError("The package's unit scale is not usable.") }
        var elements: [ElementInfo] = []
        if let rows = dict["elements"] as? [Any] {
            guard rows.count <= FieldLimits.maxSidecarElements else { throw FieldError("The sidecar lists too many elements.") }
            for (i, row) in rows.enumerated() {
                if let e = ElementInfo.parse(row, fallbackID: "row\(i)") { elements.append(e) }
            }
        }
        var points: [ReferencePoint] = []
        if let rows = dict["points"] as? [Any] ?? dict["reference_points"] as? [Any] {
            for row in rows.prefix(1000) {
                guard let p = row as? [String: Any], let name = ElementInfo.text(p, ["name", "id"]) else { continue }
                if let v = vector(p["position"] ?? p["model"] ?? p["xyz"]) {
                    points.append(ReferencePoint(name: name, position: v * Float(mpu)))
                }
            }
        }
        var version: Int?
        if let v = dict["version"] as? NSNumber ?? dict["format_version"] as? NSNumber { version = v.intValue }
        return Sidecar(anchor: anchor, formatVersion: version,
                       modelFile: ElementInfo.text(dict, ["model", "model_file", "modelFile", "glb", "usdz"]),
                       upAxis: "Y", metersPerUnit: mpu, elements: elements, points: points,
                       title: ElementInfo.text(dict, ["title", "name", "scope_name"]), notes: notes)
    }

    static func parseAnchor(_ a: [String: Any]) -> AnchorInfo {
        let rawKind = (ElementInfo.text(a, ["kind", "type", "what"]) ?? "unknown").lowercased()
            .replacingOccurrences(of: " ", with: "_").replacingOccurrences(of: "-", with: "_")
        let kind: String
        switch rawKind {
        case "room_corner", "corner", "roomcorner": kind = "room_corner"
        case "grid_intersection", "gridline_intersection", "grid", "gridlines", "grid_crossing": kind = "grid_intersection"
        case "named_point", "point", "named": kind = "named_point"
        default: kind = "unknown"
        }
        var px = ElementInfo.number(a, ["plan_x", "planX", "x"])
        var py = ElementInfo.number(a, ["plan_y", "planY", "y"])
        if let arr = (a["plan"] ?? a["plan_xy"] ?? a["planXY"]) as? [Any], arr.count >= 2 {
            px = (arr[0] as? NSNumber)?.doubleValue ?? px
            py = (arr[1] as? NSNumber)?.doubleValue ?? py
        } else if let obj = a["plan"] as? [String: Any] {
            px = ElementInfo.number(obj, ["x"]) ?? px
            py = ElementInfo.number(obj, ["y"]) ?? py
        }
        if let x = px, !x.isFinite { px = nil }
        if let y = py, !y.isFinite { py = nil }
        return AnchorInfo(kind: kind, name: ElementInfo.text(a, ["name", "label", "description"]), planX: px, planY: py,
                          planUnits: ElementInfo.text(a, ["plan_units", "planUnits", "units"]),
                          level: ElementInfo.text(a, ["level", "storey", "floor"]), room: ElementInfo.text(a, ["room", "space"]))
    }

    static func vector(_ v: Any?) -> SIMD3<Float>? {
        guard let arr = v as? [Any], arr.count == 3 else { return nil }
        let xs = arr.compactMap { ($0 as? NSNumber)?.floatValue }
        guard xs.count == 3, xs.allSatisfy({ $0.isFinite }) else { return nil }
        return SIMD3(xs[0], xs[1], xs[2])
    }
}
