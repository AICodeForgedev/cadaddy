import Foundation
import simd
import SwiftUI

enum WallMode: String, CaseIterable, Identifiable {
    case on = "On", translucent = "Translucent", off = "Off"
    var id: String { rawValue }
}

enum ARMode: Equatable {
    case starting
    case relocalizing
    case findingFloor
    case quickPlace
    case twoPointModel(Int)
    case twoPointReal(Int)
    case twoPointReview
    case aligned
    case markMoved(Int)
}

/// One opened package: its parsed model, layers, selection, marks, and (in AR) the alignment state.
@MainActor
final class FieldSession: ObservableObject {
    let library: PackageLibrary
    @Published var record: PackageRecord
    @Published var model: FieldModel?
    @Published var sidecar: Sidecar = .empty
    @Published var loadError: String?
    @Published var loading = false
    @Published var marks: [Mark] = []

    // Layers
    @Published var hiddenTrades: Set<String> = []
    @Published var hiddenKinds: Set<String> = []
    @Published var wallMode: WallMode = .translucent
    @Published var occlusion = true
    @Published var showHeights = false
    @Published var selected: Int?

    // AR
    @Published var mode: ARMode = .starting
    @Published var alignment: YawAlignment?
    @Published var lastFit: AlignmentFit?
    @Published var floorY: Float?
    @Published var status = ""
    @Published var liveDistance: Float?
    @Published var liveHeight: Float?
    @Published var fps: Double = 0
    @Published var buildProgress: Double = 0
    @Published var twoPointModel: [ReferencePoint] = []
    @Published var twoPointReal: [SIMD3<Float>] = []
    @Published var mappingReady = false
    @Published var roomSaved = false
    var cameraTransform: simd_float4x4?
    /// A "moved" mark waiting for the user to tap the new spot.
    var pendingMove: PendingMove?
    /// Set by the AR screen while it runs (enables live values and device poses on marks).
    var isAR = false
    /// Snapshot provider (the AR view), for mark photos.
    var snapshot: ((@escaping (Data?) -> Void) -> Void)?

    init(record: PackageRecord, library: PackageLibrary) {
        self.record = record
        self.library = library
        marks = library.marks(record)
    }

    func load() async {
        guard model == nil, !loading else { return }
        loading = true
        let lib = library, rec = record
        let result: Result<(FieldModel, Sidecar), FieldError> = await Task.detached(priority: .userInitiated) {
            do { return .success(try lib.loadModel(rec)) } catch {
                return .failure((error as? FieldError) ?? FieldError("The package could not be opened."))
            }
        }.value
        loading = false
        switch result {
        case let .success((m, s)):
            sidecar = s
            if record.format == .glb { model = m }
        case let .failure(e):
            loadError = e.message
        }
    }

    var elements: [FieldElement] { model?.elements ?? [] }

    func element(_ i: Int?) -> FieldElement? {
        guard let i, let m = model, m.elements.indices.contains(i) else { return nil }
        return m.elements[i]
    }

    func isVisible(_ e: FieldElement) -> Bool {
        if e.info.isContext && wallMode == .off { return false }
        if hiddenTrades.contains(e.info.trade ?? "unassigned") { return false }
        if hiddenKinds.contains(e.info.kind ?? "unknown") { return false }
        return true
    }

    /// Display colour for a trade: the first exported material colour of an element of that trade.
    func tradeColor(_ trade: String) -> Color {
        guard let m = model, let e = m.elements.first(where: { ($0.info.trade ?? "unassigned") == trade }),
              let part = e.parts.first, m.meshes.indices.contains(part.mesh), let c = m.meshes[part.mesh].primitives.first?.color else { return .gray }
        return Color(red: Double(c.x), green: Double(c.y), blue: Double(c.z))
    }

    func marks(for elementID: String) -> [Mark] { marks.filter { $0.elementID == elementID } }

    /// Where the anchor the room was last aligned to sits in this version's model frame (after an update moved it).
    var previousAnchorPosition: SIMD3<Float>? {
        guard record.needsRealign == true, let old = record.previousAnchor else { return nil }
        return record.anchor?.position(of: old)
    }

    // MARK: marks

    func addMark(element: FieldElement, status: MarkStatus, note: String, photo: Data?, pointWorld: SIMD3<Float>? = nil) {
        var photoName: String?
        if let photo {
            do { photoName = try library.savePhoto(photo, for: record) } catch { self.status = (error as? FieldError)?.message ?? "The photo could not be saved." }
        }
        var m = Mark(elementID: element.info.id, elementKind: element.info.kind, elementTag: element.info.tag, status: status,
                     note: note.isEmpty ? nil : String(note.prefix(2000)), photo: photoName, packageVersion: record.currentVersion)
        if isAR, let cam = cameraTransform {
            m.devicePoseWorld = Mark.flatten(cam)
            if let a = alignment { m.devicePoseModel = Mark.flatten(a.matrix.inverse * cam) }
        }
        if let p = pointWorld {
            m.pointWorld = [p.x, p.y, p.z]
            if let a = alignment { let q = a.modelPoint(fromWorld: p); m.pointModel = [q.x, q.y, q.z] }
        }
        marks.append(m)
        persistMarks()
    }

    func deleteMarks(_ ids: Set<UUID>) {
        marks.removeAll { ids.contains($0.id) }
        persistMarks()
    }

    private func persistMarks() {
        do { try library.saveMarks(marks, for: record) } catch { status = (error as? FieldError)?.message ?? "Marks could not be saved." }
    }

    func exportMarks() throws -> URL { try library.exportMarks(record, app: AppModel.appVersion) }

    // MARK: alignment

    func setAlignment(_ a: YawAlignment, method: String, residualInches: Double?) {
        alignment = a
        record.alignmentMethod = method
        record.residualInches = residualInches
        // Aligned again after an update moved the anchor: the old anchor has done its job.
        if method != "relocalized", record.needsRealign == true { record.needsRealign = nil; record.previousAnchor = nil }
        try? library.save(record)
    }

    var residualText: String? {
        guard let f = lastFit else { return nil }
        return "Residual \(Units.inchesText(f.rms)) (max \(Units.inchesText(f.maxResidual)))"
    }
}
