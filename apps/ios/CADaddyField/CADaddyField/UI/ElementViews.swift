import PhotosUI
import SwiftUI

struct ElementsListView: View {
    @ObservedObject var session: FieldSession
    @State private var query = ""

    var body: some View {
        let groups = Dictionary(grouping: filtered, by: { $0.info.trade ?? "unassigned" })
        List {
            if session.loading { ProgressView("Reading the model…") }
            if session.record.format == .usdz && session.model == nil {
                Text("USDZ packages are listed once they are opened in AR or the 3D preview.").font(.caption).foregroundStyle(.secondary)
            }
            ForEach(groups.keys.sorted(), id: \.self) { trade in
                Section {
                    ForEach(groups[trade] ?? []) { e in
                        NavigationLink(value: PackageDetailView.Route.card(e.index)) {
                            ElementRow(element: e, marks: session.marks(for: e.info.id))
                        }
                    }
                } header: {
                    HStack { Circle().fill(session.tradeColor(trade)).frame(width: 10, height: 10); Text(trade.capitalized) }
                }
            }
        }
        .searchable(text: $query, prompt: "Tag, kind, circuit or id")
        .navigationTitle("Objects")
    }

    var filtered: [FieldElement] {
        let q = query.trimmingCharacters(in: .whitespaces).lowercased()
        return session.elements.filter { e in
            guard !q.isEmpty else { return !e.info.isContext }
            return [e.info.tag, e.info.kind, e.info.circuit, e.info.panel, e.info.id, e.info.catalog].compactMap { $0?.lowercased() }.contains { $0.contains(q) }
        }
    }
}

struct ElementRow: View {
    let element: FieldElement
    let marks: [Mark]
    var body: some View {
        HStack {
            VStack(alignment: .leading) {
                Text(element.info.title).font(.body)
                HStack(spacing: 8) {
                    if let c = element.info.circuit { Text(c) }
                    if let h = element.info.mountingHeight { Text("\(Units.feetInches(h)) AFF") }
                }
                .font(.caption).foregroundStyle(.secondary)
            }
            Spacer()
            if let last = marks.last { MarkBadge(status: last.status) }
        }
    }
}

struct MarkBadge: View {
    let status: MarkStatus
    var body: some View {
        Text(status.label).font(.caption2.bold()).padding(.horizontal, 6).padding(.vertical, 2)
            .background(color.opacity(0.2), in: Capsule()).foregroundStyle(color)
    }
    var color: Color {
        switch status {
        case .installed: return .green
        case .missing: return .red
        case .moved: return .orange
        case .extra: return .purple
        }
    }
}

/// The metadata card: what the model says, what the room says (live, in AR), and the mark controls.
struct ElementCardView: View {
    @ObservedObject var session: FieldSession
    let index: Int
    var onMoved: (() -> Void)?
    @State private var note = ""
    @State private var photoItem: PhotosPickerItem?
    @State private var photo: Data?
    @State private var saved: String?

    var body: some View {
        if let e = session.element(index) {
            Form {
                Section("Model") {
                    row("Kind", e.info.kind)
                    row("Tag", e.info.tag)
                    row("Circuit", e.info.circuit)
                    row("Panel", e.info.panel)
                    row("Mounting height", e.info.mountingHeight.map { "\(Units.feetInches($0)) above finished floor" })
                    row("Catalog / product", e.info.catalog)
                    row("Trade", e.info.trade)
                    row("Element id", e.info.id)
                    row("Source", e.info.source)
                    ForEach(e.info.extra.keys.sorted(), id: \.self) { k in row(k, e.info.extra[k]) }
                }
                if session.isAR {
                    Section("In the room (live)") {
                        row("Distance from phone", session.liveDistance.map { Units.feetInches(Double($0)) } ?? "—")
                        row("Height above detected floor", session.liveHeight.map { Units.feetInches(Double($0)) } ?? (session.floorY == nil ? "floor not found yet" : "—"))
                        if let h = e.info.mountingHeight, let live = session.liveHeight {
                            row("Model vs as placed", Units.feetInches(Double(live) - h))
                        }
                    }
                }
                Section("Mark") {
                    TextField("Note (optional)", text: $note, axis: .vertical).lineLimit(1...4)
                    HStack {
                        PhotosPicker(selection: $photoItem, matching: .images) { Label(photo == nil ? "Photo" : "Photo added", systemImage: "photo") }
                        if session.isAR, let snap = session.snapshot {
                            Spacer()
                            Button { snap { data in Task { @MainActor in photo = data } } } label: { Label("Snapshot", systemImage: "camera.viewfinder") }
                        }
                    }
                    .buttonStyle(.borderless)
                    LazyVGrid(columns: [GridItem(.adaptive(minimum: 120))], spacing: 8) {
                        ForEach(MarkStatus.allCases, id: \.self) { s in
                            Button { mark(e, s) } label: { Text(s.label).frame(maxWidth: .infinity) }
                                .buttonStyle(.bordered).tint(MarkBadge(status: s).color)
                        }
                    }
                    if let saved { Text(saved).font(.caption).foregroundStyle(.secondary) }
                }
                let marks = session.marks(for: e.info.id)
                if !marks.isEmpty {
                    Section("Marks on this object") {
                        ForEach(marks) { m in MarkRow(mark: m, current: session.record.currentVersion) }
                    }
                }
            }
            .navigationTitle(e.info.title)
            .onChange(of: photoItem) { _, item in
                Task {
                    guard let data = try? await item?.loadTransferable(type: Data.self) else { return }
                    photo = await Task.detached { PhotoTools.jpeg(from: data) }.value
                }
            }
        } else {
            ContentUnavailableView("Object not found", systemImage: "questionmark")
        }
    }

