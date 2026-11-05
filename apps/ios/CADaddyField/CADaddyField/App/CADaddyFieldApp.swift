// CADaddy Field — opens CADaddy AR packages, aligns them to the real room and shows each object's data.
// Apple frameworks only; no network, no accounts, no analytics. Packages and marks stay on the device.

import RealityKit
import SwiftUI
import UniformTypeIdentifiers

@main
struct CADaddyFieldApp: App {
    @StateObject private var app = AppModel()

    init() {
        ElementComponent.registerComponent()
    }

    var body: some SwiftUI.Scene {
        WindowGroup {
            LibraryView()
                .environmentObject(app)
                .onOpenURL { url in app.importItems([url]) }
        }
    }
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
    @Published var selection: UUID?
    @Published var importing = false
    @Published var alert: String?
    /// Debug-only screen automation for screenshots: `-demo library|elements|card|marks|preview`.
    let demoScreen: String?

    init() {
        library = try? PackageLibrary(root: PackageLibrary.defaultRoot())
        #if DEBUG
        let args = ProcessInfo.processInfo.arguments
        if let i = args.firstIndex(of: "-demo"), args.indices.contains(i + 1) { demoScreen = args[i + 1] } else { demoScreen = nil }
        #else
        demoScreen = nil
        #endif
        reload()
        if library == nil { alert = "CADaddy Field can't create its library folder on this device." }
        if demoScreen != nil { prepareDemo() }
    }

    func reload() {
        packages = library?.list() ?? []
        if let s = selection, !packages.contains(where: { $0.id == s }) { selection = nil }
    }

    func record(_ id: UUID?) -> PackageRecord? { packages.first { $0.id == id } }

    /// Imports files, folders or zips (from the Files picker, share sheet, AirDrop or "Open in").
    func importItems(_ urls: [URL]) {
        guard let library else { return }
        importing = true
        Task.detached(priority: .userInitiated) {
            let (last, errors) = Self.runImports(urls, library: library)
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
