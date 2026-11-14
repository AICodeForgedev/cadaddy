// CADaddy Field — buildings (package sets) kept on the device, and Update for them.
//
// A set lives in its own folder: building.json, the package files it names (models and sidecars; the .usdz copies
// are not needed), and set.json (where it came from, which rooms were opened in AR). Update fetches the office's
// building.json beside its manifest.json, compares each package's content hash with the one on the device, and
// downloads only the packages whose models changed (size and SHA-256 checked); unchanged packages keep their files.
// The new version is built in a staging folder and swapped in at once, so the building is always the old version
// or the new one. Rooms opened in AR are library packages: Update replaces them through the library, which carries
// their marks over by element id (a mark whose element is gone stays, flagged).

import Foundation

/// What the device keeps about one building.
public struct SetRecord: Codable, Equatable, Identifiable, Sendable {
    public var id: UUID
    public var title: String
    public var added: Date
    public var updated: Date?
    public var exported: String?
    /// The office's building.json this set was updated from (nil = opened from Files).
    public var source: String?
    /// Room id → the library package made from that room's package (for AR and marks).
    public var roomRecords: [String: UUID] = [:]
}

public final class PackageSetStore: @unchecked Sendable {
    public let root: URL
    private let fm = FileManager.default
    static let recordFile = "set.json"
    static let stagingPrefix = ".staging-"

    public init(root: URL) throws {
        self.root = root
        try fm.createDirectory(at: root, withIntermediateDirectories: true)
        // A staging folder left by an update that stopped half way is never the live set.
        for name in (try? fm.contentsOfDirectory(atPath: root.path)) ?? [] where name.hasPrefix(Self.stagingPrefix) {
            try? fm.removeItem(at: root.appendingPathComponent(name))
        }
    }

    public static func defaultRoot() -> URL {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first ?? FileManager.default.temporaryDirectory
        return base.appendingPathComponent("Buildings", isDirectory: true)
    }

    public func folder(_ id: UUID) -> URL { root.appendingPathComponent(id.uuidString, isDirectory: true) }

    public func list() -> [SetRecord] {
        let names = (try? fm.contentsOfDirectory(atPath: root.path)) ?? []
        return names.compactMap { n -> SetRecord? in
            guard UUID(uuidString: n) != nil, let d = try? Data(contentsOf: root.appendingPathComponent(n).appendingPathComponent(Self.recordFile)) else { return nil }
            return try? JSONDecoder().decode(SetRecord.self, from: d)
        }.sorted { $0.added > $1.added }
    }

    public func save(_ r: SetRecord) throws {
        try JSONEncoder().encode(r).write(to: folder(r.id).appendingPathComponent(Self.recordFile), options: .atomic)
    }

    public func delete(_ id: UUID) throws { try fm.removeItem(at: folder(id)) }

    public func model(_ r: SetRecord) throws -> PackageSetModel { try PackageSetModel(root: folder(r.id)) }

    /// True when a folder holds a building's index (a package set), not a single package.
    public static func isSet(_ url: URL) -> Bool {
        var dir: ObjCBool = false
        return FileManager.default.fileExists(atPath: url.path, isDirectory: &dir) && dir.boolValue
            && FileManager.default.fileExists(atPath: url.appendingPathComponent("building.json").path)
    }

    /// Copies a set folder (from Files) onto the device: building.json and every model and sidecar it names that
    /// is there. A file the folder lacks is not an error here: that package says so when it is opened.
    public func importFolder(_ url: URL) throws -> SetRecord {
        let data = try Self.readIndex(url.appendingPathComponent("building.json"))
        let index = try PackageSetIndex.parse(data)
        let id = UUID()
        let dest = folder(id)
        try fm.createDirectory(at: dest, withIntermediateDirectories: true)
        do {
            try data.write(to: dest.appendingPathComponent("building.json"), options: .atomic)
            for p in index.allPackages {
                for f in [p.glb, p.sidecar].compactMap({ $0 }) {
                    let from = url.appendingPathComponent(f.path)
                    guard fm.fileExists(atPath: from.path) else { continue }
                    let to = dest.appendingPathComponent(f.path)
                    try fm.createDirectory(at: to.deletingLastPathComponent(), withIntermediateDirectories: true)
                    try fm.copyItem(at: from, to: to)
                }
            }
            let r = SetRecord(id: id, title: index.title, added: Date(), exported: index.exported)
            try save(r)
            return r
        } catch {
            try? fm.removeItem(at: dest)
            throw (error as? FieldError) ?? FieldError("The building could not be saved on this device.")
        }
    }

