// CADaddy Field — field marks (installed / missing / moved / extra) and their export JSON.
// Schema documented in README "Marks export". Version 1.

import Foundation
import simd

public enum MarkStatus: String, Codable, CaseIterable, Sendable {
    case installed, missing, moved, extra

    public var label: String {
        switch self {
        case .installed: return "Installed"
        case .missing: return "Missing"
        case .moved: return "Moved"
        case .extra: return "Extra"
        }
    }
}

public struct Mark: Codable, Equatable, Identifiable, Sendable {
    public var id: UUID
    public var elementID: String
    public var elementKind: String?
    public var elementTag: String?
    public var status: MarkStatus
    public var note: String?
    /// File name of the photo inside the package's photos folder.
    public var photo: String?
    public var time: Date
    /// Camera pose when marked, column-major 4×4, in the AR session's world frame and in the model frame.
    public var devicePoseWorld: [Float]?
    public var devicePoseModel: [Float]?
    /// For "moved": the tapped real-world spot, in the session world frame and in the model frame (metres).
    public var pointWorld: [Float]?
    public var pointModel: [Float]?
    /// The package version the mark was made against (updates keep the mark; this says which model it judged).
    public var packageVersion: PackageVersion?
    /// Set by an update when the element's id is no longer in the model. The mark is kept, never deleted.
    public var elementGone: Bool?

    public init(id: UUID = UUID(), elementID: String, elementKind: String? = nil, elementTag: String? = nil, status: MarkStatus,
                note: String? = nil, photo: String? = nil, time: Date = Mark.now(), devicePoseWorld: [Float]? = nil,
                devicePoseModel: [Float]? = nil, pointWorld: [Float]? = nil, pointModel: [Float]? = nil,
                packageVersion: PackageVersion? = nil, elementGone: Bool? = nil) {
        self.id = id; self.elementID = elementID; self.elementKind = elementKind; self.elementTag = elementTag; self.status = status
        self.note = note; self.photo = photo; self.time = time; self.devicePoseWorld = devicePoseWorld; self.devicePoseModel = devicePoseModel
        self.pointWorld = pointWorld; self.pointModel = pointModel
        self.packageVersion = packageVersion; self.elementGone = elementGone
    }

    /// The current time truncated to whole milliseconds, so it survives the JSON round trip exactly.
    public static func now() -> Date {
        Date(timeIntervalSince1970: (Date().timeIntervalSince1970 * 1000).rounded(.down) / 1000)
    }

    enum CodingKeys: String, CodingKey {
        case id, status, note, photo, time
        case elementID = "element_id", elementKind = "element_kind", elementTag = "element_tag"
        case devicePoseWorld = "device_pose_world", devicePoseModel = "device_pose_model"
        case pointWorld = "point_world", pointModel = "point_model"
        case packageVersion = "package_version", elementGone = "element_gone"
    }

    public static func flatten(_ m: simd_float4x4) -> [Float] {
        [m.columns.0, m.columns.1, m.columns.2, m.columns.3].flatMap { [$0.x, $0.y, $0.z, $0.w] }
    }
}

/// Which version of a package something was made against: the model's SHA-256 and, when the package came
/// through Update, the export time the office's manifest gave.
public struct PackageVersion: Codable, Equatable, Hashable, Sendable {
    public var modelSHA256: String
    public var exported: String?
    public init(modelSHA256: String, exported: String? = nil) { self.modelSHA256 = modelSHA256; self.exported = exported }
    enum CodingKeys: String, CodingKey { case modelSHA256 = "model_sha256", exported }

    /// "exported 2026-11-05T19:00:00Z" or "model 1a2b3c4d".
    public var label: String { exported.map { "exported \($0)" } ?? "model \(modelSHA256.prefix(8))" }
}

