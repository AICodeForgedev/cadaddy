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
                ToolbarItem(placement: .topBarLeading) {
                    Button { app.tapUpdate() } label: { Label("Update", systemImage: "arrow.triangle.2.circlepath").labelStyle(.titleAndIcon) }
                        .disabled(app.updating || app.library == nil)
                        .contextMenu { Button { app.askAddress = true } label: { Label("Change office address…", systemImage: "network") } }
                        .accessibilityHint("Fetches the latest room packages from the office over the local network")
                }
                ToolbarItem(placement: .primaryAction) {
                    Menu {
                        Button { showImporter = true } label: { Label("Open from Files…", systemImage: "folder") }
                        Button { app.addSample() } label: { Label("Add sample room", systemImage: "cube.transparent") }
                        Button { app.addSample(rooms: 40) } label: { Label("Add large sample (40 rooms)", systemImage: "square.grid.3x3") }
                        Divider()
                        Button { app.askAddress = true } label: { Label("Office address for Update…", systemImage: "network") }
                    } label: { Label("Add", systemImage: "plus") }
                }
            }
            .overlay { if app.importing { ProgressView("Opening…").padding().background(.regularMaterial, in: RoundedRectangle(cornerRadius: 12)) } }
            .overlay(alignment: .bottom) { if let p = app.updateProgress { UpdateProgressView(progress: p) { app.cancelUpdate() }.padding() } }
        } detail: {
            if let r = app.record(app.selection), let lib = app.library {
                PackageDetailView(record: r, library: lib).id("\(r.id)|\(r.sha256)|\(r.sidecarSHA256 ?? "")") // a new version reloads
            } else {
                ContentUnavailableView("No package selected", systemImage: "cube.transparent",
                                       description: Text("Open a CADaddy AR export (.glb, .usdz, or a folder or .zip with its .json) from Files, AirDrop or the share sheet."))
            }
        }
        .fileImporter(isPresented: $showImporter, allowedContentTypes: [.glb, .usdz, .zip, .folder], allowsMultipleSelection: true) { result in
            if case let .success(urls) = result { app.importItems(urls) }
            if case let .failure(err) = result { app.alert = err.localizedDescription }
        }
        .sheet(isPresented: $app.askAddress) { ServerAddressSheet().environmentObject(app) }
        .sheet(item: Binding(get: { app.updateReport.map(ReportItem.init) }, set: { if $0 == nil { app.updateReport = nil } })) { item in
            UpdateReportSheet(report: item.report).environmentObject(app)
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

// MARK: - Update (Q6218)

struct ReportItem: Identifiable { let report: UpdateReport; let id = UUID() }

struct UpdateProgressView: View {
    let progress: UpdateProgress
    let cancel: () -> Void
    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Text(progress.total > 1 || progress.done > 0 ? "Updating \(min(progress.done + 1, progress.total)) of \(progress.total)" : "Updating…").font(.subheadline.bold())
                Spacer()
                Button("Cancel", action: cancel).font(.subheadline)
            }
            ProgressView(value: progress.fraction)
            Text(progress.current).font(.caption).foregroundStyle(.secondary).lineLimit(1)
        }
        .padding(12)
        .frame(maxWidth: 420)
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 12))
    }
}

struct ServerAddressSheet: View {
    @EnvironmentObject var app: AppModel
    @Environment(\.dismiss) private var dismiss
    @State private var text = ""
    @State private var error: String?

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField("http://192.168.1.20:8000/exports/", text: $text)
                        .keyboardType(.URL).textInputAutocapitalization(.never).autocorrectionDisabled()
                        .onSubmit(save)
                } header: { Text("Office address") } footer: {
                    Text("The folder of AR exports on the office computer, served on the local network (run bim.export.ar.manifest on it first). It may include a private path. The address is kept on this device only; there are no accounts and nothing goes to the cloud. Update only connects when you tap it.")
                }
                if let error { Section { Text(error).foregroundStyle(.red) } }
            }
            .navigationTitle("Update")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) { Button("Save and update", action: save).disabled(text.trimmingCharacters(in: .whitespaces).isEmpty) }
            }
            .onAppear { text = app.serverAddress }
        }
        .presentationDetents([.medium, .large])
    }

    private func save() {
        if let e = app.saveAddress(text) { error = e; return }
        dismiss()
        app.runUpdate()
    }
}

struct UpdateReportSheet: View {
    @EnvironmentObject var app: AppModel
    @Environment(\.dismiss) private var dismiss
    let report: UpdateReport

    var body: some View {
        NavigationStack {
            List {
                Section(report.summary) {
                    LabeledContent("Updated", value: "\(report.updated.count)")
                    LabeledContent("New", value: "\(report.added.count)")
                    LabeledContent("Unchanged", value: "\(report.unchanged.count)")
                    LabeledContent("Failed", value: "\(report.failed.count)").foregroundStyle(report.failed.isEmpty ? Color.primary : Color.red)
                }
                if !report.updated.isEmpty { Section("Updated") { ForEach(report.updated, id: \.self) { Text($0) } } }
                if !report.added.isEmpty { Section("New") { ForEach(report.added, id: \.self) { Text($0) } } }
                if !report.failed.isEmpty {
                    Section {
                        ForEach(Array(report.failed.enumerated()), id: \.offset) { _, f in
                            VStack(alignment: .leading) { Text(f.name).font(.subheadline.bold()); Text(f.message).font(.caption) }
                        }
                    } header: { Text("Failed") } footer: { Text("A package that failed keeps its old version on this device.") }
                }
                if !report.notes.isEmpty { Section("Notes") { ForEach(report.notes, id: \.self) { Text($0).font(.caption) } } }
            }
            .navigationTitle("Update finished")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } }
                ToolbarItem(placement: .bottomBar) { Button("Change office address…") { dismiss(); app.askAddress = true } }
            }
        }
    }
}
