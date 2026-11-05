// CADaddy Field — the on-device library of opened packages.
//
// Layout (Application Support/CADaddyField/Packages/<uuid>/, excluded from backups — packages stay on the device):
//   package.json      PackageRecord
//   model.glb|usdz    the model as opened
//   sidecar.json      the anchor sidecar, when the package had one
//   marks.json        field marks ([Mark])
//   photos/           mark photos (JPEG)
//   room.arworldmap   saved ARKit world map (written by the AR screen)

import CryptoKit
import Foundation

public enum ModelFormat: String, Codable, Sendable { case glb, usdz }

public struct PackageRecord: Codable, Equatable, Identifiable, Sendable {
    public var id: UUID
    public var name: String
    public var modelFile: String
    public var format: ModelFormat
    public var sidecarFile: String?
    public var sourceName: String
    public var added: Date
    public var modelBytes: Int64
    public var sha256: String
    public var elementCount: Int?
    public var triangleCount: Int?
    public var anchor: AnchorInfo?
    /// How the room was last aligned (quick | two_point | relocalized) and the two-point residual.
    public var alignmentMethod: String?
    public var residualInches: Double?
    public var warnings: [String]
}

public final class PackageLibrary: @unchecked Sendable {
    public let root: URL
    private let fm = FileManager.default

    public init(root: URL) throws {
        self.root = root
        try fm.createDirectory(at: root, withIntermediateDirectories: true)
        var url = root
        var values = URLResourceValues()
        values.isExcludedFromBackup = true
        try? url.setResourceValues(values)
    }

    public static func defaultRoot() -> URL {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
            ?? FileManager.default.temporaryDirectory
        return base.appendingPathComponent("CADaddyField/Packages", isDirectory: true)
    }

    public func folder(_ id: UUID) -> URL { root.appendingPathComponent(id.uuidString, isDirectory: true) }
    public func modelURL(_ r: PackageRecord) -> URL { folder(r.id).appendingPathComponent(r.modelFile) }
    public func sidecarURL(_ r: PackageRecord) -> URL? { r.sidecarFile.map { folder(r.id).appendingPathComponent($0) } }
    public func worldMapURL(_ r: PackageRecord) -> URL { folder(r.id).appendingPathComponent("room.arworldmap") }
    public func photosURL(_ r: PackageRecord) -> URL { folder(r.id).appendingPathComponent("photos", isDirectory: true) }
    public func hasWorldMap(_ r: PackageRecord) -> Bool { fm.fileExists(atPath: worldMapURL(r).path) }

    public func list() -> [PackageRecord] {
        let dirs = (try? fm.contentsOfDirectory(at: root, includingPropertiesForKeys: nil)) ?? []
        return dirs.compactMap { d in
            guard let data = try? Data(contentsOf: d.appendingPathComponent("package.json")) else { return nil }
            return try? MarksExport.decoder().decode(PackageRecord.self, from: data)
        }.sorted { $0.added > $1.added }
    }

    public func save(_ r: PackageRecord) throws {
        let data = try MarksExport.encoder().encode(r)
        try data.write(to: folder(r.id).appendingPathComponent("package.json"), options: .atomic)
    }

    public func delete(_ id: UUID) throws { try fm.removeItem(at: folder(id)) }

    // MARK: marks

    public func marks(_ r: PackageRecord) -> [Mark] {
        guard let data = try? Data(contentsOf: folder(r.id).appendingPathComponent("marks.json")) else { return [] }
        return (try? MarksExport.decoder().decode([Mark].self, from: data)) ?? []
    }

    public func saveMarks(_ marks: [Mark], for r: PackageRecord) throws {
        guard marks.count <= FieldLimits.maxMarksPerPackage else { throw FieldError("Too many marks in this package.") }
        let data = try MarksExport.encoder().encode(marks)
        try data.write(to: folder(r.id).appendingPathComponent("marks.json"), options: .atomic)
    }