    private func mark(_ e: FieldElement, _ s: MarkStatus) {
        if s == .moved, session.isAR, let onMoved {
            session.pendingMove = PendingMove(note: note, photo: photo)
            onMoved()
            return
        }
        session.addMark(element: e, status: s, note: note, photo: photo)
        saved = "Marked \(s.label.lowercased())" + (s == .moved ? " (no new spot outside AR)" : "")
        note = ""; photo = nil; photoItem = nil
    }

    @ViewBuilder private func row(_ label: String, _ value: String?) -> some View {
        LabeledContent(label) { Text(value ?? "—").multilineTextAlignment(.trailing).textSelection(.enabled) }
    }
}

struct PendingMove {
    var note: String
    var photo: Data?
}

struct MarkRow: View {
    let mark: Mark
    /// The package's current version: a mark made against another version says so.
    var current: PackageVersion?
    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack {
                MarkBadge(status: mark.status)
                Text([mark.elementKind, mark.elementTag].compactMap { $0 }.joined(separator: " ")).font(.subheadline)
                Spacer()
                Text(mark.time, style: .time).font(.caption).foregroundStyle(.secondary)
            }
            if let n = mark.note { Text(n).font(.caption) }
            if mark.elementGone == true {
                Label("element no longer in model", systemImage: "exclamationmark.triangle").font(.caption).foregroundStyle(.orange)
            }
            if let was = mark.previousElementID {
                Text("re-linked after a rebuild (was element \(was))").font(.caption2).foregroundStyle(.secondary)
            }
            if let v = mark.packageVersion, let c = current, v != c {
                Text("made against an earlier version (\(v.label))").font(.caption2).foregroundStyle(.secondary)
            }
            HStack(spacing: 8) {
                if mark.photo != nil { Label("photo", systemImage: "photo").font(.caption2) }
                if let p = mark.pointModel, p.count == 3 {
                    Text("new spot \(Units.feetInches(Double(p[0]))), \(Units.feetInches(Double(-p[2]))) from anchor, \(Units.feetInches(Double(p[1]))) high")
                        .font(.caption2)
                }
            }
            .foregroundStyle(.secondary)
        }
    }
}

struct MarksListView: View {
    @ObservedObject var session: FieldSession
    @State private var shareURL: URL?
    @State private var error: String?

    var body: some View {
        List {
            if session.marks.isEmpty {
                Text("No marks yet. Open an object's card and mark it installed, missing, moved or extra.").foregroundStyle(.secondary)
            }
            ForEach(MarkStatus.allCases, id: \.self) { s in
                let ms = session.marks.filter { $0.status == s }
                if !ms.isEmpty {
                    Section("\(s.label) (\(ms.count))") {
                        ForEach(ms) { m in MarkRow(mark: m, current: session.record.currentVersion) }
                            .onDelete { idx in session.deleteMarks(Set(idx.map { ms[$0].id })) }
                    }
                }
            }
            if let error { Text(error).foregroundStyle(.red) }
        }
        .navigationTitle("Marks")
        .toolbar {
            Button {
                do { shareURL = try session.exportMarks() } catch { self.error = (error as? FieldError)?.message ?? "Export failed." }
            } label: { Label("Export", systemImage: "square.and.arrow.up") }
            .disabled(session.marks.isEmpty)
        }
        .sheet(item: Binding(get: { shareURL.map(ShareItem.init) }, set: { shareURL = $0?.url })) { item in
            ShareSheet(items: [item.url])
        }
    }
}

struct ShareItem: Identifiable { let url: URL; var id: String { url.path } }

struct ShareSheet: UIViewControllerRepresentable {
    let items: [Any]
    func makeUIViewController(context: Context) -> UIActivityViewController { UIActivityViewController(activityItems: items, applicationActivities: nil) }
    func updateUIViewController(_ vc: UIActivityViewController, context: Context) {}
}

enum PhotoTools {
    /// Re-encodes an image as JPEG, at most 1600 px on the long side, under the per-photo cap.
    static func jpeg(from data: Data) -> Data? {
        guard let img = UIImage(data: data) else { return nil }
        return jpeg(img)
    }

    static func jpeg(_ img: UIImage) -> Data? {
        let maxSide: CGFloat = 1600
        let scale = min(1, maxSide / max(img.size.width, img.size.height, 1))
        let size = CGSize(width: img.size.width * scale, height: img.size.height * scale)
        let format = UIGraphicsImageRendererFormat()
        format.scale = 1
        let resized = UIGraphicsImageRenderer(size: size, format: format).image { _ in img.draw(in: CGRect(origin: .zero, size: size)) }
        for q in [0.7, 0.5, 0.3] as [CGFloat] {
            if let d = resized.jpegData(compressionQuality: q), d.count <= FieldLimits.maxPhotoBytes { return d }
        }
        return nil
    }
}
