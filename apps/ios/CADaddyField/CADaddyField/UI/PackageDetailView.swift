import ARKit
import SwiftUI

struct PackageDetailView: View {
    @EnvironmentObject var app: AppModel
    @StateObject private var session: FieldSession
    @State private var showAR = false
    @State private var path = NavigationPath()

    init(record: PackageRecord, library: PackageLibrary) {
        _session = StateObject(wrappedValue: FieldSession(record: record, library: library))
    }

    enum Route: Hashable { case elements, marks, plan, preview, card(Int) }

    var body: some View {
        NavigationStack(path: $path) {
            List {
                if session.record.needsRealign == true {
                    Section {
                        Label("The anchor moved in this version. Open in AR and align again. The old anchor shows as a grey cross on the plan and in AR.",
                              systemImage: "scope").font(.subheadline).foregroundStyle(.orange)
                    }
                }
                Section {
                    Button {
                        showAR = true
                    } label: {
                        Label(app.library?.hasWorldMap(session.record) == true ? "Open in AR (room saved)" : "Open in AR", systemImage: "arkit")
                            .font(.headline)
                    }
                    .disabled(!ARWorldTrackingConfiguration.isSupported || session.loadError != nil)
                    if !ARWorldTrackingConfiguration.isSupported {
                        Text("AR needs an iPhone or iPad with ARKit world tracking. The lists, plan and 3D preview work here.")
                            .font(.caption).foregroundStyle(.secondary)
                    }
                    NavigationLink(value: Route.preview) { Label("3D preview", systemImage: "rotate.3d") }
                    NavigationLink(value: Route.plan) { Label("Plan", systemImage: "square.grid.2x2") }
                    NavigationLink(value: Route.elements) { Label("Objects", systemImage: "list.bullet.rectangle") }
                    NavigationLink(value: Route.marks) {
                        Label("Marks", systemImage: "flag").badge(session.marks.count)
                    }
                }
                Section("Package") {
                    LabeledContent("Model", value: "\(session.record.format.rawValue.uppercased()), \(ByteCountFormatter.string(fromByteCount: session.record.modelBytes, countStyle: .file))")
                    if let n = session.record.elementCount { LabeledContent("Objects", value: "\(n)") }
                    if let t = session.record.triangleCount { LabeledContent("Triangles", value: t.formatted()) }
                    LabeledContent(session.record.remoteName == nil ? "Opened from" : "From the office", value: session.record.remoteName ?? session.record.sourceName)
                    LabeledContent("Version", value: session.record.currentVersion.label)
                    if let u = session.record.updated { LabeledContent("Updated", value: u.formatted(date: .abbreviated, time: .shortened)) }
                    if let m = session.record.alignmentMethod {
                        LabeledContent("Last alignment", value: m.replacingOccurrences(of: "_", with: "-")
                                       + (session.record.residualInches.map { String(format: ", residual %.2f in", $0) } ?? ""))
                    }
                }
                Section("Anchor") {
                    let a = session.record.anchor ?? AnchorInfo(kind: "unknown")
                    LabeledContent("What", value: a.displayKind)
                    if let n = a.name { LabeledContent("Name", value: n) }
                    if let x = a.planX, let y = a.planY {
                        LabeledContent("Plan", value: String(format: "%.3f, %.3f", x, y) + (a.planUnits.map { " \($0)" } ?? ""))
                    }
                    if let l = a.level { LabeledContent("Level", value: l) }
                    if let old = session.record.previousAnchor, session.record.needsRealign == true {
                        LabeledContent("Was", value: [old.name, old.planX.flatMap { x in old.planY.map { String(format: "%.3f, %.3f", x, $0) } }]
                            .compactMap { $0 }.joined(separator: " · "))
                    }
                    Text("Quick placement: tap the real floor at this point, then turn the model to match a wall.")
                        .font(.caption).foregroundStyle(.secondary)
                }
                if !session.record.warnings.isEmpty {
                    Section("Notes") { ForEach(session.record.warnings, id: \.self) { Text($0).font(.caption) } }
                }
                if app.library?.hasWorldMap(session.record) == true {
                    Section {
                        Button("Forget the saved room", role: .destructive) {
                            if let u = app.library?.worldMapURL(session.record) { try? FileManager.default.removeItem(at: u) }
                            app.reload()
                        }
                    }
                }
                if let e = session.loadError { Section { Text(e).foregroundStyle(.red) } }
            }
            .navigationTitle(session.record.name)
            .navigationBarTitleDisplayMode(.inline)
            .navigationDestination(for: Route.self) { r in
                switch r {
                case .elements: ElementsListView(session: session)
                case .marks: MarksListView(session: session)
                case .plan: PlanView(session: session, picking: false, onPick: { _ in })
                case .preview: Preview3DView(session: session)
                case let .card(i): ElementCardView(session: session, index: i)
                }
            }
        }
        .task {
            await session.load()
            runDemo()
        }
        .fullScreenCover(isPresented: $showAR, onDismiss: { app.reload() }) {
            ARFieldScreen(session: session)
        }
    }

    private func runDemo() {
        switch app.demoScreen {
        case "elements": path.append(Route.elements)
        case "marks": path.append(Route.marks)
        case "plan": path.append(Route.plan)
        case "preview": path.append(Route.preview)
        case "card":
            if let i = session.elements.firstIndex(where: { $0.info.tag == "R6" }) { path.append(Route.card(i)) }
        default: break
        }
    }
}
