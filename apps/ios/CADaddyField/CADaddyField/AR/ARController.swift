import ARKit
import Combine
import RealityKit
import SwiftUI

struct ARViewContainer: UIViewRepresentable {
    @ObservedObject var session: FieldSession
    let controller: ARController

    func makeUIView(context: Context) -> ARView {
        let view = ARView(frame: .zero, cameraMode: .ar, automaticallyConfigureSession: false)
        controller.attach(view, session: session)
        return view
    }

    func updateUIView(_ view: ARView, context: Context) { controller.sync() }
}

/// Drives the AR session for one package.
@MainActor
final class ARController: NSObject, ARSessionDelegate {
    nonisolated static let alignmentAnchorName = "cadaddy.alignment"
    static let relocalizeTimeout: TimeInterval = 25

    weak var session: FieldSession?
    private weak var view: ARView?
    let builder = SceneBuilder()
    private let worldAnchor = AnchorEntity(world: .zero)
    private let markers = Entity()
    private var updateSub: Cancellable?
    private var alignmentAnchor: ARAnchor?
    private var relocalizeDeadline: Date?
    private var pendingSave = false
    private var frames = 0
    private var fpsStart = Date()
    private var lastLive = Date.distantPast
    private var planes: [UUID: (y: Float, isFloor: Bool)] = [:]
    private var panStartYaw: Float = 0
    private var applied = AppliedState()
    private var built = false

    struct AppliedState: Equatable {
        var layers = 0
        var occlusion: Bool?
        var selected: Int?
        var heights = false
        var alignment: YawAlignment?
    }

    // MARK: lifecycle

