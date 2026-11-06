import SwiftUI
import UniformTypeIdentifiers

struct LibraryView: View {
    @EnvironmentObject var app: AppModel
    @State private var showImporter = false
    @State private var columns: NavigationSplitViewVisibility = .all

    var body: some View {
        NavigationSplitView(columnVisibility: $columns) {
            List(selection: $app.selection) {
                if app.packages.isEmpty {
                    emptyState
                }
                ForEach(app.packages) { r in
                    PackageRow(record: r, hasRoom: app.library?.hasWorldMap(r) ?? false,
                               marks: app.library?.marks(r).count ?? 0)
                        .tag(r.id)
                }
                .onDelete { idx in app.delete(idx.map { app.packages[$0].id }) }
            }
            .navigationTitle("CADaddy Field")
            .toolbar {
                ToolbarItem(placement: .primaryAction) {
                    Menu {
                        Button { showImporter = true } label: { Label("Open from Files…", systemImage: "folder") }
                        Button { app.addSample() } label: { Label("Add sample room", systemImage: "cube.transparent") }
                        Button { app.addSample(rooms: 40) } label: { Label("Add large sample (40 rooms)", systemImage: "square.grid.3x3") }
                    } label: { Label("Add", systemImage: "plus") }
                }
            }
            .overlay { if app.importing { ProgressView("Opening…").padding().background(.regularMaterial, in: RoundedRectangle(cornerRadius: 12)) } }
        } detail: {
            if let r = app.record(app.selection), let lib = app.library {
                PackageDetailView(record: r, library: lib).id(r.id)
            } else {
                ContentUnavailableView("No package selected", systemImage: "cube.transparent",
                                       description: Text("Open a CADaddy AR export (.glb, .usdz, or a folder or .zip with its .json) from Files, AirDrop or the share sheet."))
            }
        }
        .fileImporter(isPresented: $showImporter, allowedContentTypes: [.glb, .usdz, .zip, .folder], allowsMultipleSelection: true) { result in
            if case let .success(urls) = result { app.importItems(urls) }
            if case let .failure(err) = result { app.alert = err.localizedDescription }
        }
        .alert("CADaddy Field", isPresented: Binding(get: { app.alert != nil }, set: { if !$0 { app.alert = nil } })) {
            Button("OK", role: .cancel) { app.alert = nil }
        } message: { Text(app.alert ?? "") }
    }

    private var emptyState: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("No packages yet").font(.headline)
            Text("Export a room from CADaddy with “Export for AR”, then open the .glb (or the folder or .zip with its .json) here. Packages stay on this device.")
                .font(.subheadline).foregroundStyle(.secondary)
            Button("Add sample room") { app.addSample() }.buttonStyle(.bordered)
        }
        .padding(.vertical, 8)
    }
}

struct PackageRow: View {
    let record: PackageRecord
    let hasRoom: Bool
    let marks: Int

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(record.name).font(.headline).lineLimit(2)
            HStack(spacing: 10) {
                Label(record.format.rawValue.uppercased(), systemImage: "cube")
                if let n = record.elementCount { Text("\(n) objects") }
                Text(ByteCountFormatter.string(fromByteCount: record.modelBytes, countStyle: .file))
            }
            .font(.caption).foregroundStyle(.secondary)
            HStack(spacing: 10) {
                if hasRoom { Label("Room saved", systemImage: "checkmark.seal").foregroundStyle(.green) }
                if marks > 0 { Label("\(marks) marks", systemImage: "flag") }
                Text(record.added, style: .date)
            }
            .font(.caption2).foregroundStyle(.secondary)
        }
        .padding(.vertical, 2)
    }
}