    /// Stores JPEG bytes for a mark and returns the file name.
    public func savePhoto(_ jpeg: Data, for r: PackageRecord) throws -> String {
        guard jpeg.count <= FieldLimits.maxPhotoBytes else { throw FieldError("The photo is too large.") }
        try fm.createDirectory(at: photosURL(r), withIntermediateDirectories: true)
        let name = UUID().uuidString + ".jpg"
        try jpeg.write(to: photosURL(r).appendingPathComponent(name), options: .atomic)
        return name
    }

    /// Writes one marks export .json for the package and returns its URL (in the temporary folder, for sharing).
    public func exportMarks(_ r: PackageRecord, app: String) throws -> URL {
        let entries: [MarksExport.Entry] = marks(r).map { m in
            var photo: MarkPhoto?
            if let p = m.photo, ZipReader.safeName(p) == p, let d = try? Data(contentsOf: photosURL(r).appendingPathComponent(p)) {
                photo = MarkPhoto(name: p, mime: "image/jpeg", dataBase64: d.base64EncodedString())
            }
            return MarksExport.Entry(mark: m, photo: photo)
        }
        let x = MarksExport(app: app, package: .init(name: r.name, modelFile: r.modelFile, modelSHA256: r.sha256, anchor: r.anchor),
                            alignment: .init(method: r.alignmentMethod ?? "none", residualInches: r.residualInches), marks: entries)
        let safe = r.name.components(separatedBy: CharacterSet(charactersIn: "/\\:?*\"<>|")).joined(separator: "-")
        let dir = fm.temporaryDirectory.appendingPathComponent("marks-export", isDirectory: true)
        try fm.createDirectory(at: dir, withIntermediateDirectories: true)
        let url = dir.appendingPathComponent("\(safe) marks.json")
        try x.encoded().write(to: url, options: .atomic)
        return url
    }

    // MARK: import

    /// What an import found before it is copied in.
    struct Found {
        var modelName: String
        var modelData: Data
        var format: ModelFormat
        var sidecarData: Data?
    }

    /// Imports a .glb, a .usdz, a folder or a .zip holding the model and its sidecar .json.
    /// Malformed or oversized packages are refused with a FieldError; nothing is left behind.
    @discardableResult
    public func importItem(at url: URL) throws -> PackageRecord {
        var isDir: ObjCBool = false
        guard fm.fileExists(atPath: url.path, isDirectory: &isDir) else { throw FieldError("The file could not be found.") }
        let found: Found
        if isDir.boolValue {
            found = try scanFolder(url)
        } else {
            switch url.pathExtension.lowercased() {
            case "zip": found = try scanZip(url)
            case "glb", "usdz": found = try scanSingle(url)
            case "json": throw FieldError("Open the model (.glb or .usdz), or the folder or .zip that holds it, not the .json alone.")
            default: throw FieldError("CADaddy Field opens .glb, .usdz, a folder or a .zip with the model and its .json.")
            }
        }
        return try install(found, sourceName: url.lastPathComponent)
    }

    func scanSingle(_ url: URL) throws -> Found {
        let size = (try? url.resourceValues(forKeys: [.fileSizeKey]).fileSize).map(Int64.init) ?? 0
        guard size <= FieldLimits.maxModelBytes else { throw FieldError("The model is larger than \(FieldLimits.maxModelBytes / 1_048_576) MB.") }
        let data = try readData(url)
        let base = url.deletingPathExtension().lastPathComponent
        var sidecar: Data?
        for cand in ["\(base).json", "\(base).anchor.json", "\(base)_anchor.json", "\(base)-anchor.json"] {
            let s = url.deletingLastPathComponent().appendingPathComponent(cand)
            if fm.fileExists(atPath: s.path), let d = try? readData(s, max: FieldLimits.maxSidecarBytes) { sidecar = d; break }
        }
        return Found(modelName: url.lastPathComponent, modelData: data, format: try format(of: url.lastPathComponent), sidecarData: sidecar)
    }

