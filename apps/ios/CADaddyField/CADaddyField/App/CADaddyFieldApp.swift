// CADaddy Field — opens CADaddy AR packages, aligns them to the real room and shows each object's data.
// Apple frameworks only; no accounts, no analytics. Packages and marks stay on the device. The only network use is the
// Update button: when tapped, it fetches the office's manifest.json over the local network (FieldCore/Update.swift).

import RealityKit
import SwiftUI
import UniformTypeIdentifiers

@main
struct CADaddyFieldApp: App {
    @StateObject private var app = AppModel()

    init() {
        ElementComponent.registerComponent()
        ElementTagComponent.registerComponent()
        RoomTagComponent.registerComponent()
        LevelTagComponent.registerComponent()
    }

    var body: some SwiftUI.Scene {
        WindowGroup {
            LibraryView()
                .environmentObject(app)
                .onOpenURL { url in app.importItems([url]) }
        }
    }
}

/// A building on screen: its set and the packages read to open it.
struct OpenBuilding: Identifiable {
    let id = UUID()
    let record: SetRecord
    let model: PackageSetModel
    let opening: [LoadedPackage]
    let failures: [String: String]
}

extension UTType {
    static let glb = UTType(importedAs: "org.khronos.gltf.binary")
}

/// App-wide state: the package library and import/alerts.
@MainActor
final class AppModel: ObservableObject {
    static let appVersion: String = {
        let v = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "0"
        return "CADaddy Field \(v)"
    }()

    let library: PackageLibrary?
    @Published var packages: [PackageRecord] = []
    /// Buildings opened from their package sets (Q6431).
    let sets: PackageSetStore?
    @Published var buildings: [SetRecord] = []
    /// The building on screen (its set read and its opening packages loaded).
    @Published var openBuilding: OpenBuilding?
    @Published var openingBuilding = false
    @Published var selection: UUID?
    @Published var importing = false
    @Published var alert: String?
    // Update (Q6218)
    @Published var updating = false
    @Published var updateProgress: UpdateProgress?
    @Published var updateReport: UpdateReport?
    @Published var askAddress = false
    @Published var serverAddress: String = UserDefaults.standard.string(forKey: UpdateServer.defaultsKey) ?? ""
    private var updateTask: Task<Void, Never>?
    /// Debug-only screen automation for screenshots: `-demo library|elements|card|marks|preview`.
    let demoScreen: String?
    // The building / floor / room design prototype (Q6421), on the made-up sample building.
    @Published var showBuilding = false
    /// Set by "Stand in this room": the package detail opens AR on the selected sample room.
    @Published var openARRequest = false

