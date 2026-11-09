// CADaddy Field — the building / floor / room scene. One RealityKit scene and one camera from building to floor to
// room; nothing cuts between screens. The architecture is a pale ghost; the systems are the hero.
//
// Transparency without flicker: every ghost face uses ONE material (one colour, one opacity) that reads depth but
// never writes it. Blending n layers of the same colour and opacity gives the same result in any order, so the
// renderer's per-object sort can't make ghost faces flicker or swap. Ghost hairlines, systems and equipment are
// opaque and depth-tested as usual. Fading the rest of the building back during "follow the system" swaps to a
// second shared see-through material for the same reason.
//
// Depth: distant systems fade toward the backdrop by an opaque colour mix chosen per object (DepthFog), so there
// is nothing more to sort and no render-pipeline hook (ARView.renderCallbacks traps on the iOS 27 simulator; it is
// not used). The ground is one quad with one texture (a light pool and a contact shadow), so it can't sort wrong.
//
// Smoothness: the scene is built a few milliseconds per frame from a work queue; a floor's detail arrives in small
// chunks that fade in as a whole; the M/E/P/Cabinets/Furniture switches fade one group entity per layer, not every
// element; entering a room moves the few elements that serve it aside and fades the rest of the floor as a whole.

import Combine
import QuartzCore
import RealityKit
import simd
import UIKit

struct ElementTagComponent: Component { var id: String }
struct RoomTagComponent: Component { var number: String }
struct LevelTagComponent: Component { var index: Int }

enum DesignScope: Equatable {
    case building, floor(Int), room(String)
    var detail: DetailScope {
        switch self { case .building: return .building; case .floor: return .floor; case .room: return .room }
    }
}

@available(iOS 18.0, *)
@MainActor
final class BuildingRenderer: NSObject, UIGestureRecognizerDelegate {
    private weak var view: ARView?
    private weak var exp: BuildingExperience?
    private var b: SampleBuilding { exp?.building ?? SampleBuilding.sample() }
    private var style = DesignStyle()
    static let fov: Float = 42

    private let anchor = AnchorEntity(world: .zero)
    private let camera = PerspectiveCamera()
    private var sub: Cancellable?
    let labelLayer = UIView()
    private let leaderLayer = CAShapeLayer()

    // Per level
    private var levelRoots: [Entity] = []
    private var ghost: [Entity] = []
    private var cut: [Entity] = []
    private var mains: [Entity] = []
    private var detail: [Entity] = []
    private var serving: [Entity] = []
    private var detailBuilt: [Bool] = []
    private var tiles: [Entity] = []
    private var lift: [Float] = [], liftVel: [Float] = [], liftGoal: [Float] = []
    private var exterior: [(entity: Entity, x: Float)] = []
    private var ground = Entity()
    private var roomDetail: Entity?
    private var roomDetailNumber: String?

    private var elementEntities: [String: Entity] = [:]
    private var edgeEntities: [String: ModelEntity] = [:]
    private var roomTileEntities: [String: ModelEntity] = [:]
    private var raise: [String: (y: Float, v: Float, goal: Float)] = [:]
    private var risers: Set<String> = []
    private var roomServing: Set<String> = []
    /// Elements moved into a level's `serving` container while a room is open, with the container they came from.
    private var servingHome: [String: Entity] = [:]

    // Layer groups: one entity per (container, layer); the switches fade these, not the elements.
    private struct LayerGroup { weak var entity: Entity?; var layer: FieldLayer }
    private var groups: [ObjectIdentifier: [FieldLayer: Entity]] = [:]
    private var groupList: [LayerGroup] = []

    // Fades: OpacityComponent driven by a spring; disabled at 0.
    private struct Fade { weak var e: Entity?; var value: Float; var vel: Float; var goal: Float; var delay: Double }
    private var fades: [ObjectIdentifier: Fade] = [:]

    // Build work, run a few milliseconds per frame so nothing stutters.
    private var work: [() -> Void] = []
    private var workHead = 0
    static let workBudget: Double = 0.004

    // Materials
    private var ghostMat: RealityKit.Material = UnlitMaterial()
    private var edgeMat: RealityKit.Material = UnlitMaterial()
    private var fadedMat: RealityKit.Material = UnlitMaterial()
    private var matCache: [String: RealityKit.Material] = [:]
    private var originals: [ObjectIdentifier: [RealityKit.Material]] = [:]
    private var glowTexture: TextureResource?
    private var currentFocus: SystemFocus?

    // Depth fog
    private var fogStep: [String: Int] = [:]
    private var fogEye = SIMD3<Float>(repeating: .greatestFiniteMagnitude)
    private var fogDirty = true
    private var frameCount = 0

    // Camera: target and (yaw, pitch, distance), each on the design spring.
    private var target = SIMD3<Float>(37, 4, 9), targetVel = SIMD3<Float>.zero, targetGoal = SIMD3<Float>(37, 4, 9)
    private var orbit = SIMD3<Float>(-2.25, 0.56, 150), orbitVel = SIMD3<Float>.zero, orbitGoal = SIMD3<Float>(-2.25, 0.56, 150)
    private var panStart = SIMD3<Float>.zero
    private var pinchStart: Float = 1
    private let spring = Spring.design
    var autoOrbitSpeed: Float = 0
    /// What the camera is framing (world points, lifted), and whether the user has moved the camera since.
    private var framePoints: [SIMD3<Float>] = []
    private var userMoved = false
    private var lastFree: CGRect = .zero

    // Follow the system
    private var pulse: [ModelEntity] = []
    private var pulsePath: [(SIMD3<Float>, Int)] = []
    private var pulseT: Double = 0
    private var litIDs: Set<String> = []

    // Labels
    private var labels: [String: UILabel] = [:]
    private var labelAnchors: [String: (SIMD3<Float>, Int)] = [:]
    private var labelPriority: [String: Int] = [:]
    private var labelSlots: [String: Int] = [:]

    // Frame rate
    private var frames = 0
    private var fpsStart = Date()
    private var fpsLog: [Double] = []
    private var logPerf = false
    private var worstFrame: Double = 0

    // MARK: setup