    func scanFolder(_ url: URL) throws -> Found {
        guard let en = fm.enumerator(at: url, includingPropertiesForKeys: [.isRegularFileKey], options: [.skipsHiddenFiles, .skipsPackageDescendants]) else {
            throw FieldError("The folder can't be read.")
        }
        var files: [(rel: String, url: URL)] = []
        var seen = 0
        while let item = en.nextObject() as? URL {
            seen += 1
            guard seen <= FieldLimits.maxFolderEntries else { throw FieldError("The folder holds too many files.") }
            if en.level > 2 { en.skipDescendants(); continue }
            guard (try? item.resourceValues(forKeys: [.isRegularFileKey]).isRegularFile) == true else { continue }
            files.append((String(item.path.dropFirst(url.path.count)).trimmingCharacters(in: CharacterSet(charactersIn: "/")), item))
        }
        let names = files.map(\.rel)
        let pick = try Self.choose(names)
        guard let model = files.first(where: { $0.rel == pick.model }) else { throw FieldError("No model found.") }
        let data = try readData(model.url)
        let sidecar = try pick.sidecar.flatMap { s in try files.first(where: { $0.rel == s }).map { try readData($0.url, max: FieldLimits.maxSidecarBytes) } }
        return Found(modelName: (pick.model as NSString).lastPathComponent, modelData: data, format: try format(of: pick.model), sidecarData: sidecar)
    }

    func scanZip(_ url: URL) throws -> Found {
        let size = (try? url.resourceValues(forKeys: [.fileSizeKey]).fileSize).map(Int64.init) ?? 0
        guard size <= FieldLimits.maxZipTotalBytes else { throw FieldError("The zip is too large.") }
        let zip = try ZipReader(data: try readData(url, max: FieldLimits.maxZipTotalBytes))
        var byName: [String: ZipEntry] = [:]
        for e in zip.entries where !e.isDirectory {
            if let n = ZipReader.safeName(e.name) { byName[n] = e }
        }
        let pick = try Self.choose(Array(byName.keys))
        guard let me = byName[pick.model] else { throw FieldError("No model found in the zip.") }
        let modelData = try zip.extract(me)
        let sidecar = try pick.sidecar.flatMap { s in try byName[s].map { try zip.extract($0, maxBytes: FieldLimits.maxSidecarBytes) } }
        return Found(modelName: (pick.model as NSString).lastPathComponent, modelData: modelData, format: try format(of: pick.model), sidecarData: sidecar)
    }

    /// Picks the model (.glb preferred — it carries per-node metadata — then .usdz) and its sidecar
    /// (`<model>.json`, `<model>.anchor.json`, else the only .json, else one named like "anchor").
    static func choose(_ names: [String]) throws -> (model: String, sidecar: String?) {
        let sorted = names.sorted { ($0.split(separator: "/").count, $0) < ($1.split(separator: "/").count, $1) }
        let lower = { (s: String) in s.lowercased() }
        guard let model = sorted.first(where: { lower($0).hasSuffix(".glb") }) ?? sorted.first(where: { lower($0).hasSuffix(".usdz") }) else {
            throw FieldError("The package has no .glb or .usdz model.")
        }
        let jsons = sorted.filter { lower($0).hasSuffix(".json") }
        let base = lower((model as NSString).deletingPathExtension)
        let sidecar = jsons.first(where: { lower(($0 as NSString).deletingPathExtension).hasPrefix(base) })
            ?? (jsons.count == 1 ? jsons.first : nil)
            ?? jsons.first(where: { lower($0).contains("anchor") || lower($0).contains("sidecar") })
        return (model, sidecar)
    }

    func format(of name: String) throws -> ModelFormat {
        switch (name as NSString).pathExtension.lowercased() {
        case "glb": return .glb
        case "usdz": return .usdz
        default: throw FieldError("Unsupported model type.")
        }
    }