    static func readIndex(_ url: URL) throws -> Data {
        let size = (try? FileManager.default.attributesOfItem(atPath: url.path)[.size] as? NSNumber)?.int64Value ?? 0
        guard size > 0 else { throw FieldError("This folder has no building.json.") }
        guard size <= FieldLimits.maxBuildingIndexBytes else { throw FieldError("building.json is too large to open on this device.") }
        return try Data(contentsOf: url)
    }

    /// The library package for a room, made from the room's package the first time it is opened in AR and kept
    /// (with its marks and saved alignment) after that.
    public func roomRecord(_ r: SetRecord, room: String, library: PackageLibrary) throws -> (SetRecord, PackageRecord) {
        let m = try model(r)
        guard let pname = m.index.views.roomByRoom[room]?.first ?? m.index.room(id: room)?.package, let p = m.index.package(pname), p.fieldAlignment else {
            throw FieldError("This room has no package to align in the field.")
        }
        if let id = r.roomRecords[room], let rec = library.list().first(where: { $0.id == id }) { return (r, rec) }
        _ = try m.load(pname)   // present, unchanged and readable, or a plain message
        guard let glb = p.glb else { throw FieldError("\(m.title(of: pname)) has no model.") }
        let inc = PackageLibrary.Incoming(modelURL: folder(r.id).appendingPathComponent(glb.path), modelName: (glb.path as NSString).lastPathComponent,
                                          sidecarURL: p.sidecar.map { folder(r.id).appendingPathComponent($0.path) }.flatMap { fm.fileExists(atPath: $0.path) ? $0 : nil },
                                          remoteName: "\(r.title) — \(pname)", label: m.title(of: pname), exported: m.index.exported)
        guard let rec = try library.applyUpdate(inc, replacing: nil).record else { throw FieldError("The room could not be opened.") }
        var r2 = r
        r2.roomRecords[room] = rec.id
        try save(r2)
        return (r2, rec)
    }

    // MARK: update

