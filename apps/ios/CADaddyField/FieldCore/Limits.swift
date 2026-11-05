// CADaddy Field — hard caps for hostile or huge packages.
// Every input-derived size or count is checked against these before memory is reserved.

import Foundation

/// Hard caps applied while reading packages. A package over a cap is refused with a message,
/// never loaded partially in a way that could exhaust memory.
public enum FieldLimits {
    /// Largest model file (.glb/.usdz) the app accepts.
    public static let maxModelBytes: Int64 = 400 * 1024 * 1024
    /// Largest sidecar .json.
    public static let maxSidecarBytes: Int64 = 32 * 1024 * 1024
    /// Largest glTF JSON chunk.
    public static let maxGLBJSONBytes: Int = 64 * 1024 * 1024
    /// glTF object counts.
    public static let maxNodes = 250_000
    public static let maxMeshes = 100_000
    public static let maxAccessors = 1_000_000
    public static let maxMaterials = 50_000
    public static let maxPrimitivesPerMesh = 1_024
    public static let maxVerticesPerPrimitive = 4_000_000
    public static let maxTotalTriangles = 3_000_000
    public static let maxInstancesPerNode = 100_000
    /// Node hierarchy depth and total node visits (cycles are refused before this).
    public static let maxNodeDepth = 64
    public static let maxNodeVisits = 1_000_000
    /// Zip containers.
    public static let maxZipEntries = 10_000
    public static let maxZipTotalBytes: Int64 = 1024 * 1024 * 1024
    /// Folder imports: entries looked at.
    public static let maxFolderEntries = 10_000
    /// Sidecar element rows.
    public static let maxSidecarElements = 250_000
    /// Text fields from a package are clipped to this many characters.
    public static let maxTextLength = 2_000
    /// Marks per package and photo size kept with a mark.
    public static let maxMarksPerPackage = 50_000
    public static let maxPhotoBytes = 4 * 1024 * 1024
}

/// An error with a message for the user. The app shows `message` as is.
public struct FieldError: Error, LocalizedError, Equatable, CustomStringConvertible {
    public let message: String
    public init(_ message: String) { self.message = message }
    public var errorDescription: String? { message }
    public var description: String { message }
}

extension String {
    /// Clipped to `FieldLimits.maxTextLength` characters.
    func clipped() -> String {
        count > FieldLimits.maxTextLength ? String(prefix(FieldLimits.maxTextLength)) + "…" : self
    }
}