    func attach(_ view: ARView, session: FieldSession) {
        self.view = view
        self.session = session
        view.session.delegate = self
        view.renderOptions.insert(.disableMotionBlur)
        worldAnchor.addChild(builder.root)
        worldAnchor.addChild(builder.labelRoot)
        worldAnchor.addChild(markers)
        builder.root.isEnabled = false
        builder.root.addChild(Self.anchorMarker())
        view.scene.addAnchor(worldAnchor)
        view.addGestureRecognizer(UITapGestureRecognizer(target: self, action: #selector(tapped(_:))))
        view.addGestureRecognizer(UIPanGestureRecognizer(target: self, action: #selector(panned(_:))))
        updateSub = view.scene.subscribe(to: SceneEvents.Update.self) { [weak self] _ in
            MainActor.assumeIsolated { self?.onUpdate() }
        }
        session.snapshot = { [weak view] done in
            guard let view else { return done(nil) }
            view.snapshot(saveToHDR: false) { img in done(img.flatMap(PhotoTools.jpeg)) }
        }
        session.status = "Loading the model…"
        session.buildProgress = 0
        Task { await loadAndRun() }
    }

    func stop() {
        builder.cancel()
        updateSub?.cancel()
        view?.session.pause()
        session?.snapshot = nil
    }

    private func loadAndRun() async {
        guard let session else { return }
        await session.load()
        if let e = session.loadError { session.status = e; return }
        if session.record.format == .usdz {
            do { session.model = try await builder.buildUSDZ(url: session.library.modelURL(session.record), sidecar: session.sidecar) } catch {
                session.status = (error as? FieldError)?.message ?? "The USDZ could not be opened."; return
            }
            session.buildProgress = 1
        } else if let model = session.model {
            let warnings = await builder.build(model) { [weak session] p in session?.buildProgress = p }
            if !warnings.isEmpty { session.status = warnings.prefix(2).joined(separator: " ") }
        }
        built = true
        applied = AppliedState()
        sync()
        runSession()
    }

    private func runSession(useMap: Bool = true) {
        guard let view, let session else { return }
        let config = ARWorldTrackingConfiguration()
        config.planeDetection = [.horizontal, .vertical]
        if ARWorldTrackingConfiguration.supportsSceneReconstruction(.mesh) { config.sceneReconstruction = .mesh }
        config.environmentTexturing = .automatic
        var relocalizing = false
        let mapURL = session.library.worldMapURL(session.record)
        if useMap, let data = try? Data(contentsOf: mapURL) {
            if let map = try? NSKeyedUnarchiver.unarchivedObject(ofClass: ARWorldMap.self, from: data) {
                config.initialWorldMap = map
                relocalizing = true
            } else {
                session.status = "The saved room could not be read. Align again."
            }
        }
        planes.removeAll()
        session.floorY = nil
        view.session.run(config, options: [.resetTracking, .removeExistingAnchors])
        if relocalizing {
            session.mode = .relocalizing
            relocalizeDeadline = Date().addingTimeInterval(Self.relocalizeTimeout)
            session.status = "Look around the room slowly so it can be recognised…"
        } else {
            session.mode = .findingFloor
            if session.status.isEmpty || session.status.hasPrefix("Loading") { session.status = "Move the phone slowly to find the floor." }
        }
    }

    // MARK: state → scene

    func sync() {
        guard built, let session, let view else { return }
        var h = Hasher()
        h.combine(session.wallMode); h.combine(session.hiddenTrades); h.combine(session.hiddenKinds)
        let layers = h.finalize()
        if applied.layers != layers {
            builder.applyVisibility(session)
            applied.layers = layers
            builder.updateLabels(session)
        }
        let occl = session.occlusion && ARWorldTrackingConfiguration.supportsSceneReconstruction(.mesh)
        if applied.occlusion != occl {
            if occl { view.environment.sceneUnderstanding.options.insert(.occlusion) } else { view.environment.sceneUnderstanding.options.remove(.occlusion) }
            applied.occlusion = occl
        }
        if applied.selected != session.selected {
            builder.select(session.selected, model: session.model)
            applied.selected = session.selected
        }
        if applied.heights != session.showHeights {
            builder.updateLabels(session)
            applied.heights = session.showHeights
        }
        if applied.alignment != session.alignment {
            if let a = session.alignment {
                builder.root.transform = Transform(matrix: a.matrix)
                builder.root.isEnabled = true
            } else {
                builder.root.isEnabled = false
            }
            builder.positionLabels()
            applied.alignment = session.alignment
        }
    }

    private func onUpdate() {
        frames += 1
        let now = Date()
        if now.timeIntervalSince(fpsStart) >= 1 {
            session?.fps = Double(frames) / now.timeIntervalSince(fpsStart)
            frames = 0
            fpsStart = now
        }
        guard let session else { return }
        if let cam = session.cameraTransform {
            builder.faceLabels(camera: SIMD3(cam.columns.3.x, cam.columns.3.y, cam.columns.3.z))
        }
        if now.timeIntervalSince(lastLive) > 0.2 {
            lastLive = now
            updateLive()
            if session.mode == .relocalizing, let d = relocalizeDeadline, now > d {
                relocalizeDeadline = nil
                session.status = "Couldn't recognise this room (the light or furniture may have changed). Align again."
                runSession(useMap: false)
            }
        }
    }

    private func updateLive() {
        guard let session, let i = session.selected, let cam = session.cameraTransform, let pts = builder.worldPoints(i) else {
            if session?.liveDistance != nil { session?.liveDistance = nil; session?.liveHeight = nil }
            return
        }
        let c = SIMD3(cam.columns.3.x, cam.columns.3.y, cam.columns.3.z)
        let d = simd_distance(c, pts.center)
        if abs((session.liveDistance ?? -1) - d) > 0.003 { session.liveDistance = d }
        let h = session.floorY.map { pts.origin.y - $0 }
        if h != session.liveHeight { session.liveHeight = h }
    }

    // MARK: gestures

    @objc private func tapped(_ g: UITapGestureRecognizer) {
        guard let view, let session else { return }
        let p = g.location(in: view)
        switch session.mode {
        case .quickPlace:
            guard let hit = floorPoint(p) else { session.status = "No floor there yet. Move the phone to find the floor, then tap again."; return }
            let yaw = session.alignment?.yaw ?? cameraYaw()
            session.alignment = AlignmentSolver.quick(anchorAt: hit, floorY: session.floorY, yaw: yaw)
            session.status = "Drag sideways to turn the model until it matches a wall. Tap again to move the anchor."
        case let .twoPointReal(i):
            guard let hit = floorPoint(p) else { session.status = "No floor there yet. Move the phone to find the floor, then tap again."; return }
            session.twoPointReal.append(hit)
            addMarker(at: hit, color: .systemOrange)
            if i == 0 {
                session.mode = .twoPointModel(1)
                session.status = "Now pick point B on the plan."
            } else {
                solveTwoPoint()
            }
        case let .markMoved(i):
            guard let hit = anyPoint(p), let e = session.element(i) else { session.status = "Nothing found there. Aim at the wall or ceiling and tap again."; return }
            let pending = session.pendingMove
            session.addMark(element: e, status: .moved, note: pending?.note ?? "", photo: pending?.photo, pointWorld: hit)
            session.pendingMove = nil
            addMarker(at: hit, color: .systemOrange)
            session.mode = .aligned
            session.status = "Marked moved. The new spot is saved with the mark."
        case .aligned:
            if let ent = view.entity(at: p), let idx = Self.elementIndex(ent) {
                session.selected = idx
            } else {
                session.selected = nil
            }
        default:
            break
        }
    }

    @objc private func panned(_ g: UIPanGestureRecognizer) {
        guard let session, session.mode == .quickPlace, var a = session.alignment, let view else { return }
        if g.state == .began { panStartYaw = a.yaw }
        let dx = Float(g.translation(in: view).x)
        a.yaw = YawAlignment(yaw: panStartYaw - dx * 0.005).yaw
        a.rotate(steps: 0)
        session.alignment = a
    }

    static func elementIndex(_ e: Entity) -> Int? {
        var cur: Entity? = e
        var depth = 0
        while let c = cur, depth < 32 {
            if let comp = c.components[ElementComponent.self] { return comp.index }
            cur = c.parent
            depth += 1
        }
        return nil
    }

    /// A point on the real floor under the screen point; snapped to the detected floor height.
    private func floorPoint(_ p: CGPoint) -> SIMD3<Float>? {
        guard let view else { return nil }
        let hit = view.raycast(from: p, allowing: .existingPlaneGeometry, alignment: .horizontal).first
            ?? view.raycast(from: p, allowing: .estimatedPlane, alignment: .horizontal).first
        guard let h = hit else { return nil }
        var v = SIMD3(h.worldTransform.columns.3.x, h.worldTransform.columns.3.y, h.worldTransform.columns.3.z)
        if let f = session?.floorY { v.y = f }
        return v
    }

    /// Any real surface (walls, ceilings, the LiDAR mesh) under the screen point.
    private func anyPoint(_ p: CGPoint) -> SIMD3<Float>? {
        guard let view else { return nil }
        let hit = view.raycast(from: p, allowing: .existingPlaneGeometry, alignment: .any).first
            ?? view.raycast(from: p, allowing: .estimatedPlane, alignment: .any).first
        return hit.map { SIMD3($0.worldTransform.columns.3.x, $0.worldTransform.columns.3.y, $0.worldTransform.columns.3.z) }
    }

    /// The yaw that points the model's plan-north (−Z) where the camera looks.
    private func cameraYaw() -> Float {
        guard let cam = session?.cameraTransform else { return 0 }
        let f = -SIMD3(cam.columns.2.x, cam.columns.2.y, cam.columns.2.z)
        return atan2(-f.x, -f.z)
    }

    // MARK: alignment flows

    func startQuick() {
        guard let session else { return }
        session.selected = nil
        session.lastFit = nil
        session.mode = .quickPlace
        let what = session.record.anchor?.displayKind.lowercased() ?? "anchor"
        session.status = session.floorY == nil
            ? "Find the floor first: move the phone slowly over it. Then tap the real \(what)."
            : "Tap the real floor at the \(what) (the red cross on the plan)."
    }

    func finishQuick() {
        guard let session, let a = session.alignment else { return }
        session.setAlignment(a, method: "quick", residualInches: nil)
        session.mode = .aligned
        session.status = "Placed. Tap an object to see its data. Use Nudge to fine-tune."
        pendingSave = true
        saveRoom()
    }

    func startTwoPoint() {
        guard let session else { return }
        clearMarkers()
        session.selected = nil
        session.twoPointModel = []
        session.twoPointReal = []
        session.lastFit = nil
        session.mode = .twoPointModel(0)
        session.status = "Pick point A on the plan (a room corner or gridline crossing)."
    }

    func cancelTwoPoint() {
        guard let session else { return }
        clearMarkers()
        session.twoPointModel = []
        session.twoPointReal = []
        session.mode = session.alignment == nil ? .findingFloor : .aligned
        session.status = ""
    }

    private func solveTwoPoint() {
        guard let session else { return }
        let floor = session.floorY ?? (session.twoPointReal.map(\.y).reduce(0, +) / Float(max(session.twoPointReal.count, 1)))
        do {
            let fit = try AlignmentSolver.fit(model: session.twoPointModel.map(\.position), real: session.twoPointReal, floorY: floor)
            session.lastFit = fit
            session.alignment = fit.alignment
            session.mode = .twoPointReview
            session.status = "Check the fit, then Accept (or Redo)."
        } catch {
            session.status = (error as? FieldError)?.message ?? "The picks could not be fitted."
            clearMarkers()
            session.twoPointModel = []
            session.twoPointReal = []
            session.mode = .twoPointModel(0)
        }
    }

    func acceptTwoPoint() {
        guard let session, let fit = session.lastFit else { return }
        session.setAlignment(fit.alignment, method: "two_point", residualInches: Units.inches(fromMeters: fit.rms))
        clearMarkers()
        session.mode = .aligned
        session.status = "Aligned (\(Units.inchesText(fit.rms)) residual). Tap an object to see its data."
        pendingSave = true
        saveRoom()
    }

    func nudge(x: Int = 0, y: Int = 0, z: Int = 0) {
        guard let session, var a = session.alignment else { return }
        a.nudge(x: x, y: y, z: z)
        session.alignment = a
        session.roomSaved = false
    }

    func rotate(_ steps: Int) {
        guard let session, var a = session.alignment else { return }
        a.rotate(steps: steps)
        session.alignment = a
        session.roomSaved = false
    }

    // MARK: world map

    /// Saves the ARKit world map with an anchor holding the alignment, so reopening in the same room restores it.
    func saveRoom() {
        guard let view, let session, let a = session.alignment else { return }
        if let old = alignmentAnchor { view.session.remove(anchor: old) }
        let anchor = ARAnchor(name: Self.alignmentAnchorName, transform: a.matrix)
        view.session.add(anchor: anchor)
        alignmentAnchor = anchor
        if session.record.alignmentMethod == nil || session.mode == .aligned {
            session.setAlignment(a, method: session.record.alignmentMethod ?? "quick", residualInches: session.record.residualInches)
        }
        guard session.mappingReady else {
            pendingSave = true
            session.status = "Walk around the room a little so it can be saved…"
            return
        }
        pendingSave = false
        let url = session.library.worldMapURL(session.record)
        view.session.getCurrentWorldMap { [weak self] map, error in
            Task { @MainActor in
                guard let self, let session = self.session else { return }
                guard let map else {
                    session.status = "The room could not be saved yet (\(error?.localizedDescription ?? "not enough detail")). Walk around and try again."
                    return
                }
                do {
                    let data = try NSKeyedArchiver.archivedData(withRootObject: map, requiringSecureCoding: true)
                    try data.write(to: url, options: .atomic)
                    session.roomSaved = true
                    session.status = "Room saved. Opening this package here again restores the alignment."
                } catch {
                    session.status = "The room could not be saved on this device."
                }
            }
        }
    }

    // MARK: ARSessionDelegate

    nonisolated func session(_ arSession: ARSession, didUpdate frame: ARFrame) {
        let cam = frame.camera.transform
        let mapped = frame.worldMappingStatus == .mapped || frame.worldMappingStatus == .extending
        MainActor.assumeIsolated {
            guard let session = self.session else { return }
            session.cameraTransform = cam
            if session.mappingReady != mapped { session.mappingReady = mapped }
            if mapped, self.pendingSave, session.alignment != nil { self.saveRoom() }
        }
    }

    nonisolated func session(_ arSession: ARSession, didAdd anchors: [ARAnchor]) { handle(anchors) }
    nonisolated func session(_ arSession: ARSession, didUpdate anchors: [ARAnchor]) { handle(anchors) }

    nonisolated func session(_ arSession: ARSession, didRemove anchors: [ARAnchor]) {
        let ids = anchors.map(\.identifier)
        MainActor.assumeIsolated {
            for id in ids { self.planes.removeValue(forKey: id) }
            self.updateFloor()
        }
    }

    nonisolated func session(_ arSession: ARSession, didFailWithError error: Error) {
        let msg = error.localizedDescription
        MainActor.assumeIsolated { self.session?.status = "AR stopped: \(msg)" }
    }

    nonisolated func sessionShouldAttemptRelocalization(_ session: ARSession) -> Bool { true }

    private nonisolated func handle(_ anchors: [ARAnchor]) {
        var planeUpdates: [(UUID, Float, Bool)] = []
        var restored: simd_float4x4?
        var restoredAnchor: ARAnchor?
        for a in anchors {
            if let p = a as? ARPlaneAnchor, p.alignment == .horizontal {
                let y = (p.transform * SIMD4(p.center, 1)).y
                let isFloor = ARPlaneAnchor.isClassificationSupported && p.classification == .floor
                planeUpdates.append((p.identifier, y, isFloor))
            } else if a.name == ARController.alignmentAnchorName {
                restored = a.transform
                restoredAnchor = a
            }
        }
        MainActor.assumeIsolated {
            for (id, y, f) in planeUpdates { self.planes[id] = (y, f) }
            if !planeUpdates.isEmpty { self.updateFloor() }
            if let t = restored, let session = self.session {
                self.alignmentAnchor = restoredAnchor
                if session.mode == .relocalizing {
                    self.relocalizeDeadline = nil
                    session.alignment = YawAlignment(matrix: t)
                    session.setAlignment(YawAlignment(matrix: t), method: "relocalized", residualInches: session.record.residualInches)
                    session.mode = .aligned
                    session.roomSaved = true
                    session.status = "Room recognised. The alignment is restored."
                }
            }
        }
    }

    private func updateFloor() {
        guard let session else { return }
        let floors = planes.values.filter(\.isFloor).map(\.y)
        let candidate = floors.min() ?? planes.values.map(\.y).min()
        guard let y = candidate else { return }
        if session.floorY.map({ abs($0 - y) > 0.005 }) ?? true {
            let first = session.floorY == nil
            session.floorY = y
            if first, session.mode == .findingFloor { session.status = "Floor found. Tap Align to place the model." }
            if first, session.mode == .quickPlace { session.status = "Floor found. Tap the real floor at the anchor." }
        }
    }

    // MARK: markers

    private func addMarker(at p: SIMD3<Float>, color: UIColor) {
        let m = ModelEntity(mesh: .generateSphere(radius: 0.025), materials: [UnlitMaterial(color: color)])
        m.position = p
        markers.addChild(m)
    }

    private func clearMarkers() { markers.children.removeAll() }

    /// A red cross on the model's anchor (origin), so the user sees which real point it must sit on.
    static func anchorMarker() -> Entity {
        let e = Entity()
        let mat = UnlitMaterial(color: .systemRed)
        for size in [SIMD3<Float>(0.4, 0.004, 0.02), SIMD3<Float>(0.02, 0.004, 0.4)] {
            let bar = ModelEntity(mesh: .generateBox(size: size), materials: [mat])
            bar.position = SIMD3(0, 0.003, 0)
            e.addChild(bar)
        }
        return e
    }
}
