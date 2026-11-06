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
    // Update (Q6218). All optional, so package.json files from before still read.
    /// The office manifest's package name this was last updated from (its stored source name for Update).
    public var remoteName: String? = nil
    /// SHA-256 of the stored sidecar, so Update can tell an unchanged package without re-reading it.
    public var sidecarSHA256: String? = nil
    /// The export time the office's manifest gave for this version.
    public var exported: String? = nil
    /// When Update last replaced this package.
    public var updated: Date? = nil
    /// The anchor the room was last aligned to, kept when an update moved the anchor.
    public var previousAnchor: AnchorInfo? = nil
    /// An update moved the anchor: the saved alignment no longer fits, align again.
    public var needsRealign: Bool? = nil

    /// The version marks and exports are made against.
    public var currentVersion: PackageVersion { PackageVersion(modelSHA256: sha256, exported: exported) }
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
        recoverInterruptedUpdates()
    }

    /// After a crash mid-update: a package folder that was moved aside is put back; leftover downloads and staged
    /// folders are removed.
    func recoverInterruptedUpdates() {
        let dirs = (try? fm.contentsOfDirectory(atPath: root.path)) ?? []
        for name in dirs where name.hasPrefix(".") {
            let u = root.appendingPathComponent(name, isDirectory: true)
            if name.hasPrefix(Self.oldPrefix), let id = UUID(uuidString: String(name.dropFirst(Self.oldPrefix.count))),
               !fm.fileExists(atPath: folder(id).path) {
                try? fm.moveItem(at: u, to: folder(id))
            } else if name.hasPrefix(Self.oldPrefix) || name.hasPrefix(Self.stagingPrefix) || name.hasPrefix(Self.incomingPrefix) {
                try? fm.removeItem(at: u)
            }
        }
    }

    static let oldPrefix = ".old-", stagingPrefix = ".staging-", incomingPrefix = ".incoming-"

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
        return dirs.filter { !$0.lastPathComponent.hasPrefix(".") }.compactMap { d in
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
        let x = MarksExport(app: app, package: .init(name: r.name, modelFile: r.modelFile, modelSHA256: r.sha256, anchor: r.anchor, version: r.currentVersion),
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

    /// What validation found in a package before anything is written.
    struct Validated {
        var sidecar: Sidecar?
        var elementCount: Int?
        var triangles: Int?
        var warnings: [String]
        /// The element ids in the model (nil when they can't be known: a USDZ without a sidecar element list).
        var elementIDs: Set<String>?
        /// Element id → its stable key (`devid`), for elements that have one.
        var elementKeys: [String: String] = [:]
    }

    /// The metadata key that names the same device across rebuilds (a rebuild re-issues element ids).
    static let stableKey = "devid"

    static func keys(_ infos: [ElementInfo]) -> [String: String] {
        var out: [String: String] = [:]
        for e in infos { if let k = e.extra[stableKey], !k.isEmpty { out[e.id] = k } }
        return out
    }

    /// Element id → stable key of a stored package (empty when it can't be read).
    func elementKeys(_ r: PackageRecord) -> [String: String] {
        if r.format == .glb, let (m, _) = try? loadModel(r) { return Self.keys(m.elements.map(\.info)) }
        return Self.keys(loadSidecar(r).elements)
    }

    /// Checks that the model opens (GLB parsed in full; USDZ a valid zip) and the sidecar parses. Throws a FieldError.
    func validate(_ f: Found) throws -> Validated {
        guard Int64(f.modelData.count) <= FieldLimits.maxModelBytes else { throw FieldError("The model is too large.") }
        var sidecar: Sidecar?
        if let s = f.sidecarData { sidecar = try Sidecar.parse(s) }
        var v = Validated(sidecar: nil, elementCount: nil, triangles: nil, warnings: sidecar?.notes ?? [], elementIDs: nil)
        switch f.format {
        case .glb:
            var model = try GLBReader.read(data: f.modelData)
            guard !model.elements.isEmpty else { throw FieldError("The model has no geometry to show.") }
            if sidecar == nil { sidecar = model.embeddedSidecar }
            if let s = sidecar { model.merge(sidecar: s) }
            v.elementCount = model.elements.count
            v.triangles = model.triangleCount
            v.warnings += model.warnings
            v.elementIDs = Set(model.elements.map(\.info.id))
            v.elementKeys = Self.keys(model.elements.map(\.info))
        case .usdz:
            guard f.modelData.count >= 4, ZipReader.u32(f.modelData, 0) == 0x0403_4B50 else { throw FieldError("This .usdz file is not a valid USDZ package.") }
            _ = try ZipReader(data: f.modelData)
            v.elementCount = sidecar.map { $0.elements.isEmpty ? nil : $0.elements.count } ?? nil
            if let els = sidecar?.elements, !els.isEmpty { v.elementIDs = Set(els.map(\.id)); v.elementKeys = Self.keys(els) }
            if sidecar?.elements.isEmpty ?? true {
                v.warnings.append("USDZ without a sidecar element list: each object's card shows only what its name says. Open the .glb for full metadata.")
            }
        }
        v.sidecar = sidecar
        return v
    }

    static func sha256Hex(_ d: Data) -> String { SHA256.hash(data: d).map { String(format: "%02x", $0) }.joined() }

    func install(_ f: Found, sourceName: String, label: String? = nil, remoteName: String? = nil, exported: String? = nil) throws -> PackageRecord {
        let v = try validate(f)
        let id = UUID()
        let dir = folder(id)
        try fm.createDirectory(at: dir, withIntermediateDirectories: true)
        do {
            let modelFile = "model." + f.format.rawValue
            try f.modelData.write(to: dir.appendingPathComponent(modelFile), options: .atomic)
            var sidecarFile: String?
            if let s = f.sidecarData { sidecarFile = "sidecar.json"; try s.write(to: dir.appendingPathComponent("sidecar.json"), options: .atomic) }
            let baseName = (f.modelName as NSString).deletingPathExtension
            var record = PackageRecord(id: id, name: (v.sidecar?.title ?? label ?? baseName).clipped(), modelFile: modelFile, format: f.format,
                                       sidecarFile: sidecarFile, sourceName: sourceName.clipped(), added: Mark.now(),
                                       modelBytes: Int64(f.modelData.count), sha256: Self.sha256Hex(f.modelData), elementCount: v.elementCount,
                                       triangleCount: v.triangles, anchor: v.sidecar?.anchor, alignmentMethod: nil, residualInches: nil,
                                       warnings: Array(v.warnings.prefix(20)))
            record.sidecarSHA256 = f.sidecarData.map(Self.sha256Hex)
            record.remoteName = remoteName
            record.exported = exported
            try save(record)
            return record
        } catch {
            try? fm.removeItem(at: dir)
            throw (error as? FieldError) ?? FieldError("The package could not be saved on this device.")
        }
    }

    // MARK: update (Q6218)

    /// Files an update downloaded and verified (size + SHA-256), waiting to be applied.
    public struct Incoming: Sendable {
        public var modelURL: URL
        /// The model's file name on the server (its extension picks GLB or USDZ).
        public var modelName: String
        public var sidecarURL: URL?
        public var remoteName: String
        public var label: String
        public var exported: String?
    }

    public struct UpdateResult: Sendable {
        public var record: PackageRecord?
        public var notes: [String]
    }

    /// A scratch folder inside the library for downloads (same volume, so moves are renames). Removed on next launch
    /// if the app stops mid-update.
    public func makeIncomingFolder() throws -> URL {
        let u = root.appendingPathComponent(Self.incomingPrefix + UUID().uuidString, isDirectory: true)
        try fm.createDirectory(at: u, withIntermediateDirectories: true)
        return u
    }

    /// SHA-256 of a package's stored sidecar (for packages opened before Update kept it).
    public func storedSidecarHash(_ r: PackageRecord) -> String? {
        guard let u = sidecarURL(r), let d = try? Data(contentsOf: u) else { return nil }
        return Self.sha256Hex(d)
    }

    /// Applies a verified download: checks the model opens, then adds it (`replacing` nil) or builds the new
    /// version of package `replacing` in a staging folder — new model and sidecar, the marks and photos carried
    /// over by element id, the saved room kept when the anchor is unchanged — and swaps the folders atomically.
    /// Any failure throws and leaves the old package as it was.
    public func applyUpdate(_ inc: Incoming, replacing id: UUID?) throws -> UpdateResult {
        let found = Found(modelName: inc.modelName, modelData: try readData(inc.modelURL), format: try format(of: inc.modelName),
                          sidecarData: try inc.sidecarURL.map { try readData($0, max: FieldLimits.maxSidecarBytes) })
        guard let id else {
            let r = try install(found, sourceName: inc.remoteName, label: inc.label, remoteName: inc.remoteName, exported: inc.exported)
            return UpdateResult(record: r, notes: [])
        }
        let v = try validate(found)
        guard let old = list().first(where: { $0.id == id }) else { throw FieldError("The package was removed from this device during the update.") }
        var notes: [String] = []
        let staging = root.appendingPathComponent(Self.stagingPrefix + UUID().uuidString, isDirectory: true)
        try fm.createDirectory(at: staging, withIntermediateDirectories: true)
        defer { try? fm.removeItem(at: staging) }
        do {
            let modelFile = "model." + found.format.rawValue
            try found.modelData.write(to: staging.appendingPathComponent(modelFile), options: .atomic)
            var sidecarFile: String?
            if let s = found.sidecarData { sidecarFile = "sidecar.json"; try s.write(to: staging.appendingPathComponent("sidecar.json"), options: .atomic) }
            var r = old
            r.name = (v.sidecar?.title ?? inc.label).clipped()
            r.modelFile = modelFile
            r.format = found.format
            r.sidecarFile = sidecarFile
            r.modelBytes = Int64(found.modelData.count)
            r.sha256 = Self.sha256Hex(found.modelData)
            r.sidecarSHA256 = found.sidecarData.map(Self.sha256Hex)
            r.elementCount = v.elementCount
            r.triangleCount = v.triangles
            r.anchor = v.sidecar?.anchor
            r.warnings = Array(v.warnings.prefix(20))
            r.remoteName = inc.remoteName
            r.exported = inc.exported
            r.updated = Mark.now()
            // The saved room: kept when the anchor is the same point; otherwise the old anchor is kept for display and
            // the user is asked to align again.
            let wasAligned = old.alignmentMethod != nil || hasWorldMap(old) || old.needsRealign == true
            if AnchorInfo.sameSpot(old.anchor, r.anchor) {
                if hasWorldMap(old) { try fm.copyItem(at: worldMapURL(old), to: staging.appendingPathComponent("room.arworldmap")) }
            } else if wasAligned {
                r.previousAnchor = old.needsRealign == true ? (old.previousAnchor ?? old.anchor) : old.anchor
                r.needsRealign = true
                r.alignmentMethod = nil
                r.residualInches = nil
                notes.append("the anchor moved in this version; open it in AR and align again (the old anchor shows as a grey cross)")
            }
            // Marks and photos carry over, matched by element id. A mark whose element is gone stays, flagged.
            let oldMarks = self.marks(old)
            let marks = Self.carryMarks(oldMarks, from: old.currentVersion, into: v.elementIDs,
                                        oldKeys: oldMarks.isEmpty ? [:] : elementKeys(old), newKeys: v.elementKeys)
            let gone = marks.filter { $0.elementGone == true }.count
            let relinked = zip(oldMarks, marks).filter { $0.elementID != $1.elementID }.count
            if relinked > 0 { notes.append("\(relinked) mark\(relinked == 1 ? "" : "s") re-linked to the same device under its new id") }
            if gone > 0 { notes.append("\(gone) mark\(gone == 1 ? "" : "s") on objects no longer in the model (kept)") }
            if !marks.isEmpty {
                try MarksExport.encoder().encode(marks).write(to: staging.appendingPathComponent("marks.json"), options: .atomic)
            }
            if fm.fileExists(atPath: photosURL(old).path) { try fm.copyItem(at: photosURL(old), to: staging.appendingPathComponent("photos", isDirectory: true)) }
            try MarksExport.encoder().encode(r).write(to: staging.appendingPathComponent("package.json"), options: .atomic)
            try swapFolders(staging, folder(id), id: id)
            return UpdateResult(record: r, notes: notes)
        } catch {
            throw (error as? FieldError) ?? FieldError("The new version could not be saved on this device; the old one is kept.")
        }
    }

    /// The marks of the old version for the new one, matched by element id. Each keeps the version it was made against.
    /// A mark whose id is gone is re-linked when its old element's stable key (`devid`) names exactly one element of the
    /// new model (the old id is kept as `element_id_was`); otherwise it stays, flagged "element no longer in model"
    /// (unflagged when it comes back). With `ids` unknown the flag is left as it was.
    public static func carryMarks(_ marks: [Mark], from version: PackageVersion, into ids: Set<String>?,
                                  oldKeys: [String: String] = [:], newKeys: [String: String] = [:]) -> [Mark] {
        var byKey: [String: String] = [:], dup = Set<String>()
        for (id, k) in newKeys { if byKey[k] != nil { dup.insert(k) } else { byKey[k] = id } }
        for k in dup { byKey[k] = nil }
        return marks.map { m in
            var m = m
            if m.packageVersion == nil { m.packageVersion = version }
            guard let ids else { return m }
            if ids.contains(m.elementID) { m.elementGone = nil; return m }
            if let k = oldKeys[m.elementID], let new = byKey[k], ids.contains(new) {
                m.previousElementID = m.previousElementID ?? m.elementID
                m.elementID = new
                m.elementGone = nil
                return m
            }
            m.elementGone = true
            return m
        }
    }

    /// Exchanges two folders in one step (APFS `renamex_np` RENAME_SWAP), so the package is always either the old or
    /// the new version. Afterwards `staged` holds the old version (the caller deletes it). Where the volume can't swap,
    /// the old folder is moved aside first and put back if the second move fails; a crash in between is undone on the
    /// next launch (`recoverInterruptedUpdates`).
    func swapFolders(_ staged: URL, _ live: URL, id: UUID) throws {
        if renamex_np(staged.path, live.path, UInt32(RENAME_SWAP)) == 0 { return }
        let aside = root.appendingPathComponent(Self.oldPrefix + id.uuidString, isDirectory: true)
        try? fm.removeItem(at: aside)
        try fm.moveItem(at: live, to: aside)
        do { try fm.moveItem(at: staged, to: live) } catch {
            try? fm.moveItem(at: aside, to: live)
            throw FieldError("The new version could not be put in place; the old one is kept.")
        }
        try? fm.moveItem(at: aside, to: staged)
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