    /// Builds the new version of set `r` (or a new set, `r` nil) in a staging folder from `index` and the files
    /// `fetch` downloads for the packages whose content hash changed, then swaps it in. `fetch(file, to)` saves one
    /// file and returns its size and SHA-256.
    func stageUpdate(_ r: SetRecord?, indexData: Data, source: String?,
                     fetch: (SetPackage.File, URL) async throws -> (Int64, String),
                     progress: (Int, Int, String) -> Void = { _, _, _ in }) async throws -> (SetRecord, SetUpdateReport) {
        let index = try PackageSetIndex.parse(indexData)
        let old: PackageSetIndex? = r.flatMap { try? model($0).index }
        let staging = root.appendingPathComponent(Self.stagingPrefix + UUID().uuidString, isDirectory: true)
        try fm.createDirectory(at: staging, withIntermediateDirectories: true)
        defer { try? fm.removeItem(at: staging) }
        var report = SetUpdateReport(title: index.title)
        let todo = index.allPackages.filter { p in
            guard let r, let o = old?.package(p.name), o.contentHash == p.contentHash, !p.contentHash.isEmpty, let g = o.glb else { return true }
            return !fm.fileExists(atPath: folder(r.id).appendingPathComponent(g.path).path)
        }
        let todoNames = Set(todo.map(\.name))
        // Unchanged packages: their files as they are on the device (hard links on the same volume).
        for p in index.allPackages where !todoNames.contains(p.name) {
            guard let r, let o = old?.package(p.name) else { continue }
            for (f, new) in [(o.glb, p.glb), (o.sidecar, p.sidecar)] {
                guard let f, let new else { continue }
                let from = folder(r.id).appendingPathComponent(f.path), to = staging.appendingPathComponent(new.path)
                guard fm.fileExists(atPath: from.path) else { continue }
                try fm.createDirectory(at: to.deletingLastPathComponent(), withIntermediateDirectories: true)
                do { try fm.linkItem(at: from, to: to) } catch { try fm.copyItem(at: from, to: to) }
            }
            report.unchanged.append(p.name)
        }
        for (i, p) in todo.enumerated() {
            try Task.checkCancellation()
            progress(i, todo.count, p.name)
            do {
                for f in [p.glb, p.sidecar].compactMap({ $0 }) {
                    let to = staging.appendingPathComponent(f.path)
                    try fm.createDirectory(at: to.deletingLastPathComponent(), withIntermediateDirectories: true)
                    let (size, sha) = try await fetch(f, to)
                    guard size == f.bytes else { throw FieldError("\((f.path as NSString).lastPathComponent) is \(size) bytes, not the \(f.bytes) the index says") }
                    // The sidecar's hash changes with every export (its time stamp); the model's never lies.
                    if f == p.glb { guard sha == f.sha256 else { throw FieldError("\((f.path as NSString).lastPathComponent) doesn't match its SHA-256") } }
                }
                if old?.package(p.name) == nil { report.added.append(p.name) } else { report.updated.append(p.name) }
            } catch {
                // The old version of this package stays when there is one (it opens with a plain message if it no
                // longer matches the new index); the rest of the building updates.
                if let r, let o = old?.package(p.name) {
                    for f in [o.glb, o.sidecar].compactMap({ $0 }) {
                        let from = folder(r.id).appendingPathComponent(f.path), to = staging.appendingPathComponent(f.path)
                        try? fm.removeItem(at: to)
                        if fm.fileExists(atPath: from.path) { try? fm.copyItem(at: from, to: to) }
                    }
                }
                report.failed.append((p.name, (error as? FieldError)?.message ?? "could not be downloaded"))
            }
        }
        try indexData.write(to: staging.appendingPathComponent("building.json"), options: .atomic)
        var rec = r ?? SetRecord(id: UUID(), title: index.title, added: Date())
        rec.title = index.title
        rec.exported = index.exported
        rec.updated = Date()
        rec.source = source ?? rec.source
        try JSONEncoder().encode(rec).write(to: staging.appendingPathComponent(Self.recordFile), options: .atomic)
        let live = folder(rec.id)
        if fm.fileExists(atPath: live.path) {
            if renamex_np(staging.path, live.path, UInt32(RENAME_SWAP)) != 0 {
                let aside = root.appendingPathComponent(Self.stagingPrefix + "old-" + rec.id.uuidString)
                try? fm.removeItem(at: aside)
                try fm.moveItem(at: live, to: aside)
                do { try fm.moveItem(at: staging, to: live) } catch { try? fm.moveItem(at: aside, to: live); throw error }
                try? fm.removeItem(at: aside)
            }
        } else {
            try fm.moveItem(at: staging, to: live)
        }
        report.changedRooms = Set(index.rooms.filter { r in r.package.map { todoNames.contains($0) && !report.failed.contains { $0.name == r.package } } ?? false }.map(\.id))
        return (rec, report)
    }