    func readData(_ url: URL, max: Int64 = FieldLimits.maxModelBytes) throws -> Data {
        let size = (try? url.resourceValues(forKeys: [.fileSizeKey]).fileSize).map(Int64.init) ?? 0
        guard size <= max else { throw FieldError("\(url.lastPathComponent) is too large.") }
        do { return try Data(contentsOf: url, options: .mappedIfSafe) } catch { throw FieldError("\(url.lastPathComponent) can't be read.") }
    }

    func install(_ f: Found, sourceName: String) throws -> PackageRecord {
        guard Int64(f.modelData.count) <= FieldLimits.maxModelBytes else { throw FieldError("The model is too large.") }
        var sidecar: Sidecar?
        if let s = f.sidecarData { sidecar = try Sidecar.parse(s) }
        var elementCount: Int?
        var triangles: Int?
        var warnings: [String] = sidecar?.notes ?? []
        switch f.format {
        case .glb:
            var model = try GLBReader.read(data: f.modelData)
            guard !model.elements.isEmpty else { throw FieldError("The model has no geometry to show.") }
            if sidecar == nil { sidecar = model.embeddedSidecar }
            if let s = sidecar { model.merge(sidecar: s) }
            elementCount = model.elements.count
            triangles = model.triangleCount
            warnings += model.warnings
        case .usdz:
            guard f.modelData.count >= 4, ZipReader.u32(f.modelData, 0) == 0x0403_4B50 else { throw FieldError("This .usdz file is not a valid USDZ package.") }
            _ = try ZipReader(data: f.modelData)
            elementCount = sidecar.map { $0.elements.isEmpty ? nil : $0.elements.count } ?? nil
            if sidecar?.elements.isEmpty ?? true {
                warnings.append("USDZ without a sidecar element list: each object's card shows only what its name says. Open the .glb for full metadata.")
            }
        }
        let id = UUID()
        let dir = folder(id)
        try fm.createDirectory(at: dir, withIntermediateDirectories: true)
        do {
            let modelFile = "model." + f.format.rawValue
            try f.modelData.write(to: dir.appendingPathComponent(modelFile), options: .atomic)
            var sidecarFile: String?
            if let s = f.sidecarData { sidecarFile = "sidecar.json"; try s.write(to: dir.appendingPathComponent("sidecar.json"), options: .atomic) }
            let digest = SHA256.hash(data: f.modelData).map { String(format: "%02x", $0) }.joined()
            let baseName = (f.modelName as NSString).deletingPathExtension
            let record = PackageRecord(id: id, name: (sidecar?.title ?? baseName).clipped(), modelFile: modelFile, format: f.format,
                                       sidecarFile: sidecarFile, sourceName: sourceName.clipped(), added: Mark.now(),
                                       modelBytes: Int64(f.modelData.count), sha256: digest, elementCount: elementCount,
                                       triangleCount: triangles, anchor: sidecar?.anchor, alignmentMethod: nil, residualInches: nil,
                                       warnings: Array(warnings.prefix(20)))
            try save(record)
            return record
        } catch {
            try? fm.removeItem(at: dir)
            throw (error as? FieldError) ?? FieldError("The package could not be saved on this device.")
        }
    }

    /// Loads the model + sidecar of a library package (GLB only; USDZ is loaded by RealityKit).
    public func loadModel(_ r: PackageRecord) throws -> (FieldModel, Sidecar) {
        var sidecar = Sidecar.empty
        if let s = sidecarURL(r), let d = try? Data(contentsOf: s) { sidecar = try Sidecar.parse(d) }
        guard r.format == .glb else { return (FieldModel(meshes: [], elements: []), sidecar) }
        var model = try GLBReader.read(url: modelURL(r))
        if r.sidecarFile == nil, let e = model.embeddedSidecar { sidecar = e }
        model.merge(sidecar: sidecar)
        return (model, sidecar)
    }

    public func loadSidecar(_ r: PackageRecord) -> Sidecar {
        guard let s = sidecarURL(r), let d = try? Data(contentsOf: s), let sc = try? Sidecar.parse(d) else { return .empty }
        return sc
    }
}
