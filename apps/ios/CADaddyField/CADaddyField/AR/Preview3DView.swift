import Combine
import RealityKit
import SwiftUI

/// A non-AR orbit view of the package (works in the simulator and without a room). Drag to orbit,
/// pinch to zoom, tap an object for its card. Shows the frame rate.
struct Preview3DView: View {
    @ObservedObject var session: FieldSession
    @State private var preview = PreviewController()
    @State private var showCard = false

    var body: some View {
        ZStack(alignment: .topLeading) {
            PreviewContainer(session: session, controller: preview).ignoresSafeArea(edges: .bottom)
            VStack(alignment: .leading, spacing: 4) {
                Text(String(format: "%.0f fps", session.fps)).monospacedDigit()
                if session.buildProgress < 1 { ProgressView(value: session.buildProgress).frame(width: 160) }
                if !session.status.isEmpty { Text(session.status) }
            }
            .font(.caption)
            .padding(8)
            .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: 8))
            .padding()
        }
        .navigationTitle("3D preview")
        .navigationBarTitleDisplayMode(.inline)
        .onChange(of: session.selected) { _, v in showCard = v != nil }
        .sheet(isPresented: $showCard, onDismiss: { session.selected = nil }) {
            NavigationStack { if let i = session.selected { ElementCardView(session: session, index: i) } }
                .presentationDetents([.medium, .large])
        }
        .onDisappear { preview.stop() }
    }
}

struct PreviewContainer: UIViewRepresentable {
    @ObservedObject var session: FieldSession
    let controller: PreviewController

    func makeUIView(context: Context) -> ARView {
        let v = ARView(frame: .zero, cameraMode: .nonAR, automaticallyConfigureSession: false)
        controller.attach(v, session: session)
        return v
    }

    func updateUIView(_ v: ARView, context: Context) { controller.sync() }
}

@MainActor
final class PreviewController: NSObject {
    private weak var view: ARView?
    private weak var session: FieldSession?
    let builder = SceneBuilder()
    private let anchor = AnchorEntity(world: .zero)
    private let camera = PerspectiveCamera()
    private var sub: Cancellable?
    private var yaw: Float = 0.6, pitch: Float = 0.7, distance: Float = 8
    private var target = SIMD3<Float>(0, 1, 0)
    private var startYaw: Float = 0, startPitch: Float = 0, startDistance: Float = 8
    private var frames = 0
    private var fpsStart = Date()
    private var fpsSamples: [Double] = []
    private var built = false
    private var lastLayers = 0
    private var lastSelected: Int?
    private var autoOrbit = false

    func attach(_ v: ARView, session: FieldSession) {
        view = v
        self.session = session
        v.environment.background = .color(UIColor.secondarySystemBackground)
        anchor.addChild(builder.root)
        let light = DirectionalLight()
        light.light.intensity = 600
        light.look(at: .zero, from: SIMD3(3, 6, 4), relativeTo: nil)
        anchor.addChild(light)
        anchor.addChild(camera)
        v.scene.addAnchor(anchor)
        v.addGestureRecognizer(UIPanGestureRecognizer(target: self, action: #selector(pan(_:))))
        v.addGestureRecognizer(UIPinchGestureRecognizer(target: self, action: #selector(pinch(_:))))
        v.addGestureRecognizer(UITapGestureRecognizer(target: self, action: #selector(tap(_:))))
        #if DEBUG
        autoOrbit = ProcessInfo.processInfo.arguments.contains("-fpslog")
        #endif
        sub = v.scene.subscribe(to: SceneEvents.Update.self) { [weak self] e in
            MainActor.assumeIsolated { self?.tick(e.deltaTime) }
        }
        Task { await load() }
    }

    func stop() {
        builder.cancel()
        sub?.cancel()
    }

    private func load() async {
        guard let session else { return }
        await session.load()
        session.buildProgress = 0
        let t0 = Date()
        if session.record.format == .usdz {
            do { session.model = try await builder.buildUSDZ(url: session.library.modelURL(session.record), sidecar: session.sidecar) } catch {
                session.status = (error as? FieldError)?.message ?? "The USDZ could not be opened."
            }
        } else if let m = session.model {
            _ = await builder.build(m) { [weak session] p in session?.buildProgress = p }
        }
        session.buildProgress = 1
        let loadTime = Date().timeIntervalSince(t0)
        if let lo = session.model?.boundsMin, let hi = session.model?.boundsMax {
            target = (lo + hi) / 2
            distance = max(simd_length(hi - lo) * 0.9, 2)
        }
        built = true
        sync()
        place()
        #if DEBUG
        if autoOrbit { print(String(format: "FIELD-PERF build %.2f s, %d objects", loadTime, session.model?.elements.count ?? 0)) }
        #endif
        _ = loadTime
    }

    func sync() {
        guard built, let session else { return }
        var h = Hasher()
        h.combine(session.wallMode); h.combine(session.hiddenTrades); h.combine(session.hiddenKinds)
        let layers = h.finalize()
        if layers != lastLayers { builder.applyVisibility(session); lastLayers = layers }
        if lastSelected != session.selected { builder.select(session.selected, model: session.model); lastSelected = session.selected }
    }

    private func place() {
        let p = target + distance * SIMD3(cos(pitch) * sin(yaw), sin(pitch), cos(pitch) * cos(yaw))
        camera.look(at: target, from: p, relativeTo: nil)
    }

    private func tick(_ dt: TimeInterval) {
        frames += 1
        let now = Date()
        if autoOrbit, built { yaw += Float(dt) * 0.6; place() }
        if now.timeIntervalSince(fpsStart) >= 1 {
            let fps = Double(frames) / now.timeIntervalSince(fpsStart)
            session?.fps = fps
            frames = 0
            fpsStart = now
            #if DEBUG
            if autoOrbit, built {
                fpsSamples.append(fps)
                if fpsSamples.count == 10 {
                    let sorted = fpsSamples.sorted()
                    print(String(format: "FIELD-PERF fps avg %.1f min %.1f max %.1f over 10 s (orbiting); per second: ", fpsSamples.reduce(0, +) / 10, sorted.first ?? 0, sorted.last ?? 0)
                          + fpsSamples.map { String(format: "%.0f", $0) }.joined(separator: " "))
                }
            }
            #endif
        }
    }

    @objc private func pan(_ g: UIPanGestureRecognizer) {
        guard let v = view else { return }
        if g.state == .began { startYaw = yaw; startPitch = pitch }
        let t = g.translation(in: v)
        yaw = startYaw - Float(t.x) * 0.008
        pitch = min(max(startPitch + Float(t.y) * 0.008, -0.2), 1.5)
        place()
    }

    @objc private func pinch(_ g: UIPinchGestureRecognizer) {
        if g.state == .began { startDistance = distance }
        distance = min(max(startDistance / Float(max(g.scale, 0.05)), 0.5), 500)
        place()
    }

    @objc private func tap(_ g: UITapGestureRecognizer) {
        guard let v = view, let session else { return }
        let p = g.location(in: v)
        if let e = v.entity(at: p), let i = ARController.elementIndex(e) { session.selected = i } else { session.selected = nil }
    }
}