    init() {
        library = try? PackageLibrary(root: PackageLibrary.defaultRoot())
        sets = try? PackageSetStore(root: PackageSetStore.defaultRoot())
        #if DEBUG
        let args = ProcessInfo.processInfo.arguments
        if let i = args.firstIndex(of: "-demo"), args.indices.contains(i + 1) { demoScreen = args[i + 1] } else { demoScreen = nil }
        #else
        demoScreen = nil
        #endif
        reload()
        if library == nil { alert = "CADaddy Field can't create its library folder on this device." }
        if demoScreen != nil { prepareDemo() }
        #if DEBUG
        // `-updateserver URL` presets the address and `-autoupdate` taps Update once at launch (simulator end-to-end run).
        if let i = args.firstIndex(of: "-updateserver"), args.indices.contains(i + 1) { serverAddress = args[i + 1] }
        if args.contains("-autoupdate") { DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) { self.tapUpdate() } }
        if args.contains("-design"), !args.contains("-openset"), !args.contains("-bigset") { showBuilding = true }
        // `-openset FOLDER` imports a package set folder (once) and opens it; `-bigset N` generates a synthetic
        // building of N rooms (scale runs) the same way.
        if let i = args.firstIndex(of: "-openset"), args.indices.contains(i + 1) { debugOpenSet(URL(fileURLWithPath: args[i + 1])) }
        if let i = args.firstIndex(of: "-bigset"), args.indices.contains(i + 1), let n = Int(args[i + 1]) { debugBigSet(rooms: n) }
        #endif
    }

    // MARK: Update

    /// The Update button: asks for the address the first time, then runs one update. Nothing runs at any other time.
    func tapUpdate() {
        guard !updating else { return }
        if serverAddress.trimmingCharacters(in: .whitespaces).isEmpty { askAddress = true; return }
        runUpdate()
    }

    /// Checks and stores the address (on this device only). Returns an error message, or nil when it was saved.
    func saveAddress(_ text: String) -> String? {
        do { _ = try UpdateServer.manifestURL(text) } catch { return (error as? FieldError)?.message ?? "That address can't be used." }
        serverAddress = text.trimmingCharacters(in: .whitespacesAndNewlines)
        UserDefaults.standard.set(serverAddress, forKey: UpdateServer.defaultsKey)
        return nil
    }

    func runUpdate() {
        guard let library, !updating else { return }
        let sets = self.sets
        updating = true
        updateReport = nil
        updateProgress = UpdateProgress(done: 0, total: 1, current: "Checking the office for updates…")
        let address = serverAddress
        updateTask = Task { [weak self] in
            let updater = PackageUpdater(library: library)
            let outcome: Result<UpdateReport, FieldError>
            do {
                // A building's set first (only the packages whose content hash changed), then the single packages.
                var setNames = Set<String>()
                var setReport: SetUpdateReport?
                if let sets, let url = try? UpdateServer.manifestURL(address) {
                    setReport = try await SetUpdater(store: sets, library: library).run(manifestURL: url) { p in Task { @MainActor in self?.updateProgress = p } }
                    if let r = setReport { setNames = Set(r.updated + r.added + r.unchanged + r.failed.map(\.name)) }
                }
                var r = try await updater.run(address: address, excluding: setNames) { p in Task { @MainActor in self?.updateProgress = p } }
                if let s = setReport {
                    r.notes.insert(s.summary, at: 0)
                    r.notes += s.notes
                    r.failed += s.failed
                }
                outcome = .success(r)
            } catch {
                outcome = .failure((error as? FieldError) ?? FieldError("The update stopped: \(error.localizedDescription)"))
            }
            guard let self else { return }
            self.updating = false
            self.updateProgress = nil
            self.reload()
            switch outcome {
            case let .success(r): self.updateReport = r
            case let .failure(e): self.alert = e.message
            }
        }
    }

    func cancelUpdate() { updateTask?.cancel() }

    func reload() {
        packages = library?.list() ?? []
        buildings = sets?.list() ?? []
        if let s = selection, !packages.contains(where: { $0.id == s }) { selection = nil }
    }

    func record(_ id: UUID?) -> PackageRecord? { packages.first { $0.id == id } }

    /// Imports files, folders or zips (from the Files picker, share sheet, AirDrop or "Open in"). A folder with a
    /// building.json is a building's package set.
    func importItems(_ urls: [URL]) {
        guard let library else { return }
        importing = true
        let sets = self.sets
        Task.detached(priority: .userInitiated) {
            var plain: [URL] = [], setErrors: [String] = []
            for url in urls.prefix(20) {
                let scoped = url.startAccessingSecurityScopedResource()
                defer { if scoped { url.stopAccessingSecurityScopedResource() } }
                if PackageSetStore.isSet(url), let sets {
                    do { _ = try sets.importFolder(url) } catch { setErrors.append("\(url.lastPathComponent): \((error as? FieldError)?.message ?? "could not be opened.")") }
                } else {
                    plain.append(url)
                }
            }
            var (last, errors) = Self.runImports(plain, library: library)
            errors = setErrors + errors
            await MainActor.run {
                self.importing = false
                self.reload()
                if let l = last { self.selection = l }
                if !errors.isEmpty { self.alert = errors.joined(separator: "\n") }
            }
        }
    }

    nonisolated static func runImports(_ urls: [URL], library: PackageLibrary) -> (UUID?, [String]) {
        var errors: [String] = []
        var last: UUID?
        for url in urls.prefix(20) {
            let scoped = url.startAccessingSecurityScopedResource()
            defer { if scoped { url.stopAccessingSecurityScopedResource() } }
            do { last = try library.importItem(at: url).id } catch {
                errors.append("\(url.lastPathComponent): \((error as? FieldError)?.message ?? "could not be opened.")")
            }
        }
        return (last, errors)
    }

    nonisolated static func makeSample(rooms: Int, library: PackageLibrary) -> (UUID?, String?) {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("sample-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: dir) }
        do {
            let title = rooms > 1 ? "Sample Floor (\(rooms) rooms)" : SamplePackage.title
            try SamplePackage.write(to: dir, rooms: rooms, title: title)
            return (try library.importItem(at: dir).id, nil)
        } catch {
            return (nil, (error as? FieldError)?.message ?? "The sample could not be created.")
        }
    }

    /// Writes the built-in synthetic sample room and imports it.
    func addSample(rooms: Int = 1) {
        guard let library else { return }
        importing = true
        Task.detached(priority: .userInitiated) {
            let (id, message) = Self.makeSample(rooms: rooms, library: library)
            await MainActor.run {
                self.importing = false
                self.reload()
                if let id { self.selection = id }
                if let message { self.alert = message }
            }
        }
    }

    /// "Stand in this room" from the building prototype: open the sample room package and go to AR alignment.
    /// (The prototype is not wired to real exports yet, so it always uses the built-in sample room.)
    func standInSampleRoom() {
        showBuilding = false
        if let r = packages.first(where: { $0.name == SamplePackage.title }) {
            selection = r.id
            openARRequest = true
            return
        }
        guard let library else { return }
        importing = true
        Task.detached(priority: .userInitiated) {
            let (id, message) = Self.makeSample(rooms: 1, library: library)
            await MainActor.run {
                self.importing = false
                self.reload()
                if let id { self.selection = id; self.openARRequest = true }
                if let message { self.alert = message }
            }
        }
    }

    // MARK: buildings (package sets)

    /// Reads the building's set and its opening packages off the main thread, then shows it.
    func open(building r: SetRecord) {
        guard let sets, !openingBuilding else { return }
        openingBuilding = true
        let root = sets.folder(r.id)
        Task {
            do {
                let t0 = Date()
                let (m, opening, failed) = try await PackageSetModel.open(root: root)
                #if DEBUG
                print(String(format: "FIELD-OPEN %.2f s graph %d packages %d opening %d elements %.0f MB geometry", Date().timeIntervalSince(t0), m.index.graph.count,
                             m.index.allPackages.count, opening.reduce(0) { $0 + $1.elements.count }, Double(opening.reduce(0) { $0 + $1.bytes }) / 1_048_576))
                #endif
                self.openBuilding = OpenBuilding(record: r, model: m, opening: opening, failures: failed)
            } catch {
                self.alert = (error as? FieldError)?.message ?? "The building could not be opened."
            }
            self.openingBuilding = false
        }
    }

    /// "Stand in this room": the room's own package (made a library package the first time, with its marks and
    /// saved alignment after that), opened in AR alignment.
    func standIn(room: String, of r: SetRecord) {
        guard let sets, let library else { return }
        do {
            let latest = buildings.first { $0.id == r.id } ?? r
            let (_, rec) = try sets.roomRecord(latest, room: room, library: library)
            openBuilding = nil
            reload()
            selection = rec.id
            openARRequest = true
        } catch {
            alert = (error as? FieldError)?.message ?? "The room could not be opened."
        }
    }

    func deleteBuildings(_ ids: [UUID]) {
        for id in ids { try? sets?.delete(id) }
        reload()
    }

    #if DEBUG
    private func debugOpenSet(_ url: URL) {
        guard let sets else { return }
        let r = sets.list().first { $0.title == ((try? PackageSetIndex.parse(Data(contentsOf: url.appendingPathComponent("building.json"))))?.title ?? "") }
            ?? (try? sets.importFolder(url))
        reload()
        if let r { DispatchQueue.main.asyncAfter(deadline: .now() + 0.4) { self.open(building: r) } }
    }

    private func debugBigSet(rooms: Int) {
        guard let sets else { return }
        Task.detached(priority: .userInitiated) {
            let dir = FileManager.default.temporaryDirectory.appendingPathComponent("bigset-\(rooms)", isDirectory: true)
            if !FileManager.default.fileExists(atPath: dir.appendingPathComponent("building.json").path) {
                let t0 = Date()
                do { try SyntheticSet.write(to: dir, spec: .scale(rooms: rooms)) } catch { print("FIELD-BIGSET failed: \(error)"); return }
                print(String(format: "FIELD-BIGSET generated in %.1f s", Date().timeIntervalSince(t0)))
            }
            await MainActor.run {
                let title = SyntheticSet.Spec.scale(rooms: rooms).title
                let r = sets.list().first { $0.title == title } ?? (try? sets.importFolder(dir))
                self.reload()
                if let r { self.open(building: r) }
            }
        }
    }
    #endif

    func delete(_ ids: [UUID]) {
        for id in ids { try? library?.delete(id) }
        reload()
    }

    func update(_ r: PackageRecord) {
        try? library?.save(r)
        reload()
    }

    private func prepareDemo() {
        guard let library else { return }
        if library.list().isEmpty {
            let dir = FileManager.default.temporaryDirectory.appendingPathComponent("demo", isDirectory: true)
            let args = ProcessInfo.processInfo.arguments
            let rooms = args.firstIndex(of: "-demorooms").flatMap { args.indices.contains($0 + 1) ? Int(args[$0 + 1]) : nil } ?? 1
            _ = try? SamplePackage.write(to: dir, rooms: rooms, title: rooms > 1 ? "Sample Floor (\(rooms) rooms)" : SamplePackage.title)
            if let r = try? library.importItem(at: dir) {
                let marks = [
                    Mark(elementID: "E-1010", elementKind: "Receptacle", elementTag: "R1", status: .installed, note: "Plate on, height checks out"),
                    Mark(elementID: "E-1015", elementKind: "Receptacle", elementTag: "R6", status: .moved, note: "Set 6\" left of the model to clear the TV bracket",
                         pointWorld: [1.1, 1.52, -2.3], pointModel: [3.98, 1.524, -5.05]),
                    Mark(elementID: "E-1020", elementKind: "Data Outlet", elementTag: "D1", status: .missing),
                ]
                _ = try? library.saveMarks(marks, for: r)
            }
            try? FileManager.default.removeItem(at: dir)
        }
        reload()
        if demoScreen != "library" { selection = packages.first?.id }
    }
}