    func attach(_ v: ARView, experience: BuildingExperience) {
        view = v
        exp = experience
        style = experience.style
        v.environment.background = .color(.clear)
        v.backgroundColor = .clear
        v.isOpaque = false
        v.renderOptions.insert(.disablePersonOcclusion)
        v.renderOptions.insert(.disableMotionBlur)
        v.renderOptions.insert(.disableDepthOfField)
        v.renderOptions.insert(.disableHDR)
        v.renderOptions.insert(.disableCameraGrain)
        v.renderOptions.insert(.disableGroundingShadows)
        camera.camera.fieldOfViewInDegrees = Self.fov
        camera.camera.fieldOfViewOrientation = .vertical
        camera.camera.near = 0.2
        camera.camera.far = 600
        anchor.addChild(camera)
        let key = DirectionalLight()
        key.light.intensity = 2600
        key.light.color = UIColor(hex: 0xFFF6EA)
        key.look(at: .zero, from: SIMD3(-30, 60, -40), relativeTo: nil)
        anchor.addChild(key)
        let fill = DirectionalLight()
        fill.light.intensity = 900
        fill.light.color = UIColor(hex: 0xDDEBFF)
        fill.look(at: .zero, from: SIMD3(50, 25, 60), relativeTo: nil)
        anchor.addChild(fill)
        v.scene.addAnchor(anchor)
        labelLayer.isUserInteractionEnabled = false
        labelLayer.frame = v.bounds
        labelLayer.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        leaderLayer.fillColor = nil
        leaderLayer.lineWidth = 1
        labelLayer.layer.addSublayer(leaderLayer)
        v.addSubview(labelLayer)

        let pan = UIPanGestureRecognizer(target: self, action: #selector(onPan(_:)))
        pan.maximumNumberOfTouches = 1
        v.addGestureRecognizer(pan)
        v.addGestureRecognizer(UIPinchGestureRecognizer(target: self, action: #selector(onPinch(_:))))
        v.addGestureRecognizer(UITapGestureRecognizer(target: self, action: #selector(onTap(_:))))
        #if DEBUG
        logPerf = ProcessInfo.processInfo.arguments.contains("-fpslog")
        #endif

        makeMaterials()
        buildStatic()
        sub = v.scene.subscribe(to: SceneEvents.Update.self) { [weak self] e in
            MainActor.assumeIsolated { self?.tick(e.deltaTime) }
        }
        snapCamera()
    }

    func stop() { sub?.cancel(); sub = nil }

    static func ui(_ c: SIMD3<Float>) -> UIColor { UIColor(red: CGFloat(c.x), green: CGFloat(c.y), blue: CGFloat(c.z), alpha: 1) }

    // MARK: work queue

    private func enqueue(_ f: @escaping () -> Void) { work.append(f) }

    private func runWork(budget: Double) {
        guard workHead < work.count else { return }
        let start = CACurrentMediaTime()
        while workHead < work.count {
            let f = work[workHead]
            workHead += 1
            f()
            if CACurrentMediaTime() - start > budget { break }
        }
        if workHead >= work.count { work.removeAll(keepingCapacity: true); workHead = 0 }
    }

    /// Runs every queued build step now (screenshots, Reduce Motion is not a reason: it still spreads work).
    func finishWork() { runWork(budget: .infinity) }

    // MARK: materials

    private func makeMaterials() {
        var g = UnlitMaterial(color: style.ghost)
        g.blending = .transparent(opacity: .init(floatLiteral: style.ghostOpacity))
        g.writesDepth = false
        g.faceCulling = .back
        ghostMat = g
        edgeMat = UnlitMaterial(color: style.edge)
        var f = UnlitMaterial(color: style.ghost)
        f.blending = .transparent(opacity: .init(floatLiteral: style.highContrast ? 0.16 : 0.1))
        f.writesDepth = false
        fadedMat = f
        matCache.removeAll()
        glowTexture = MeshKit.radialTexture(inner: style.lightWarm.withAlphaComponent(0.85), outer: style.lightWarm.withAlphaComponent(0))
    }

    private func pbr(_ key: String, _ c: UIColor, rough: Float = 0.5, metal: Float = 0, emissive: Float = 0, fog: Int = 0) -> RealityKit.Material {
        let k = "\(key)#\(fog)"
        if let m = matCache[k] { return m }
        let mix = CGFloat(DepthFog.mix(step: fog, maxMix: style.fogMax))
        let colour = mix > 0 ? c.mixed(with: style.fog, mix) : c
        var m = PhysicallyBasedMaterial()
        m.baseColor = .init(tint: colour)
        m.roughness = .init(floatLiteral: rough)
        m.metallic = .init(floatLiteral: metal)
        if emissive > 0 {
            m.emissiveColor = .init(color: colour)
            m.emissiveIntensity = emissive * Float(1 - mix)
        }
        matCache[k] = m
        return m
    }

    private func unlit(_ key: String, _ c: UIColor, fog: Int = 0) -> RealityKit.Material {
        let k = "\(key)#\(fog)"
        if let m = matCache[k] { return m }
        let mix = CGFloat(DepthFog.mix(step: fog, maxMix: style.fogMax))
        let m = UnlitMaterial(color: mix > 0 ? c.mixed(with: style.fog, mix) : c)
        matCache[k] = m
        return m
    }

    private func material(for e: BuildingElement, lit: Bool = false, fog: Int = 0) -> RealityKit.Material {
        let boost: Float = lit ? 0.9 : (style.highContrast ? 0.35 : 0.12)
        let f = lit ? 0 : fog
        switch e.role {
        case .equipment where e.kind == "Panelboard":
            // Panels read as small solid orange objects (the spec's floor view), not neutral gear.
            return pbr("panel-\(lit)", style.color(.electrical), rough: 0.45, emissive: lit ? 0.6 : 0.15, fog: f)
        case .equipment:
            return pbr("eq-\(e.system)-\(lit)", lit ? style.color(e.system) : style.equipment(e.system), rough: 0.55, emissive: lit ? 0.5 : 0.04, fog: f)
        case .main:
            return pbr("main-\(e.system)-\(lit)", style.color(e.system), rough: 0.42, emissive: boost, fog: f)
        case .branch:
            return pbr("br-\(e.system)-\(lit)", lit ? style.color(e.system) : style.branch(e.system), rough: 0.5, emissive: lit ? 0.9 : 0.08, fog: f)
        case .terminal:
            if e.kind == "Luminaire" { return unlit("lum", style.lightWarm, fog: f) }
            return pbr("tm-\(e.system)-\(lit)", style.color(e.system), rough: 0.45, emissive: lit ? 0.9 : boost, fog: f)
        case .fixture:
            return pbr("fx-\(lit)", lit ? style.color(.plumbing) : UIColor(hex: 0xF3F5F4).mixed(with: style.color(.plumbing), 0.08), rough: 0.25, emissive: lit ? 0.6 : 0, fog: f)
        case .casework:
            return pbr("cw-\(e.kind)", e.kind == "Vanity" ? UIColor(hex: 0xEDEBE6) : UIColor(hex: 0xB89670), rough: 0.7, fog: f)
        case .furniture:
            let c: UIColor
            switch e.kind {
            case "Bed": c = UIColor(hex: 0xF4F2EE)
            case "Headboard", "Desk", "Nightstand": c = UIColor(hex: 0x7C5E46)
            case "Chair": c = UIColor(hex: 0x3B4248)
            default: c = UIColor(hex: 0x9AA6A0)
            }
            return pbr("fu-\(e.kind)", c, rough: 0.85, fog: f)
        case .shell:
            return ghostMat
        }
    }

    /// The outline colour of an element with edges.
    private func edgeMaterial(for e: BuildingElement, fog: Int) -> RealityKit.Material {
        let base = e.role == .equipment && e.kind != "Panelboard" ? style.equipment(e.system) : style.color(e.system)
        return unlit("edge-\(e.role)-\(e.kind == "Panelboard")-\(e.system)", style.outline(base), fog: fog)
    }

    // MARK: static scene (shell, mains, exterior)

    private func buildStatic() {
        let n = b.levels.count
        levelRoots = (0..<n).map { _ in Entity() }
        ghost = (0..<n).map { _ in Entity() }
        cut = (0..<n).map { _ in Entity() }
        mains = (0..<n).map { _ in Entity() }
        detail = (0..<n).map { _ in Entity() }
        serving = (0..<n).map { _ in Entity() }
        tiles = (0..<n).map { _ in Entity() }
        detailBuilt = Array(repeating: false, count: n)
        lift = Array(repeating: 0, count: n); liftVel = lift; liftGoal = lift
        for i in 0..<n {
            let root = levelRoots[i]
            root.name = "level-\(i)"
            anchor.addChild(root)
            for c in [ghost[i], cut[i], tiles[i], mains[i], detail[i], serving[i]] { root.addChild(c) }
            cut[i].isEnabled = false
            tiles[i].isEnabled = false
        }
        for e in b.elements where e.detail == .building {
            let bb = e.shape.bounds
            if bb.max.y - bb.min.y > 5 { risers.insert(e.id) }
        }
        // What the opening needs right away: the solid exterior and the ground. The ghost shell and the mains
        // are behind the solid until the sweep starts, so they build over the next frames.
        buildGround()
        buildExterior()
        buildPulse()
        for i in 0..<n { enqueue { [weak self] in self?.buildShell(i) } }
        let mainsList = b.elements.filter { $0.detail == .building }
        for start in stride(from: 0, to: mainsList.count, by: 6) {
            let slice = Array(mainsList[start..<min(start + 6, mainsList.count)])
            enqueue { [weak self] in
                guard let self else { return }
                for e in slice { self.addElement(e, home: self.mains[e.level]) }
            }
        }
    }

    private func buildShell(_ i: Int) {
        guard let lv = b.level(i), ghost.indices.contains(i) else { return }
        let lo = b.footprintMin, hi = b.footprintMax
        var faces = MeshKit(), edges = MeshKit()
        let w: Float = style.highContrast ? 0.07 : 0.05
        if i > 0 {
            // The slab this level stands on (the roof's is the roof).
            let c = SIMD3<Float>((lo.x + hi.x) / 2, lv.elevation - 0.15, (lo.y + hi.y) / 2), s = SIMD3<Float>(hi.x - lo.x, 0.3, hi.y - lo.y)
            faces.box(center: c, size: s)
            edges.edges(center: c, size: s, width: w)
        }
        // Building scale: the envelope and slabs only. Interior walls appear in the floor's cut view.
        for (c, s) in b.walls(level: i).prefix(4) {
            faces.box(center: c, size: s)
            edges.edges(center: c, size: s, width: w)
        }
        if i == b.levels.count - 1 {
            // Parapet as a ghost too.
            for z in [lo.y + 0.15, hi.y - 0.15] {
                let c = SIMD3<Float>((lo.x + hi.x) / 2, lv.elevation + 0.45, z), s = SIMD3<Float>(hi.x - lo.x, 0.9, 0.3)
                faces.box(center: c, size: s); edges.edges(center: c, size: s, width: w)
            }
        }
        if let m = faces.resource("ghost-\(i)") { ghost[i].addChild(ModelEntity(mesh: m, materials: [ghostMat])) }
        if let m = edges.resource("edges-\(i)") { ghost[i].addChild(ModelEntity(mesh: m, materials: [edgeMat])) }
        // The cut view: walls at 4 ft, every wall gets its hairlines, a soft floor plate.
        guard lv.height > 0 || i == b.levels.count - 1 else { return }
        var cf = MeshKit(), ce = MeshKit(), plate = MeshKit()
        for (c, s) in b.walls(level: i, cut: 1.22) { cf.box(center: c, size: s); ce.edges(center: c, size: s, width: 0.035) }
        plate.box(center: SIMD3((lo.x + hi.x) / 2, lv.elevation - 0.13, (lo.y + hi.y) / 2), size: SIMD3(hi.x - lo.x, 0.26, hi.y - lo.y))
        if let m = cf.resource("cut-\(i)") {
            var cutMat = UnlitMaterial(color: style.ghost)
            cutMat.blending = .transparent(opacity: .init(floatLiteral: min(style.ghostOpacity * 2.2, 0.32)))
            cutMat.writesDepth = false
            cut[i].addChild(ModelEntity(mesh: m, materials: [cutMat]))
        }
        if let m = ce.resource("cutedge-\(i)") { cut[i].addChild(ModelEntity(mesh: m, materials: [edgeMat])) }
        if let m = plate.resource("plate-\(i)") { cut[i].addChild(ModelEntity(mesh: m, materials: [pbr("plate", style.floorPlate, rough: 0.95)])) }
        // Room tiles (tap targets on the floor view).
        for r in b.rooms(on: i) where r.name != "Corridor" {
            let inset: Float = 0.12
            let size = SIMD3<Float>(r.max.x - r.min.x - 2 * inset, 0.04, r.max.y - r.min.y - 2 * inset)
            let tile = ModelEntity(mesh: .generateBox(size: size, cornerRadius: 0.06), materials: [pbr("tile", style.roomTile, rough: 0.9)])
            tile.position = SIMD3(r.center.x, lv.elevation + 0.03, r.center.y)
            tile.components.set(RoomTagComponent(number: r.number))
            tile.components.set(CollisionComponent(shapes: [.generateBox(size: SIMD3(size.x, 0.3, size.z))]))
            tiles[i].addChild(tile)
            roomTileEntities[r.number] = tile
            if raise[r.number] == nil { raise[r.number] = (0, 0, 0) }
        }
    }

    /// One quad, one texture: a pool of light (dark looks) and the soft contact shadow of the footprint.
    private func buildGround() {
        let lo = b.footprintMin, hi = b.footprintMax
        let size = SIMD2<Float>(hi.x - lo.x + 60, hi.y - lo.y + 50)
        let share = CGSize(width: CGFloat((hi.x - lo.x + 3) / size.x), height: CGFloat((hi.y - lo.y + 3) / size.y))
        guard let q = MeshKit.quad(center: SIMD3((lo.x + hi.x) / 2, -0.02, (lo.y + hi.y) / 2), size: size),
              let t = MeshKit.groundTexture(glow: style.groundGlow, shadow: style.groundShadow, footprint: share) else { return }
        var m = UnlitMaterial()
        m.color = .init(tint: .white, texture: .init(t))
        m.blending = .transparent(opacity: .init(floatLiteral: 1))
        m.writesDepth = false
        ground = ModelEntity(mesh: q, materials: [m])
        anchor.addChild(ground)
    }

    /// The solid exterior for the opening reveal: one merged piece per bay, swept from solid to clear.
    private func buildExterior() {
        let lo = b.footprintMin, hi = b.footprintMax
        let bays = Int(((hi.x - lo.x) / b.bay).rounded())
        let wall = pbr("facade", style.facade, rough: 0.82)
        let base = pbr("facade-dark", style.facadeDark, rough: 0.9)
        let glass = pbr("glass", style.glass, rough: 0.12, metal: 0.4)
        let t: Float = 0.32
        let top = b.levels.count - 1
        for k in 0..<bays {
            let x0 = lo.x + Float(k) * b.bay, x1 = x0 + b.bay
            var solid = MeshKit(), dark = MeshKit(), pane = MeshKit()
            for i in 1..<max(top, 1) {
                guard let lv = b.level(i) else { continue }
                let e = lv.elevation, h = lv.height
                for (z, out) in [(lo.y, Float(-1)), (hi.y, Float(1))] {
                    let zc = z - out * t / 2
                    let storefront = i == 1 && out < 0 && x1 <= lo.x + (hi.x - lo.x) / 2
                    let sill: Float = storefront ? 0.35 : 0.9, head: Float = storefront ? h - 0.7 : 2.45
                    solid.box(min: SIMD3(x0, e + head, zc - t / 2), max: SIMD3(x1, e + h, zc + t / 2))
                    if i == 1 { dark.box(min: SIMD3(x0, e, zc - t / 2), max: SIMD3(x1, e + sill, zc + t / 2)) } else { solid.box(min: SIMD3(x0, e, zc - t / 2), max: SIMD3(x1, e + sill, zc + t / 2)) }
                    let pier: Float = storefront ? 0.18 : 0.55
                    solid.box(min: SIMD3(x0, e + sill, zc - t / 2), max: SIMD3(x0 + pier, e + head, zc + t / 2))
                    solid.box(min: SIMD3(x1 - pier, e + sill, zc - t / 2), max: SIMD3(x1, e + head, zc + t / 2))
                    pane.box(min: SIMD3(x0 + pier, e + sill, zc - 0.02 + out * 0.06), max: SIMD3(x1 - pier, e + head, zc + 0.02 + out * 0.06))
                    // A slim fin at each bay line gives the facade rhythm.
                    solid.box(min: SIMD3(x0 - 0.06, e, z + out * 0.02 - 0.1), max: SIMD3(x0 + 0.06, e + h, z + out * 0.2))
                }
            }
            // Roof slab and parapet.
            let roof = b.level(top)?.elevation ?? b.height
            solid.box(min: SIMD3(x0, roof - 0.3, lo.y), max: SIMD3(x1, roof, hi.y))
            solid.box(min: SIMD3(x0, roof, lo.y), max: SIMD3(x1, roof + 0.9, lo.y + 0.3))
            solid.box(min: SIMD3(x0, roof, hi.y - 0.3), max: SIMD3(x1, roof + 0.9, hi.y))
            if k == 0 || k == bays - 1 {
                let x = k == 0 ? lo.x + 0.16 : hi.x - 0.16
                solid.box(min: SIMD3(x - 0.16, 0, lo.y), max: SIMD3(x + 0.16, roof + 0.9, hi.y))
            }
            let bay = Entity()
            for (mk, m) in [(solid, wall), (dark, base), (pane, glass)] {
                if let r = mk.resource("bay-\(k)") { bay.addChild(ModelEntity(mesh: r, materials: [m])) }
            }
            anchor.addChild(bay)
            exterior.append((bay, (x0 + x1) / 2))
        }
    }

    private func buildPulse() {
        for i in 0..<5 {
            let s = ModelEntity(mesh: .generateSphere(radius: i == 0 ? 0.16 : 0.12 - Float(i) * 0.018), materials: [UnlitMaterial(color: .white)])
            s.isEnabled = false
            anchor.addChild(s)
            pulse.append(s)
        }
    }

    // MARK: layer groups

    /// The group entity for `e`'s layer inside `parent` (made on first use). Elements with no layer (the shell)
    /// go straight into the parent.
    private func container(_ parent: Entity, for e: BuildingElement) -> Entity {
        guard let layer = FieldLayer.of(e.system) else { return parent }
        let k = ObjectIdentifier(parent)
        if let g = groups[k]?[layer] { return g }
        let g = Entity()
        g.name = "layer-\(layer.rawValue)"
        parent.addChild(g)
        groups[k, default: [:]][layer] = g
        groupList.append(LayerGroup(entity: g, layer: layer))
        if !(exp?.layers.isOn(layer) ?? true) { g.isEnabled = false }
        return g
    }

    private func forgetGroups(under parent: Entity) {
        groups[ObjectIdentifier(parent)] = nil
        groupList.removeAll { $0.entity == nil || $0.entity?.parent === parent }
    }

    // MARK: elements

    /// Makes an element's entity inside `home`'s layer group. While a room is open, an element that serves it
    /// goes to its level's `serving` container instead (and remembers `home` to go back to).
    @discardableResult
    private func addElement(_ e: BuildingElement, home: Entity) -> Entity? {
        if let existing = elementEntities[e.id] { return existing }
        let mesh: MeshResource?
        var position = SIMD3<Float>.zero
        switch e.shape {
        case let .box(c, s):
            position = c
            if e.role == .equipment {
                mesh = .generateBox(size: s, cornerRadius: min(0.08, min(s.x, min(s.y, s.z)) * 0.12))
            } else if e.kind == "Luminaire" {
                mesh = .generateCylinder(height: s.y, radius: s.x / 2)
            } else if e.role == .furniture || e.role == .casework || e.role == .fixture {
                mesh = .generateBox(size: s, cornerRadius: min(0.04, min(s.x, min(s.y, s.z)) * 0.2))
            } else {
                mesh = .generateBox(size: s, cornerRadius: min(0.01, min(s.x, min(s.y, s.z)) * 0.2))
            }
        case let .run(points, profile):
            var k = MeshKit()
            k.run(points, profile: profile)
            mesh = k.resource(e.id)
        }
        guard let mesh else { return nil }
        let m = ModelEntity(mesh: mesh, materials: [material(for: e)])
        m.position = position
        m.name = e.id
        m.components.set(ElementTagComponent(id: e.id))
        let bb = e.shape.bounds
        let minTap: Float = e.detail == .room ? 0.14 : 0.45
        let size = simd_max(bb.max - bb.min, SIMD3(repeating: minTap))
        let center = (bb.min + bb.max) / 2 - position
        if case let .run(points, _) = e.shape {
            // Runs: a box per segment so a tap near the line hits it (not the run's whole bounding box).
            if points.count >= 2 {
                var shapes: [ShapeResource] = []
                for i in 1..<min(points.count, 16) {
                    let a = points[i - 1], c = points[i]
                    let s = simd_max(simd_abs(c - a), SIMD3(repeating: minTap))
                    shapes.append(.generateBox(size: s).offsetBy(translation: (a + c) / 2))
                }
                m.components.set(CollisionComponent(shapes: shapes))
            }
        } else {
            m.components.set(CollisionComponent(shapes: [.generateBox(size: size).offsetBy(translation: center)]))
        }
        // Outline edges on the solids that carry the building: mains with a rectangular profile and equipment.
        if e.detail != .room {
            var k = MeshKit()
            let w: Float = e.detail == .building ? 0.045 : 0.03
            switch e.shape {
            case let .box(_, s) where e.role == .equipment: k.edges(center: .zero, size: s, width: w)
            case let .run(points, profile) where e.role == .main: k.runEdges(points, profile: profile, width: w)
            default: break
            }
            if let em = k.resource("edge-\(e.id)") {
                let edge = ModelEntity(mesh: em, materials: [edgeMaterial(for: e, fog: 0)])
                m.addChild(edge)
                edgeEntities[e.id] = edge
            }
        }
        var parent = container(home, for: e)
        if roomServing.contains(e.id), case .room = exp?.scope, serving.indices.contains(e.level) {
            servingHome[e.id] = parent
            parent = container(serving[e.level], for: e)
        }
        parent.addChild(m)
        if e.kind == "Luminaire", let t = glowTexture, let q = MeshKit.quad(center: SIMD3(0, -0.04, 0), size: SIMD2(1.3, 1.3)) {
            var gm = UnlitMaterial()
            gm.color = .init(tint: .white, texture: .init(t))
            gm.blending = .transparent(opacity: .init(floatLiteral: 1))
            gm.writesDepth = false
            let glow = ModelEntity(mesh: q, materials: [gm])
            m.addChild(glow)
            // And a pool of light on the floor, very soft.
            if let h = e.mountingHeight, let pool = MeshKit.quad(center: SIMD3(0, -h + 0.05, 0), size: SIMD2(2.6, 2.6)) {
                var pm = UnlitMaterial()
                pm.color = .init(tint: UIColor.white.withAlphaComponent(0.55), texture: .init(t))
                pm.blending = .transparent(opacity: .init(floatLiteral: 0.5))
                pm.writesDepth = false
                m.addChild(ModelEntity(mesh: pool, materials: [pm]))
            }
        }
        elementEntities[e.id] = m
        fogStep[e.id] = 0
        // Made while following the system: take the follow look straight away.
        if let f = currentFocus { styleForFocus(e, m, f) }
        return m
    }

    /// The floor's branches, lights and terminals — built the first time the floor is opened, a small chunk per
    /// frame. Each chunk's layer groups fade in as a whole (a few fades, not one per element).
    private func ensureDetail(_ level: Int) {
        guard detail.indices.contains(level), !detailBuilt[level] else { return }
        detailBuilt[level] = true
        let items = b.elements.filter { $0.level == level && $0.detail == .floor }
        let chunk = 24
        for (n, start) in stride(from: 0, to: items.count, by: chunk).enumerated() {
            let slice = Array(items[start..<min(start + chunk, items.count)])
            enqueue { [weak self] in
                guard let self, self.detail.indices.contains(level) else { return }
                let holder = Entity()
                holder.name = "chunk-\(level)-\(n)"
                self.detail[level].addChild(holder)
                for e in slice { self.addElement(e, home: holder) }
                if !self.reduceMotion { self.fade(holder, from: 0, to: 1, delay: min(Double(n) * 0.02, 0.2)) }
            }
        }
    }

    private func buildRoom(_ number: String) {
        if roomDetailNumber == number { return }
        removeRoomDetail()
        guard let r = b.room(number), let lv = b.level(r.level) else { return }
        let root = Entity()
        root.name = "room-\(number)"
        levelRoots[r.level].addChild(root)
        // A dollhouse: floor, the far walls full height, the near walls (camera side) cut low.
        let e = lv.elevation, h = max(lv.height - 0.45, 2.4)
        let wallM = pbr("roomwall", style.isDark ? UIColor(hex: 0xE9E6DF) : UIColor(hex: 0xF1EFEA), rough: 0.92)
        let floorM = pbr("roomfloor", UIColor(hex: 0xC8B9A3), rough: 0.8)
        let bathM = pbr("bathfloor", UIColor(hex: 0xD9DCDC), rough: 0.5)
        var walls = MeshKit(), fl = MeshKit(), bath = MeshKit()
        fl.box(min: SIMD3(r.min.x, e - 0.02, r.min.y), max: SIMD3(r.max.x, e + 0.06, r.max.y))
        let south = r.max.y <= b.footprintMax.y / 2
        let camSide = orbitGoal.x   // yaw: which walls face the camera
        let camDir = SIMD2<Float>(sin(camSide), cos(camSide))
        let t: Float = 0.12
        let sides: [(SIMD2<Float>, SIMD3<Float>, SIMD3<Float>)] = [
            (SIMD2(0, -1), SIMD3(r.min.x, e, r.min.y), SIMD3(r.max.x, e, r.min.y + t)),
            (SIMD2(0, 1), SIMD3(r.min.x, e, r.max.y - t), SIMD3(r.max.x, e, r.max.y)),
            (SIMD2(-1, 0), SIMD3(r.min.x, e, r.min.y), SIMD3(r.min.x + t, e, r.max.y)),
            (SIMD2(1, 0), SIMD3(r.max.x - t, e, r.min.y), SIMD3(r.max.x, e, r.max.y)),
        ]
        for (n, a, c) in sides {
            let near = simd_dot(n, camDir) > 0.2
            walls.box(min: a, max: SIMD3(c.x, e + (near ? 0.9 : h), c.z))
        }
        if r.name == "Guest Room" {
            let bz = south ? r.max.y - 2.4 : r.min.y + 2.4
            let near2 = south ? camDir.y < 0 : camDir.y > 0
            walls.box(min: SIMD3(r.min.x, e, bz - 0.05), max: SIMD3(r.min.x + 2.4, e + (near2 ? 0.9 : h), bz + 0.05))
            walls.box(min: SIMD3(r.min.x + 2.35, e, min(bz, south ? r.max.y : r.min.y)), max: SIMD3(r.min.x + 2.45, e + 0.9, max(bz, south ? r.max.y : r.min.y)))
            bath.box(min: SIMD3(r.min.x + t, e + 0.06, min(bz, south ? r.max.y : r.min.y)), max: SIMD3(r.min.x + 2.35, e + 0.065, max(bz, south ? r.max.y : r.min.y)))
        }
        for (mk, m) in [(walls, wallM), (fl, floorM), (bath, bathM)] {
            if let res = mk.resource("room-\(number)") { root.addChild(ModelEntity(mesh: res, materials: [m])) }
        }
        let items = b.elements(in: number).filter { $0.detail == .room }
        for (k, el) in items.enumerated() {
            guard let ent = addElement(el, home: root) else { continue }
            if !reduceMotion { fade(ent, from: 0, to: 1, delay: 0.12 + Double(k) * 0.012) }
        }
        if !reduceMotion { fade(root, from: 0, to: 1, delay: 0) }
        roomDetail = root
        roomDetailNumber = number
    }

    private func removeRoomDetail() {
        guard let r = roomDetail else { return }
        for el in b.elements(in: roomDetailNumber ?? "") where el.detail == .room {
            elementEntities[el.id] = nil
            edgeEntities[el.id] = nil
            fogStep[el.id] = nil
        }
        forgetGroups(under: r)
        r.removeFromParent()
        roomDetail = nil
        roomDetailNumber = nil
    }

    private func dropRoom() {
        restoreServing()
        removeRoomDetail()
    }

    /// Moves the elements that serve the open room out of the floor, so the floor can fade back as a whole.
    private func moveServing(level: Int, ids: Set<String>) {
        restoreServing()
        guard serving.indices.contains(level) else { return }
        for id in ids {
            guard let ent = elementEntities[id], let e = b.element(id), e.level == level, let home = ent.parent else { continue }
            servingHome[id] = home
            ent.setParent(container(serving[level], for: e), preservingWorldTransform: true)
        }
    }

    private func restoreServing() {
        for (id, home) in servingHome { elementEntities[id]?.setParent(home, preservingWorldTransform: true) }
        servingHome.removeAll()
    }

    // MARK: scope changes

    var reduceMotion: Bool { UIAccessibility.isReduceMotionEnabled }

    func apply(scope: DesignScope, liftFocus: Float?) {
        guard exp != nil else { return }
        let n = b.levels.count
        userMoved = false
        switch scope {
        case .building:
            dropRoom()
            roomServing = []
            let offs = FloorLift.offsets(levels: n, focus: liftFocus, gap: 10, spread: 1.4)
            for i in 0..<n {
                liftGoal[i] = offs[i]
                setVisible(ghost[i], true)
                setVisible(mains[i], true)
                setVisible(cut[i], false)
                setVisible(tiles[i], false)
                setVisible(detail[i], false)
                setVisible(levelRoots[i], true)
            }
            for id in risers { if let e = elementEntities[id] { setOpacity(e, liftFocus == nil ? 1 : 0.18) } }
            orbitGoal.y = liftFocus == nil ? 0.56 : 0.38
            fit(buildingPoints(lift: offs), yaw: orbitGoal.x, pitch: orbitGoal.y)
            for (number, _) in raise { raise[number]?.goal = 0 }
        case let .floor(i):
            dropRoom()
            roomServing = []
            showFloor(i)
            orbitGoal.y = 0.74
            fit(floorPoints(i), yaw: nearestAngle(designYaw, to: orbitGoal.x), pitch: orbitGoal.y, turn: 0.6)
            for (number, _) in raise { raise[number]?.goal = 0 }
        case let .room(number):
            guard let r = b.room(number) else { break }
            let south = r.max.y <= b.footprintMax.y / 2
            // Look in from the outside of the room's long open side.
            let yaw: Float = south ? -2.55 : -0.62
            orbitGoal.x = nearestAngle(yaw, to: orbit.x)
            orbitGoal.y = 0.92
            // What serves this room stays; the rest of the floor steps back as a whole.
            var serves = Set<String>()
            for el in b.elements(in: number) { if let u = el.upstream { serves.insert(u) } }
            if roomDetailNumber != number { removeRoomDetail() }
            roomServing = serves
            showFloor(r.level)
            setVisible(mains[r.level], false)
            setVisible(detail[r.level], false)
            moveServing(level: r.level, ids: serves)
            buildRoom(number)
            fit(roomPoints(r), yaw: orbitGoal.x, pitch: orbitGoal.y)
            for (k, _) in raise { raise[k]?.goal = k == number ? 0.18 : 0 }
            for (k, t) in roomTileEntities where b.room(k)?.level == r.level { setOpacity(t, k == number ? 1 : 0.35) }
        }
        if reduceMotion { snapCamera() }
        applyLayers()
        refreshLabels(scope)
        fogDirty = true
    }

    private func showFloor(_ i: Int) {
        let n = b.levels.count
        ensureDetail(i)
        for j in 0..<n {
            if j > i {
                liftGoal[j] = 16 + Float(j - i) * 2.5
                setVisible(levelRoots[j], false)
            } else if j < i {
                liftGoal[j] = 0
                setVisible(levelRoots[j], true)
                setVisible(ghost[j], true)
                setVisible(mains[j], false)
                setVisible(detail[j], false)
                setVisible(cut[j], false)
                setVisible(tiles[j], false)
            } else {
                liftGoal[j] = 0
                setVisible(levelRoots[j], true)
                setVisible(ghost[j], false)
                setVisible(mains[j], true)
                setVisible(detail[j], true)
                setVisible(cut[j], true)
                setVisible(tiles[j], true)
                for (k, t) in roomTileEntities where b.room(k)?.level == j { setOpacity(t, 1) }
            }
        }
        for id in risers { if let e = elementEntities[id] { setOpacity(e, 1) } }
    }

    /// The tapped room tile rises on the spring before the camera goes in.
    func raiseRoom(_ number: String) {
        raise[number]?.goal = 0.32
        if let t = roomTileEntities[number] { t.model?.materials = [pbr("tile-up", style.roomTileRaised, rough: 0.8, emissive: 0.15)] }
    }

    private func nearestAngle(_ a: Float, to ref: Float) -> Float {
        var x = a
        while x - ref > .pi { x -= 2 * .pi }
        while ref - x > .pi { x += 2 * .pi }
        return x
    }

    // MARK: framing

    /// Each level's box (lifted) and its building-scale systems, as corner points.
    private func buildingPoints(lift offs: [Float]) -> [SIMD3<Float>] {
        let lo = b.footprintMin, hi = b.footprintMax
        var pts: [SIMD3<Float>] = []
        for lv in b.levels {
            let up = offs.indices.contains(lv.index) ? offs[lv.index] : 0
            var mn = SIMD3<Float>(lo.x, lv.elevation - 0.3, lo.y), mx = SIMD3<Float>(hi.x, lv.elevation + max(lv.height, 1.0), hi.y)
            for e in b.elements where e.level == lv.index && e.detail == .building && !risers.contains(e.id) {
                let bb = e.shape.bounds
                mn = simd_min(mn, bb.min); mx = simd_max(mx, bb.max)
            }
            pts += SceneFraming.corners(min: mn + SIMD3(0, up, 0), max: mx + SIMD3(0, up, 0))
        }
        return pts
    }

    /// The floor's plate, its cut walls and what sits on it (not the risers that run through it).
    private func floorPoints(_ i: Int) -> [SIMD3<Float>] {
        let lo = b.footprintMin, hi = b.footprintMax
        let e = b.level(i)?.elevation ?? 0
        var mn = SIMD3<Float>(lo.x, e - 0.3, lo.y), mx = SIMD3<Float>(hi.x, e + 1.3, hi.y)
        var any = false
        for el in b.elements where el.level == i && el.detail != .room && !risers.contains(el.id) {
            let bb = el.shape.bounds
            if i == 0 && !any { mn = bb.min; mx = bb.max; any = true; continue }
            mn = simd_min(mn, bb.min); mx = simd_max(mx, bb.max)
        }
        return SceneFraming.corners(min: mn, max: mx)
    }

    private func roomPoints(_ r: BuildingRoom) -> [SIMD3<Float>] {
        let lv = b.level(r.level)
        let e = lv?.elevation ?? 0, h = max((lv?.height ?? 3) - 0.45, 2.4)
        return SceneFraming.corners(min: SIMD3(r.min.x, e, r.min.y), max: SIMD3(r.max.x, e + h, r.max.y))
    }

    private var viewport: CGSize? {
        guard let v = view, v.bounds.width > 10, v.bounds.height > 10 else { return nil }
        return v.bounds.size
    }

    private var freeArea: CGRect {
        if let c = exp?.chrome, c.free.width > 10, c.free.height > 10 { return c.free }
        let s = viewport ?? CGSize(width: 400, height: 860)
        return CGRect(x: 16, y: 150, width: s.width - 90, height: s.height - 330)
    }

    /// Points the camera so `pts` fill the free area, centred. `turn` lets it turn up to that many radians from
    /// `yaw` when another angle shows the subject clearly larger.
    private func fit(_ pts: [SIMD3<Float>], yaw: Float, pitch: Float, turn: Float = 0) {
        framePoints = pts
        frameTurn = turn
        guard let vp = viewport, !pts.isEmpty else { return }
        let pose = SceneFraming.fitTurning(pts, yaw: yaw, spread: turn, pitch: pitch, fovYDegrees: Self.fov, viewport: vp, free: freeArea, margin: 14)
        targetGoal = pose.target
        orbitGoal.x = pose.yaw
        orbitGoal.z = pose.distance
        lastFree = freeArea
    }
    private var frameTurn: Float = 0
    /// The design angle each scope starts from (turning from the last fitted angle would drift).
    private var designYaw: Float = -2.25

    /// The chrome moved (a sheet grew, the text size changed): frame the same subject again, unless the user has
    /// moved the camera since.
    func chromeChanged() {
        guard !userMoved, !framePoints.isEmpty, freeArea != lastFree else { return }
        fit(framePoints, yaw: frameTurn > 0 ? nearestAngle(designYaw, to: orbitGoal.x) : orbitGoal.x, pitch: orbitGoal.y, turn: frameTurn)
        if reduceMotion { snapCamera() }
    }

    // MARK: layers

    private func layersShow(_ e: BuildingElement) -> Bool { exp?.layers.shows(e.system) ?? true }

    /// Fades each layer group, a handful of entities however big the building is.
    func applyLayers() {
        guard let exp else { return }
        groupList.removeAll { $0.entity == nil }
        for g in groupList { if let e = g.entity { setOpacity(e, exp.layers.isOn(g.layer) ? 1 : 0) } }
    }

    // MARK: follow the system

    private func styleForFocus(_ e: BuildingElement, _ ent: Entity, _ focus: SystemFocus) {
        guard let m = ent as? ModelEntity else { return }
        let key = ObjectIdentifier(m)
        // Furniture and casework are context in a room, not part of any system: they stay as they are.
        if !focus.lit.contains(e.id), e.role == .furniture || e.role == .casework { return }
        originals[key] = m.model?.materials
        entityByObject[key] = m
        m.model?.materials = focus.lit.contains(e.id) ? [material(for: e, lit: true)] : [fadedMat]
        if focus.lit.contains(e.id), focus.lit.count < 400, e.role != .equipment || e.id == focus.tapped { addHalo(e, to: m) }
        if let edge = edgeEntities[e.id], !focus.lit.contains(e.id) { edge.isEnabled = false; hiddenGlows.append(edge) }
        for case let child as ModelEntity in m.children where e.kind == "Luminaire" && !focus.lit.contains(e.id) {
            child.isEnabled = false
            hiddenGlows.append(child)
        }
    }

    func apply(focus: SystemFocus?) {
        // Restore everything first.
        for (k, mats) in originals {
            if let ent = entityByObject[k] as? ModelEntity { ent.model?.materials = mats }
        }
        originals.removeAll()
        entityByObject.removeAll()
        halos.forEach { $0.removeFromParent() }
        halos.removeAll()
        litIDs = []
        pulsePath = []
        pulse.forEach { $0.isEnabled = false }
        currentFocus = focus
        guard let focus else { fogDirty = true; return }
        litIDs = focus.lit
        for (id, ent) in elementEntities {
            guard let e = b.element(id) else { continue }
            styleForFocus(e, ent, focus)
        }
        // The route the pulse runs: from the tapped element up to the source.
        var path: [(SIMD3<Float>, Int)] = []
        for s in focus.steps {
            guard let e = b.element(s.id) else { continue }
            switch e.shape {
            case let .box(c, _): path.append((c, e.level))
            case let .run(points, _): path += points.reversed().map { ($0, e.level) }
            }
        }
        if focus.showsDownstream, focus.steps.count <= 1, let e = b.element(focus.tapped) { path = [(e.shape.anchor, e.level)] }
        pulsePath = path
        pulseT = 0
        let c = b.element(focus.tapped).map { style.color($0.system).mixed(with: .white, 0.45) } ?? .white
        for (i, p) in pulse.enumerated() {
            var m = UnlitMaterial(color: c)
            if i > 0 { m.blending = .transparent(opacity: .init(floatLiteral: 0.8 - Float(i) * 0.15)) }
            p.model?.materials = [m]
        }
    }

    private var entityByObject: [ObjectIdentifier: Entity] = [:]
    private var halos: [Entity] = []

    /// A soft sheath around a lit element, in its system colour. Same colour and opacity everywhere, no depth
    /// writes: like the ghost, it blends the same in any order.
    private func addHalo(_ e: BuildingElement, to ent: Entity) {
        var k = MeshKit()
        switch e.shape {
        case let .box(_, size): k.box(center: .zero, size: size + SIMD3(repeating: 0.12))
        case let .run(points, profile):
            switch profile {
            case let .round(rr): k.run(points, profile: .round(radius: rr * 2.2 + 0.05))
            case let .rect(w, h): k.run(points, profile: .rect(width: w + 0.14, height: h + 0.14))
            }
        }
        guard let mesh = k.resource("halo-\(e.id)") else { return }
        var m = UnlitMaterial(color: style.color(e.system))
        m.blending = .transparent(opacity: .init(floatLiteral: style.highContrast ? 0.4 : 0.28))
        m.writesDepth = false
        let h = ModelEntity(mesh: mesh, materials: [m])
        ent.addChild(h)
        halos.append(h)
    }
    private var hiddenGlows: [Entity] = []

    func clearFocusVisuals() {
        hiddenGlows.forEach { $0.isEnabled = true }
        hiddenGlows.removeAll()
        apply(focus: nil)
    }

    /// Flies the one camera to an element, opening its floor if it is on another one: the element and enough
    /// around it to read what it connects to, framed in the free area.
    func frame(element id: String) {
        guard let e = b.element(id) else { return }
        let up = liftGoal.indices.contains(e.level) ? liftGoal[e.level] : 0
        let bb = e.shape.bounds
        let reach: Float = e.detail == .room ? 1.6 : 5
        let c = (bb.min + bb.max) / 2
        let half = simd_max((bb.max - bb.min) / 2, SIMD3(repeating: reach))
        orbitGoal.y = 0.62
        fit(SceneFraming.corners(min: c - half + SIMD3(0, up, 0), max: c + half + SIMD3(0, up, 0)), yaw: orbitGoal.x, pitch: orbitGoal.y)
        if reduceMotion { snapCamera() }
    }

    // MARK: reveal

    func setReveal(_ p: Float) {
        let lo = b.footprintMin.x, hi = b.footprintMax.x
        for bay in exterior {
            let o = RevealSweep.solidOpacity(x: bay.x, minX: lo, maxX: hi, progress: p, band: b.bay * 1.6)
            if o <= 0.001 {
                bay.entity.isEnabled = false
            } else {
                bay.entity.isEnabled = true
                if o >= 0.999 { bay.entity.components.remove(OpacityComponent.self) } else { bay.entity.components.set(OpacityComponent(opacity: o)) }
            }
        }
        if let v = view, p > 0, p < 1 {
            let front = lo + (hi - lo) * p
            let top = v.project(SIMD3(front, b.height, b.footprintMin.y)), bottom = v.project(SIMD3(front, 0, b.footprintMin.y))
            exp?.revealLine = top.flatMap { t in bottom.map { (t, $0) } }
        } else {
            exp?.revealLine = nil
        }
    }

    // MARK: style

    func restyle(_ s: DesignStyle) {
        guard s != style else { return }
        style = s
        // Rebuild with the new palette; the camera and scope carry over.
        let keepFocus = exp?.focus
        apply(focus: nil)
        restoreServing()
        for r in levelRoots { r.removeFromParent() }
        for e in exterior { e.entity.removeFromParent() }
        exterior = []
        ground.removeFromParent()
        pulse.forEach { $0.removeFromParent() }
        pulse = []
        work.removeAll(); workHead = 0
        elementEntities.removeAll(); edgeEntities.removeAll(); roomTileEntities.removeAll(); raise.removeAll(); fades.removeAll()
        groups.removeAll(); groupList.removeAll(); fogStep.removeAll(); risers.removeAll()
        roomDetail = nil; roomDetailNumber = nil
        makeMaterials()
        buildStatic()
        setReveal(exp?.revealProgress ?? 1)
        if let exp { apply(scope: exp.scope, liftFocus: exp.liftFocus); apply(focus: keepFocus) }
        for l in labels.values { l.removeFromSuperview() }
        labels.removeAll()
        if let exp { refreshLabels(exp.scope) }
    }

    // MARK: fades and visibility

    private func setVisible(_ e: Entity, _ on: Bool) { setOpacity(e, on ? 1 : 0) }

    private func setOpacity(_ e: Entity, _ goal: Float) {
        let k = ObjectIdentifier(e)
        let current: Float = e.isEnabled ? (e.components[OpacityComponent.self]?.opacity ?? 1) : 0
        if reduceMotion {
            fades[k] = nil
            applyOpacity(e, goal)
            return
        }
        if var f = fades[k] { f.goal = goal; fades[k] = f; if goal > 0 { e.isEnabled = true }; return }
        if abs(current - goal) < 0.001 { if goal > 0 { e.isEnabled = true }; return }
        if goal > 0 && !e.isEnabled { e.isEnabled = true; e.components.set(OpacityComponent(opacity: 0)) }
        fades[k] = Fade(e: e, value: e.isEnabled ? current : 0, vel: 0, goal: goal, delay: 0)
    }

    private func fade(_ e: Entity, from: Float, to: Float, delay: Double) {
        e.isEnabled = true
        e.components.set(OpacityComponent(opacity: from))
        fades[ObjectIdentifier(e)] = Fade(e: e, value: from, vel: 0, goal: to, delay: delay)
    }

    private func applyOpacity(_ e: Entity, _ o: Float) {
        if o <= 0.002 { e.isEnabled = false; e.components.remove(OpacityComponent.self); return }
        e.isEnabled = true
        if o >= 0.998 { e.components.remove(OpacityComponent.self) } else { e.components.set(OpacityComponent(opacity: o)) }
    }

    // MARK: per frame

    private func tick(_ dt: TimeInterval) {
        let frameStart = CACurrentMediaTime()
        let dt = min(max(dt, 0), 0.1)
        frameCount += 1
        if autoOrbitSpeed != 0 {
            orbitGoal.x += autoOrbitSpeed * Float(dt)
            // Keep the whole subject in the free area as it turns.
            if !framePoints.isEmpty, let vp = viewport {
                let pose = SceneFraming.fit(framePoints, yaw: orbitGoal.x, pitch: orbitGoal.y, fovYDegrees: Self.fov, viewport: vp, free: freeArea, margin: 14)
                targetGoal = pose.target; orbitGoal.z = pose.distance
            }
        }
        spring.step(&target, &targetVel, toward: targetGoal, dt: dt)
        spring.step(&orbit, &orbitVel, toward: orbitGoal, dt: dt)
        for i in lift.indices {
            spring.step(&lift[i], &liftVel[i], toward: liftGoal[i], dt: dt)
            levelRoots[i].position.y = lift[i]
        }
        for (k, var r) in raise {
            guard abs(r.y - r.goal) > 0.0005 || abs(r.v) > 0.0005 else { continue }
            spring.step(&r.y, &r.v, toward: r.goal, dt: dt)
            raise[k] = r
            if let t = roomTileEntities[k], let room = b.room(k), let lv = b.level(room.level) {
                t.position.y = lv.elevation + 0.03 + r.y
            }
        }
        var done: [ObjectIdentifier] = []
        for (k, var f) in fades {
            guard let e = f.e else { done.append(k); continue }
            if f.delay > 0 { f.delay -= dt; fades[k] = f; continue }
            // Opacity fades a touch quicker than the camera, never past ~0.5 s.
            Spring(response: 0.38, dampingFraction: 1).step(&f.value, &f.vel, toward: f.goal, dt: dt)
            if abs(f.value - f.goal) < 0.01 { f.value = f.goal; done.append(k) }
            applyOpacity(e, min(max(f.value, 0), 1))
            fades[k] = f
        }
        for k in done { fades[k] = nil }
        placeCamera()
        tickPulse(dt)
        positionLabels()
        if frameCount % 3 == 0 { updateFog() }
        exp?.tick(dt)
        // Whatever is left of this frame's few milliseconds goes to building.
        runWork(budget: max(Self.workBudget - (CACurrentMediaTime() - frameStart), 0.001))
        measure(dt)
    }

    private func placeCamera() {
        let p = target + orbit.z * SIMD3(cos(orbit.y) * sin(orbit.x), sin(orbit.y), cos(orbit.y) * cos(orbit.x))
        camera.look(at: target, from: p, relativeTo: nil)
    }

    func snapCamera() {
        target = targetGoal; targetVel = .zero
        orbit = orbitGoal; orbitVel = .zero
        placeCamera()
    }

    /// Distant systems fade toward the backdrop: a fog step per element from its distance, between the nearest
    /// and farthest corner of what the camera frames. Materials only change when an element crosses a step.
    private func updateFog() {
        guard currentFocus == nil, !framePoints.isEmpty else { return }
        let eye = camera.position(relativeTo: nil)
        guard fogDirty || simd_distance(eye, fogEye) > 0.25 else { return }
        fogDirty = false
        fogEye = eye
        var near = Float.greatestFiniteMagnitude, far: Float = 0
        for p in framePoints { let d = simd_distance(p, eye); near = min(near, d); far = max(far, d) }
        let scale: Float
        switch exp?.scope ?? .building {
        case .building: scale = 1
        case .floor: scale = 0.85
        case .room: scale = 0.3
        }
        for (id, ent) in elementEntities {
            guard let e = b.element(id), let m = ent as? ModelEntity, e.role != .shell else { continue }
            let up = lift.indices.contains(e.level) ? lift[e.level] : 0
            let d = simd_distance(e.shape.anchor + SIMD3(0, up, 0), eye)
            let step = Int((Float(DepthFog.step(distance: d, near: near, far: far)) * scale).rounded())
            guard fogStep[id] != step else { continue }
            fogStep[id] = step
            m.model?.materials = [material(for: e, fog: step)]
            edgeEntities[id]?.model?.materials = [edgeMaterial(for: e, fog: step)]
        }
    }

    private func tickPulse(_ dt: TimeInterval) {
        guard pulsePath.count >= 2 else { pulse.forEach { $0.isEnabled = false }; return }
        pulseT += dt
        // Path length with each point lifted with its level.
        let pts = pulsePath.map { $0.0 + SIMD3(0, lift.indices.contains($0.1) ? lift[$0.1] : 0, 0) }
        var lens: [Float] = [0]
        for i in 1..<pts.count { lens.append(lens[i - 1] + simd_length(pts[i] - pts[i - 1])) }
        let total = max(lens.last ?? 1, 0.01)
        let travel = Double(min(max(total / 30, 1.0), 2.2))   // seconds along the route
        let cycle = travel + 0.8
        let phase = pulseT.truncatingRemainder(dividingBy: cycle)
        for (k, s) in pulse.enumerated() {
            let t = Float((phase - Double(k) * 0.05) / travel)
            guard t >= 0, t <= 1 else { s.isEnabled = false; continue }
            let d = t * t * (3 - 2 * t) * total
            var i = 1
            while i < lens.count - 1 && lens[i] < d { i += 1 }
            let seg = max(lens[i] - lens[i - 1], 0.0001)
            let u = min(max((d - lens[i - 1]) / seg, 0), 1)
            s.position = simd_mix(pts[i - 1], pts[i], SIMD3(repeating: u))
            s.isEnabled = true
        }
    }

    private func measure(_ dt: Double) {
        frames += 1
        worstFrame = max(worstFrame, dt)
        let now = Date()
        let span = now.timeIntervalSince(fpsStart)
        guard span >= 1 else { return }
        let fps = Double(frames) / span
        exp?.fps = fps
        #if DEBUG
        if logPerf {
            fpsLog.append(fps)
            let ents = elementEntities.count
            print(String(format: "FIELD-PERF design fps %.1f worst %.0f ms scope %@ elements %d queued %d", fps, worstFrame * 1000, String(describing: exp?.scope ?? .building), ents, work.count - workHead))
        }
        #endif
        frames = 0
        worstFrame = 0
        fpsStart = now
    }

    // MARK: labels

    /// Label priority (higher keeps its place): the selected element, then equipment by size, then rooms by
    /// area, then fixtures, then terminals.
    static func labelPriority(_ e: BuildingElement) -> Int {
        let bb = e.shape.bounds
        let v = (bb.max - bb.min)
        let size = min(Int(v.x * v.y * v.z * 10), 999)
        switch e.role {
        case .equipment: return 3000 + size
        case .fixture: return 1500 + size
        default: return 1000 + size
        }
    }

    func refreshLabels(_ scope: DesignScope) {
        var want: [String: (String, SIMD3<Float>, Int, Bool, Int)] = [:]   // key → text, point, level, mono, priority
        switch scope {
        case .building:
            for e in b.elements where e.role == .equipment && e.detail == .building && e.kind != "Panelboard" && layersShow(e) {
                want["e-\(e.id)"] = (e.tag, e.shape.bounds.max * SIMD3(0, 1, 0) + e.shape.anchor * SIMD3(1, 0, 1) + SIMD3(0, 0.6, 0), e.level, true, Self.labelPriority(e))
            }
        case let .floor(i):
            for r in b.rooms(on: i) where r.name != "Corridor" {
                let text = r.name == "Guest Room" ? r.number : r.name
                let area = Int((r.max.x - r.min.x) * (r.max.y - r.min.y))
                want["r-\(r.number)"] = (text, SIMD3(r.center.x, (b.level(i)?.elevation ?? 0) + 0.1, r.center.y), i, r.name == "Guest Room", 2000 + min(area, 999))
            }
            for e in b.elements where e.level == i && e.role == .equipment && layersShow(e) {
                want["e-\(e.id)"] = (e.tag, e.shape.anchor + SIMD3(0, 1.1, 0), e.level, true, Self.labelPriority(e))
            }
        case let .room(n):
            for e in b.elements(in: n) where (e.role == .terminal || e.role == .fixture) && layersShow(e) {
                want["d-\(e.id)"] = (e.tag, e.shape.bounds.max * SIMD3(0, 1, 0) + e.shape.anchor * SIMD3(1, 0, 1) + SIMD3(0, 0.12, 0), e.level, true, Self.labelPriority(e))
            }
        }
        // Labels leaving go at once: a fading-out label could sit under a new one.
        for (k, l) in labels where want[k] == nil {
            l.removeFromSuperview()
            labels[k] = nil
            labelAnchors[k] = nil
            labelPriority[k] = nil
            labelSlots[k] = nil
        }
        for (k, (text, p, lv, mono, prio)) in want {
            labelAnchors[k] = (p, lv)
            labelPriority[k] = prio
            guard labels[k] == nil else { continue }
            let l = PaddedLabel()
            l.text = text
            l.font = mono ? .monospacedSystemFont(ofSize: k.hasPrefix("d-") ? 11 : 12, weight: .semibold) : .systemFont(ofSize: 12, weight: .semibold)
            l.textColor = style.isDark ? .white : UIColor(hex: 0x0E1A24)
            l.backgroundColor = (style.isDark ? UIColor(hex: 0x0C1A19) : .white).withAlphaComponent(style.highContrast ? 0.92 : 0.7)
            l.layer.cornerRadius = 7
            l.layer.borderWidth = style.highContrast ? 1 : 0.5
            l.layer.borderColor = (style.isDark ? UIColor.white : UIColor.black).withAlphaComponent(style.highContrast ? 0.6 : 0.12).cgColor
            l.clipsToBounds = true
            l.sizeToFit()
            l.alpha = 0
            l.isHidden = true
            labelLayer.addSubview(l)
            labels[k] = l
        }
        leaderLayer.strokeColor = (style.isDark ? UIColor.white : UIColor(hex: 0x0E1A24)).withAlphaComponent(style.highContrast ? 0.8 : 0.5).cgColor
    }

    /// Screen-space layout every frame: on screen, off the chrome, no two overlapping; moved aside with a short
    /// leader, or dropped lowest priority first (LabelLayout).
    private func positionLabels() {
        guard let v = view, !labels.isEmpty else { leaderLayer.path = nil; return }
        let selectedKey = exp?.selected.map { "d-\($0)" }
        var cands: [LabelCandidate] = []
        cands.reserveCapacity(labels.count)
        for (k, l) in labels {
            guard let (p, lv) = labelAnchors[k] else { continue }
            let up = lift.indices.contains(lv) ? lift[lv] : 0
            guard let s = v.project(p + SIMD3(0, up, 0)) else { continue }
            let prio = (labelPriority[k] ?? 0) + (k == selectedKey ? 10_000 : 0)
            cands.append(LabelCandidate(id: k, anchor: s, size: l.bounds.size, priority: prio))
        }
        var avoid: [CGRect] = []
        if let c = exp?.chrome {
            // The header plus the scrim's fade below it: a label there would sit half washed out.
            var head = c.header
            head.size.height += 20
            avoid = [head, c.close, c.layers, c.bottom]
            if let r = c.rail, exp?.currentRoom == nil { avoid.append(r) }
            if let extra = exp?.overlay { avoid.append(extra) }
        }
        let placed = LabelLayout.place(cands, bounds: v.bounds, avoid: avoid, previous: labelSlots)
        var shown = Set<String>()
        let path = UIBezierPath()
        for p in placed {
            guard let l = labels[p.id] else { continue }
            shown.insert(p.id)
            labelSlots[p.id] = p.slot
            l.frame = p.frame
            if l.isHidden { l.isHidden = false; l.alpha = 0 }
            l.alpha = min(l.alpha + (reduceMotion ? 1 : 0.16), 1)
            if p.hasLeader {
                let a = p.anchor
                let q = CGPoint(x: min(max(a.x, p.frame.minX), p.frame.maxX), y: min(max(a.y, p.frame.minY), p.frame.maxY))
                path.move(to: a); path.addLine(to: q)
                path.append(UIBezierPath(arcCenter: a, radius: 1.8, startAngle: 0, endAngle: 2 * .pi, clockwise: true))
            }
        }
        for (k, l) in labels where !shown.contains(k) {
            l.isHidden = true
            l.alpha = 0
            labelSlots[k] = nil
        }
        leaderLayer.path = path.cgPath
    }

    // MARK: gestures

    @objc private func onPan(_ g: UIPanGestureRecognizer) {
        guard let v = view, let exp else { return }
        let t = g.translation(in: v)
        if !exp.revealDone {
            if g.state == .began { exp.beginRevealDrag() }
            exp.dragReveal(Float(t.x / max(v.bounds.width, 1)) * 1.25)
            if g.state == .ended || g.state == .cancelled { exp.endRevealDrag() }
            return
        }
        if g.state == .began { panStart = orbitGoal; userMoved = true }
        orbitGoal.x = panStart.x - Float(t.x) * 0.006
        orbitGoal.y = min(max(panStart.y + Float(t.y) * 0.005, 0.08), 1.35)
        // Direct manipulation: no lag under the finger.
        orbit.x = orbitGoal.x; orbit.y = orbitGoal.y; orbitVel.x = 0; orbitVel.y = 0
        if g.state == .ended {
            let vel = g.velocity(in: v)
            if vel.y > 1500 && abs(vel.x) < vel.y * 0.5 {
                orbitGoal.y = panStart.y
                exp.goUp()
            }
        }
    }

    @objc private func onPinch(_ g: UIPinchGestureRecognizer) {
        if g.state == .began { pinchStart = orbitGoal.z; userMoved = true }
        orbitGoal.z = min(max(pinchStart / Float(max(g.scale, 0.05)), 4), 400)
        orbit.z = orbitGoal.z; orbitVel.z = 0
    }

    @objc private func onTap(_ g: UITapGestureRecognizer) {
        guard let v = view, let exp else { return }
        if !exp.revealDone { exp.finishReveal(); return }
        let hits = v.hitTest(g.location(in: v), query: .all, mask: .all)
        var element: String?, room: String?, level: Int?
        for h in hits {
            var e: Entity? = h.entity
            while let x = e {
                if element == nil, let t = x.components[ElementTagComponent.self] { element = t.id; break }
                if room == nil, let r = x.components[RoomTagComponent.self] { room = r.number; break }
                if level == nil, let l = x.components[LevelTagComponent.self] { level = l.index; break }
                e = x.parent
            }
        }
        switch exp.scope {
        case .building:
            if let element { exp.tapElement(element) } else if let level = levelUnder(g.location(in: v)) { exp.go(.floor(level)) } else { exp.clearFocus() }
        case .floor:
            if let element, let e = b.element(element), e.role == .equipment || e.role == .main || room == nil { exp.tapElement(element) } else if let room { exp.tapRoom(room) } else { exp.clearFocus() }
        case .room:
            if let element { exp.tapElement(element) } else if let room, room != exp.currentRoom { exp.tapRoom(room) } else { exp.clearFocus() }
        }
    }

    /// The level whose (lifted) slab band is under a screen point at building scope.
    private func levelUnder(_ p: CGPoint) -> Int? {
        guard let v = view else { return nil }
        var best: (Int, CGFloat)?
        let lo = b.footprintMin, hi = b.footprintMax
        for lv in b.levels where lv.height > 0 {
            let y = lv.elevation + lv.height / 2 + (lift.indices.contains(lv.index) ? lift[lv.index] : 0)
            for x in stride(from: lo.x, through: hi.x, by: max((hi.x - lo.x) / 14, 0.5)) {
                for z in [lo.y, (lo.y + hi.y) / 2, hi.y] {
                    guard let s = v.project(SIMD3(x, y, z)) else { continue }
                    let d = hypot(s.x - p.x, s.y - p.y)
                    if d < (best?.1 ?? 70) { best = (lv.index, d) }
                }
            }
        }
        return best?.0
    }
}

final class PaddedLabel: UILabel {
    override func drawText(in rect: CGRect) { super.drawText(in: rect.insetBy(dx: 7, dy: 3)) }
    override var intrinsicContentSize: CGSize { let s = super.intrinsicContentSize; return CGSize(width: s.width + 14, height: s.height + 6) }
    override func sizeThatFits(_ size: CGSize) -> CGSize { let s = super.sizeThatFits(size); return CGSize(width: s.width + 14, height: s.height + 6) }
}