    /// After an update: the rooms opened in AR whose package changed get the new version through the library,
    /// which carries their marks over by element id. A room no longer in the building keeps its package.
    func refreshRooms(_ r: SetRecord, changed: Set<String>, library: PackageLibrary, report: inout SetUpdateReport) {
        guard let m = try? model(r) else { return }
        for (room, recID) in r.roomRecords where changed.contains(room) {
            guard let pname = m.index.views.roomByRoom[room]?.first, let p = m.index.package(pname), let glb = p.glb else { continue }
            let inc = PackageLibrary.Incoming(modelURL: folder(r.id).appendingPathComponent(glb.path), modelName: (glb.path as NSString).lastPathComponent,
                                              sidecarURL: p.sidecar.map { folder(r.id).appendingPathComponent($0.path) },
                                              remoteName: "\(r.title) — \(pname)", label: m.title(of: pname), exported: m.index.exported)
            do {
                let res = try library.applyUpdate(inc, replacing: recID)
                report.notes += res.notes.map { "\(m.title(of: pname)): \($0)" }
            } catch {
                report.failed.append((pname, (error as? FieldError)?.message ?? "the room's AR package could not be updated"))
            }
        }
    }
}

public struct SetUpdateReport: Equatable, Sendable {
    public var title: String
    public var updated: [String] = []
    public var added: [String] = []
    public var unchanged: [String] = []
    public var failed: [(name: String, message: String)] = []
    public var notes: [String] = []
    /// Rooms whose package changed in this update.
    public var changedRooms: Set<String> = []

    public var summary: String { "\(title): \(updated.count) updated, \(added.count) new, \(unchanged.count) unchanged, \(failed.count) failed" }

    public static func == (a: SetUpdateReport, b: SetUpdateReport) -> Bool {
        a.title == b.title && a.updated == b.updated && a.added == b.added && a.unchanged == b.unchanged && a.notes == b.notes
            && a.failed.map(\.name) == b.failed.map(\.name) && a.changedRooms == b.changedRooms
    }
}

/// Update for buildings: the office's building.json beside its manifest.json.
public final class SetUpdater: @unchecked Sendable {
    let store: PackageSetStore
    let library: PackageLibrary?
    let configuration: URLSessionConfiguration

    public init(store: PackageSetStore, library: PackageLibrary?, configuration: URLSessionConfiguration = PackageUpdater.defaultConfiguration()) {
        self.store = store; self.library = library; self.configuration = configuration
    }

    /// The building at the office address, updated or added; nil when the office serves no building.json there.
    public func run(manifestURL: URL, progress: @escaping @Sendable (UpdateProgress) -> Void = { _ in }) async throws -> SetUpdateReport? {
        guard let indexURL = UpdateServer.fileURL(manifest: manifestURL, name: "building.json") else { return nil }
        let work = store.root.appendingPathComponent(PackageSetStore.stagingPrefix + "dl-" + UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: work, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: work) }
        let local = work.appendingPathComponent("building.json")
        do { _ = try await CappedDownload.fetch(indexURL, to: local, maxBytes: FieldLimits.maxBuildingIndexBytes, configuration: configuration) } catch let e as FieldError {
            if e.message.hasPrefix("not found") { return nil }
            throw FieldError("The office's building.json could not be fetched: \(e.message).")
        }
        let data = try Data(contentsOf: local)
        let index = try PackageSetIndex.parse(data)
        let source = indexURL.absoluteString
        let existing = store.list().first { $0.source == source } ?? store.list().first { $0.source == nil && $0.title == index.title }
        let config = configuration
        var (rec, report) = try await store.stageUpdate(existing, indexData: data, source: source, fetch: { f, to in
            guard let url = UpdateServer.fileURL(manifest: indexURL, name: f.path) else { throw FieldError("\(f.path) is not a safe file name") }
            return try await CappedDownload.fetch(url, to: to, maxBytes: f.bytes, configuration: config)
        }, progress: { i, n, name in progress(UpdateProgress(done: i, total: n, current: name)) })
        if let library { store.refreshRooms(rec, changed: report.changedRooms, library: library, report: &report) }
        rec.updated = rec.updated ?? Date()
        return report
    }

    /// The package names a building's index covers (the per-package Update leaves them to this one).
    public static func packageNames(_ data: Data) -> Set<String> {
        guard let i = try? PackageSetIndex.parse(data) else { return [] }
        return Set(i.allPackages.map(\.name))
    }
}