/// The photo embedded in an export (JPEG, base64).
public struct MarkPhoto: Codable, Equatable, Sendable {
    public var name: String
    public var mime: String
    public var dataBase64: String
    enum CodingKeys: String, CodingKey { case name, mime, dataBase64 = "data_base64" }
}

public struct MarksExport: Codable, Equatable, Sendable {
    public static let schemaID = "cadaddy-field-marks"
    public static let currentVersion = 1

    public struct PackageRef: Codable, Equatable, Sendable {
        public var name: String
        public var modelFile: String
        public var modelSHA256: String
        public var anchor: AnchorInfo?
        /// The package version the export was taken from (each mark carries its own `package_version` too).
        public var version: PackageVersion?
        public init(name: String, modelFile: String, modelSHA256: String, anchor: AnchorInfo?, version: PackageVersion? = nil) {
            self.name = name; self.modelFile = modelFile; self.modelSHA256 = modelSHA256; self.anchor = anchor; self.version = version
        }
        enum CodingKeys: String, CodingKey { case name, modelFile = "model_file", modelSHA256 = "model_sha256", anchor, version }
    }

    public struct AlignmentRef: Codable, Equatable, Sendable {
        /// quick | two_point | relocalized | none
        public var method: String
        public var residualInches: Double?
        enum CodingKeys: String, CodingKey { case method, residualInches = "residual_in" }
    }

    public struct Entry: Codable, Equatable, Sendable {
        public var mark: Mark
        public var photo: MarkPhoto?
        public init(mark: Mark, photo: MarkPhoto? = nil) { self.mark = mark; self.photo = photo }
        public init(from decoder: Decoder) throws {
            mark = try Mark(from: decoder)
            let c = try decoder.container(keyedBy: PhotoKey.self)
            photo = try c.decodeIfPresent(MarkPhoto.self, forKey: .photoData)
        }
        public func encode(to encoder: Encoder) throws {
            try mark.encode(to: encoder)
            var c = encoder.container(keyedBy: PhotoKey.self)
            try c.encodeIfPresent(photo, forKey: .photoData)
        }
        enum PhotoKey: String, CodingKey { case photoData = "photo_data" }
    }

    public var schema: String
    public var version: Int
    public var app: String
    public var exported: Date
    public var package: PackageRef
    public var alignment: AlignmentRef?
    public var marks: [Entry]

    public init(app: String, exported: Date = Mark.now(), package: PackageRef, alignment: AlignmentRef?, marks: [Entry]) {
        schema = Self.schemaID; version = Self.currentVersion; self.app = app; self.exported = exported
        self.package = package; self.alignment = alignment; self.marks = marks
    }

    public static func encoder() -> JSONEncoder {
        let e = JSONEncoder()
        e.outputFormatting = [.prettyPrinted, .sortedKeys]
        e.dateEncodingStrategy = .custom { date, enc in
            var c = enc.singleValueContainer()
            try c.encode(isoFormatter().string(from: date))
        }
        return e
    }

    public static func decoder() -> JSONDecoder {
        let d = JSONDecoder()
        d.dateDecodingStrategy = .custom { dec in
            let c = try dec.singleValueContainer()
            let s = try c.decode(String.self)
            if let date = isoFormatter().date(from: s) ?? ISO8601DateFormatter().date(from: s) { return date }
            throw DecodingError.dataCorruptedError(in: c, debugDescription: "bad date \(s)")
        }
        return d
    }

    static func isoFormatter() -> ISO8601DateFormatter {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return f
    }

    public func encoded() throws -> Data { try Self.encoder().encode(self) }

    public static func decode(_ data: Data) throws -> MarksExport {
        let x: MarksExport
        do { x = try decoder().decode(MarksExport.self, from: data) } catch { throw FieldError("This is not a CADaddy Field marks file.") }
        guard x.schema == schemaID else { throw FieldError("This is not a CADaddy Field marks file.") }
        guard x.version <= currentVersion else { throw FieldError("This marks file is from a newer version of CADaddy Field.") }
        return x
    }
}
