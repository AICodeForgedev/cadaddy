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
    /// Full MEP's additions per level (built the first time Full MEP or the floor is shown).
    private var full: [Entity] = []
    private var fullBuilt: [Bool] = []
    /// The Architectural view per level: solid slabs, walls with their windows, doors and stairs.
    private var arch: [Entity] = []
    private var archBuilt: [Bool] = []
    private var cutBuilt: [Bool] = []
    /// MEP's one outline around the outside of the building.
    private var outline = Entity()
    private var detail: [Entity] = []
    private var serving: [Entity] = []
    private var detailBuilt: [Bool] = []
    private var tiles: [Entity] = []
    private var lift: [Float] = [], liftVel: [Float] = [], liftGoal: [Float] = []
    private var exterior: [(entity: Entity, x: Float)] = []
    private var ground = Entity()
    private var roomDetail: Entity?
    private var roomDetailNumber: String?
    /// A package set: the room package the open room was built from, and each entity's package.
    fileprivate var roomDetailPackage: String?
    fileprivate var packageOf: [String: String] = [:]

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
    private var hoistMat: RealityKit.Material = UnlitMaterial()
    private var matCache: [String: RealityKit.Material] = [:]
    private var originals: [ObjectIdentifier: [RealityKit.Material]] = [:]
    private var glowTexture: TextureResource?
    private var currentFocus: SystemFocus?

    // Depth fog
    private var fogStep: [String: Int] = [:]
    private var fogEye = SIMD3<Float>(repeating: .greatestFiniteMagnitude)
    private var fogDirty = true
    private var fogQueue: [String] = []
    static let fogBudget: Double = 0.0015
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
        #if DEBUG
        let attachStart = CACurrentMediaTime()
        defer { if logPerf { print(String(format: "FIELD-ATTACH %.1f ms (renderer age %.0f ms)", (CACurrentMediaTime() - attachStart) * 1000, Date().timeIntervalSince(fpsStart) * 1000)) } }
        #endif
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

        let m0 = CACurrentMediaTime()
        makeMaterials()
        #if DEBUG
        if logPerf { print(String(format: "FIELD-MATS %.1f ms", (CACurrentMediaTime() - m0) * 1000)) }
        #endif
        _ = m0
        buildStatic()
        sub = v.scene.subscribe(to: SceneEvents.Update.self) { [weak self] e in
            MainActor.assumeIsolated { self?.tick(e.deltaTime) }
        }
        snapCamera()
    }

    func stop() { sub?.cancel(); sub = nil }

    private func tier(_ e: BuildingElement) -> ViewTier { exp?.filter.classifier.tier(e.id) ?? .room }
    /// A real building from its package set (the sample's generated shell is not used for it).
    private var isSet: Bool { b.isPackageSet }
    /// The package set's architecture parts (merged into the shell, cut and room views, not drawn one by one).
    static let shellParts: Set<ElementPart> = [.wall, .slab, .door, .window, .stair, .roomTile, .exterior, .outline]
    private func isShell(_ e: BuildingElement) -> Bool { e.isBaseGroup || Self.shellParts.contains(e.part) || e.system == .architecture }
    private func isPanel(_ e: BuildingElement) -> Bool { e.part == .panel }
    private func isLuminaire(_ e: BuildingElement) -> Bool { e.kind == "Luminaire" || (e.package != nil && e.part == .light) }

    static func ui(_ c: SIMD3<Float>) -> UIColor { UIColor(red: CGFloat(c.x), green: CGFloat(c.y), blue: CGFloat(c.z), alpha: 1) }

    // MARK: work queue

    private func enqueue(_ f: @escaping () -> Void) { work.append(f) }

    private func runWork(budget: Double) {
        guard workHead < work.count else { return }
        let start = CACurrentMediaTime()
        while workHead < work.count {
            let f = work[workHead]
            workHead += 1
            #if DEBUG
            let itemStart = CACurrentMediaTime()
            f()
            if logPerf, CACurrentMediaTime() - itemStart > 0.008 { print(String(format: "FIELD-ITEM %.1f ms %@", (CACurrentMediaTime() - itemStart) * 1000, lastBuilt)) }
            #else
            f()
            #endif
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
        var h = UnlitMaterial(color: style.color(.conveying))
        h.blending = .transparent(opacity: .init(floatLiteral: style.highContrast ? 0.24 : 0.16))
        h.writesDepth = false
        hoistMat = h
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
        case .equipment where e.part == .hoistway:
            // The one shaft MEP shows: a quiet see-through volume (one shared material, like the ghost).
            return hoistMat
        case .equipment where isPanel(e):
            // Panels read as small solid orange objects (the spec's floor view), not neutral gear.
            return pbr("panel-\(lit)", style.color(.electrical), rough: 0.45, emissive: lit ? 0.6 : 0.15, fog: f)
        case .equipment:
            return pbr("eq-\(e.system)-\(lit)", lit ? style.color(e.system) : style.equipment(e.system), rough: 0.55, emissive: lit ? 0.5 : 0.04, fog: f)
        case .main:
            return pbr("main-\(e.system)-\(lit)", style.color(e.system), rough: 0.42, emissive: boost, fog: f)
        case .branch, .unknown:
            return pbr("br-\(e.system)-\(lit)", lit ? style.color(e.system) : style.branch(e.system), rough: 0.5, emissive: lit ? 0.9 : 0.08, fog: f)
        case .terminal:
            if isLuminaire(e) { return unlit("lum", style.lightWarm, fog: f) }
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
        let base = e.role == .equipment && !isPanel(e) ? style.equipment(e.system) : style.color(e.system)
        return unlit("edge-\(e.role)-\(isPanel(e))-\(e.system)", style.outline(base), fog: fog)
    }

    // MARK: static scene (shell, mains, exterior)

    private func buildStatic() {
        let n = b.levels.count
        levelRoots = (0..<n).map { _ in Entity() }
        ghost = (0..<n).map { _ in Entity() }
        cut = (0..<n).map { _ in Entity() }
        mains = (0..<n).map { _ in Entity() }
        full = (0..<n).map { _ in Entity() }
        arch = (0..<n).map { _ in Entity() }
        fullBuilt = Array(repeating: false, count: n)
        archBuilt = Array(repeating: false, count: n)
        cutBuilt = Array(repeating: false, count: n)
        detail = (0..<n).map { _ in Entity() }
        serving = (0..<n).map { _ in Entity() }
        tiles = (0..<n).map { _ in Entity() }
        detailBuilt = Array(repeating: false, count: n)
        lift = Array(repeating: 0, count: n); liftVel = lift; liftGoal = lift
        for i in 0..<n {
            let root = levelRoots[i]
            root.name = "level-\(i)"
            anchor.addChild(root)
            for c in [ghost[i], cut[i], tiles[i], mains[i], full[i], detail[i], serving[i], arch[i]] { root.addChild(c) }
            cut[i].isEnabled = false
            tiles[i].isEnabled = false
            full[i].isEnabled = false
            arch[i].isEnabled = false
        }
        for e in b.elements where tier(e) == .mains || tier(e) == .fullMEP {
            let bb = e.shape.bounds
            if bb.max.y - bb.min.y > 5 { risers.insert(e.id) }
        }
        // What the opening needs right away: the solid exterior and the ground. The ghost shell and the mains
        // are behind the solid until the sweep starts, so they build over the next frames.
        let s0 = CACurrentMediaTime()
        buildGround()
        let s1 = CACurrentMediaTime()
        if isSet { buildSetExterior() } else { buildExterior() }
        let s2 = CACurrentMediaTime()
        if isSet { buildSetOutline() } else { buildOutline() }
        buildPulse()
        #if DEBUG
        if logPerf { print(String(format: "FIELD-STATIC ground %.1f exterior %.1f outline+pulse %.1f", (s1 - s0) * 1000, (s2 - s1) * 1000, (CACurrentMediaTime() - s2) * 1000)) }
        #endif
        for i in 0..<n { enqueue { [weak self] in self?.buildShell(i) } }
        // One element per work item: the frame budget is checked between items, so no item is long.
        for e in b.elements where tier(e) == .mains && !(isSet && isShell(e)) {
            enqueue { [weak self] in
                guard let self, self.mains.indices.contains(e.level) else { return }
                self.addElement(e, home: self.mains[e.level])
            }
        }
    }

    /// Queues `items` into `parent` in chunks: each chunk gets a holder that starts fading in at once, then one
    /// work item per element, so the frame budget holds and a chunk arrives as one soft fade.
    private func enqueueElements(_ items: [BuildingElement], into parent: @escaping () -> Entity?, name: String, chunk: Int = 24) {
        for (n, start) in stride(from: 0, to: items.count, by: chunk).enumerated() {
            let slice = Array(items[start..<min(start + chunk, items.count)])
            let holder = Entity()
            holder.name = "\(name)-\(n)"
            enqueue { [weak self] in
                guard let self, let p = parent() else { return }
                p.addChild(holder)
                if !self.reduceMotion { self.fade(holder, from: 0, to: 1, delay: min(Double(n) * 0.02, 0.2)) }
            }
            for e in slice {
                enqueue { [weak self, weak holder] in
                    guard let self, let holder, holder.parent != nil else { return }
                    self.addElement(e, home: holder)
                }
            }
        }
    }

    private func buildShell(_ i: Int) {
        guard let lv = b.level(i), ghost.indices.contains(i) else { return }
        if isSet {
            // MEP's context, as the mains package has it: the floors (its base nodes on this level), as the ghost.
            var faces = MeshKit(), edges = MeshKit()
            let w: Float = style.highContrast ? 0.07 : 0.05
            for e in b.elements where e.level == i && tier(e) == .mains && isShell(e) && e.part != .outline {
                guard case let .mesh(m) = e.shape else { continue }
                faces.append(m)
                edges.edges(center: (m.min + m.max) / 2, size: m.max - m.min, width: w)
            }
            if let m = faces.resource("ghost-\(i)") { ghost[i].addChild(ModelEntity(mesh: m, materials: [ghostMat])) }
            if let m = edges.resource("edges-\(i)") { ghost[i].addChild(ModelEntity(mesh: m, materials: [edgeMat])) }
            return
        }
        let lo = b.footprintMin, hi = b.footprintMax
        var faces = MeshKit(), edges = MeshKit()
        let w: Float = style.highContrast ? 0.07 : 0.05
        if i > 0 {
            // The slab this level stands on (the roof's is the roof).
            let c = SIMD3<Float>((lo.x + hi.x) / 2, lv.elevation - 0.15, (lo.y + hi.y) / 2), s = SIMD3<Float>(hi.x - lo.x, 0.3, hi.y - lo.y)
            faces.box(center: c, size: s)
            edges.edges(center: c, size: s, width: w)
        }
        // MEP's context is the floors only (and one outline around the whole building, built once): no walls,
        // windows, doors, stairs or shafts. Interior walls appear in the floor's cut view.
        if let m = faces.resource("ghost-\(i)") { ghost[i].addChild(ModelEntity(mesh: m, materials: [ghostMat])) }
        if let m = edges.resource("edges-\(i)") { ghost[i].addChild(ModelEntity(mesh: m, materials: [edgeMat])) }
    }

    /// The floor's cut view (walls at 4 ft, plate, room tiles): built when a floor is first opened, or after the
    /// opening when the app is idle, in small work items.
    private func ensureCut(_ i: Int) {
        if isSet { ensureSetCut(i); return }
        guard cut.indices.contains(i), !cutBuilt[i], let lv = b.level(i) else { return }
        cutBuilt[i] = true
        let lo = b.footprintMin, hi = b.footprintMax
        guard lv.height > 0 || i == b.levels.count - 1 else { return }
        // The cut view in separate work items (faces, edges, plate, then one item per room tile).
        let walls = b.walls(level: i, cut: 1.22)
        enqueue { [weak self] in
            guard let self, self.cut.indices.contains(i) else { return }
            var cf = MeshKit()
            for (c, s) in walls { cf.box(center: c, size: s) }
            if let m = cf.resource("cut-\(i)") {
                var cutMat = UnlitMaterial(color: self.style.ghost)
                cutMat.blending = .transparent(opacity: .init(floatLiteral: min(self.style.ghostOpacity * 2.2, 0.32)))
                cutMat.writesDepth = false
                self.cut[i].addChild(ModelEntity(mesh: m, materials: [cutMat]))
            }
        }
        enqueue { [weak self] in
            guard let self, self.cut.indices.contains(i) else { return }
            var ce = MeshKit()
            for (c, s) in walls { ce.edges(center: c, size: s, width: 0.035) }
            if let m = ce.resource("cutedge-\(i)") { self.cut[i].addChild(ModelEntity(mesh: m, materials: [self.edgeMat])) }
        }
        enqueue { [weak self] in
            guard let self, self.cut.indices.contains(i) else { return }
            var plate = MeshKit()
            plate.box(center: SIMD3((lo.x + hi.x) / 2, lv.elevation - 0.13, (lo.y + hi.y) / 2), size: SIMD3(hi.x - lo.x, 0.26, hi.y - lo.y))
            if let m = plate.resource("plate-\(i)") { self.cut[i].addChild(ModelEntity(mesh: m, materials: [self.pbr("plate", self.style.floorPlate, rough: 0.95)])) }
        }
        // Room tiles (tap targets on the floor view).
        for r in b.rooms(on: i) where r.name != "Corridor" { enqueue { [weak self] in self?.buildTile(r, level: lv) } }
    }

    private func buildTile(_ r: BuildingRoom, level lv: BuildingLevel) {
        let i = lv.index
        guard tiles.indices.contains(i) else { return }
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

    /// One hairline outline around the outside of the building, ground to roof (MEP's only context besides the floors).
    private func buildOutline() {
        let lo = b.footprintMin, hi = b.footprintMax
        let top = b.level(b.levels.count - 1)?.elevation ?? b.height
        var k = MeshKit()
        k.edges(center: SIMD3((lo.x + hi.x) / 2, top / 2, (lo.y + hi.y) / 2), size: SIMD3(hi.x - lo.x, top, hi.y - lo.y), width: style.highContrast ? 0.08 : 0.06)
        outline = Entity()
        if let m = k.resource("outline") { outline.addChild(ModelEntity(mesh: m, materials: [edgeMat])) }
        anchor.addChild(outline)
    }

    /// The Architectural view of one level, solid: its slab, exterior walls with their windows, interior walls,
    /// doors and stairs (the roof level: the roof and parapet). A few merged meshes per level.
    private func ensureArch(_ i: Int) {
        guard arch.indices.contains(i), !archBuilt[i] else { return }
        if isSet, !b.elements.contains(where: { $0.level == i && tier($0) == .architecture }) { return }
        archBuilt[i] = true
        enqueue { [weak self] in
            guard let self, self.arch.indices.contains(i) else { return }
            self.buildArch(i)
            // Arrives with a soft fade if the Architectural view is still showing.
            if self.exp?.wholeView == .architectural, self.exp?.scope == .building, !self.reduceMotion { self.fade(self.arch[i], from: 0, to: 1, delay: 0) }
        }
    }

    private func buildArch(_ i: Int) {
        guard let lv = b.level(i) else { return }
        if isSet { buildSetArch(i); return }
        let lo = b.footprintMin, hi = b.footprintMax
        var slab = MeshKit(), wall = MeshKit(), dark = MeshKit(), pane = MeshKit(), door = MeshKit(), stair = MeshKit()
        let top = b.levels.count - 1
        if i == top {
            slab.box(min: SIMD3(lo.x, lv.elevation - 0.3, lo.y), max: SIMD3(hi.x, lv.elevation, hi.y))
            for z in [lo.y, hi.y - 0.3] { wall.box(min: SIMD3(lo.x, lv.elevation, z), max: SIMD3(hi.x, lv.elevation + 0.9, z + 0.3)) }
            for x in [lo.x, hi.x - 0.3] { wall.box(min: SIMD3(x, lv.elevation, lo.y), max: SIMD3(x + 0.3, lv.elevation + 0.9, hi.y)) }
        } else if lv.height > 0 {
            let e = lv.elevation, h = lv.height, t: Float = 0.3
            slab.box(min: SIMD3(lo.x, e - 0.3, lo.y), max: SIMD3(hi.x, e, hi.y))
            // Long faces: wall around each window (sill, head, piers); the window itself is the glass.
            for w in b.elements where w.level == i && w.part == .window {
                guard case let .box(c, sz) = w.shape else { continue }
                let south = c.z < (lo.y + hi.y) / 2
                let z0 = south ? lo.y : hi.y - t, z1 = z0 + t
                let bayX0 = (c.x / b.bay).rounded(.down) * b.bay, bayX1 = bayX0 + b.bay
                let sill = c.y - sz.y / 2, head = c.y + sz.y / 2
                if i == 1 { dark.box(min: SIMD3(bayX0, e, z0), max: SIMD3(bayX1, sill, z1)) } else { wall.box(min: SIMD3(bayX0, e, z0), max: SIMD3(bayX1, sill, z1)) }
                wall.box(min: SIMD3(bayX0, head, z0), max: SIMD3(bayX1, e + h, z1))
                wall.box(min: SIMD3(bayX0, sill, z0), max: SIMD3(c.x - sz.x / 2, head, z1))
                wall.box(min: SIMD3(c.x + sz.x / 2, sill, z0), max: SIMD3(bayX1, head, z1))
                pane.box(center: c, size: SIMD3(sz.x, sz.y, 0.04))
            }
            // End walls, full height.
            for x in [lo.x, hi.x - t] { wall.box(min: SIMD3(x, e, lo.y), max: SIMD3(x + t, e + h, hi.y)) }
            // Interior walls, full height (their door gaps stay open; the doors fill them).
            for (c, sz) in b.walls(level: i).dropFirst(4) { wall.box(center: SIMD3(c.x, e + h / 2, c.z), size: SIMD3(sz.x, h, sz.z)) }
            for d in b.elements where d.level == i && d.part == .door { if case let .box(c, sz) = d.shape { door.box(center: c, size: sz) } }
            // Stairs: two flights and a landing inside each stair's volume.
            for st in b.elements where st.level == i && st.part == .stair {
                guard case let .box(c, sz) = st.shape else { continue }
                let steps = 10
                let x0 = c.x - sz.x / 2, x1 = c.x + sz.x / 2, z0 = c.z - sz.z / 2, z1 = c.z + sz.z / 2
                let run = (z1 - z0 - 1.2) / Float(steps), rise = h / Float(2 * steps)
                for k in 0..<steps {
                    let ya = e + Float(k + 1) * rise
                    stair.box(min: SIMD3(x0, e, z0 + Float(k) * run), max: SIMD3(c.x - 0.05, ya, z0 + Float(k + 1) * run))
                    let yb = e + h / 2 + Float(k + 1) * rise
                    stair.box(min: SIMD3(c.x + 0.05, e + h / 2, z1 - 1.2 - Float(k + 1) * run), max: SIMD3(x1, yb, z1 - 1.2 - Float(k) * run))
                }
                stair.box(min: SIMD3(x0, e + h / 2 - 0.15, z1 - 1.2), max: SIMD3(x1, e + h / 2, z1))
            }
        }
        let mats: [(MeshKit, RealityKit.Material, String)] = [
            (slab, pbr("arch-slab", style.isDark ? UIColor(hex: 0xC9CDCB) : UIColor(hex: 0xD9DDE1), rough: 0.9), "slab"),
            (wall, pbr("facade", style.facade, rough: 0.82), "wall"),
            (dark, pbr("facade-dark", style.facadeDark, rough: 0.9), "base"),
            (pane, pbr("glass", style.glass, rough: 0.12, metal: 0.4), "glass"),
            (door, pbr("arch-door", UIColor(hex: 0x8A6A4E), rough: 0.7), "door"),
            (stair, pbr("arch-stair", UIColor(hex: 0xB7BBBE), rough: 0.85), "stair"),
        ]
        for (mk, m, name) in mats { if let r = mk.resource("arch-\(name)-\(i)") { arch[i].addChild(ModelEntity(mesh: r, materials: [m])) } }
    }

    /// Full MEP's additions on one level, a chunk per frame.
    private func ensureFull(_ level: Int) {
        guard full.indices.contains(level), !fullBuilt[level] else { return }
        let items = b.elements.filter { $0.level == level && tier($0) == .fullMEP && !(isSet && isShell($0)) }
        if isSet && items.isEmpty { return }   // its package is not loaded yet
        fullBuilt[level] = true
        enqueueElements(items, into: { [weak self] in self.flatMap { $0.full.indices.contains(level) ? $0.full[level] : nil } }, name: "full-\(level)")
    }

    /// The hard rule, applied last: an element the active view does not allow is never drawn, whatever its
    /// container, layer switch or fade says.
    private func enforceLimit() {
        guard let exp else { return }
        let state = exp.viewState, prefs = exp.layers
        // Only when the view or the switches changed (elements made later clamp themselves in addElement).
        let key = "\(state.view)|\(state.density)|\(state.level ?? -1)|\(state.room ?? "")|\(prefs.off.map(\.rawValue).sorted())|\(elementEntities.count)"
        guard key != limitKey else { return }
        limitKey = key
        for (id, ent) in elementEntities {
            guard let e = b.element(id) else { continue }
            let ok = exp.filter.allows(e, state, prefs: prefs)
            if ok != limitOK(ent) { setLimit(ent, ok) }
        }
    }

    private var limitKey = ""
    #if DEBUG
    private var lastBuilt = ""
    #endif
    private struct LimitTag: Component { var blocked: Bool }
    private func limitOK(_ e: Entity) -> Bool { !(e.components[LimitTag.self]?.blocked ?? false) }
    private func setLimit(_ e: Entity, _ ok: Bool) {
        e.components.set(LimitTag(blocked: !ok))
        // Blocked: hide the model and its outline (children) without fighting the fades on the containers.
        e.isEnabled = ok
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
        guard let layer = e.layer else { return parent }
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
        case let .mesh(em):
            var k = MeshKit()
            k.append(em)
            mesh = k.resource(e.id)
        }
        guard let mesh else { return nil }
        let m = ModelEntity(mesh: mesh, materials: [material(for: e)])
        m.position = position
        m.name = e.id
        m.components.set(ElementTagComponent(id: e.id))
        let bb = e.shape.bounds
        let minTap: Float = tier(e) == .room ? 0.14 : 0.45
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
        if tier(e) != .room {
            var k = MeshKit()
            let w: Float = tier(e) == .mains ? 0.045 : 0.03
            switch e.shape {
            case let .box(_, s) where e.role == .equipment: k.edges(center: .zero, size: s, width: w)
            case let .run(points, profile) where e.role == .main: k.runEdges(points, profile: profile, width: w)
            case let .mesh(em) where e.role == .equipment && e.part != .hoistway: k.edges(center: (em.min + em.max) / 2, size: em.max - em.min, width: w)
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
        if isLuminaire(e), let t = glowTexture, let q = MeshKit.quad(center: (isSet ? e.shape.anchor : .zero) + SIMD3(0, -0.04, 0), size: SIMD2(1.3, 1.3)) {
            var gm = UnlitMaterial()
            gm.color = .init(tint: .white, texture: .init(t))
            gm.blending = .transparent(opacity: .init(floatLiteral: 1))
            gm.writesDepth = false
            let glow = ModelEntity(mesh: q, materials: [gm])
            m.addChild(glow)
            // And a pool of light on the floor, very soft.
            if let h = e.mountingHeight, let pool = MeshKit.quad(center: (isSet ? e.shape.anchor : .zero) + SIMD3(0, -h + 0.05, 0), size: SIMD2(2.6, 2.6)) {
                var pm = UnlitMaterial()
                pm.color = .init(tint: UIColor.white.withAlphaComponent(0.55), texture: .init(t))
                pm.blending = .transparent(opacity: .init(floatLiteral: 0.5))
                pm.writesDepth = false
                m.addChild(ModelEntity(mesh: pool, materials: [pm]))
            }
        }
        elementEntities[e.id] = m
        fogStep[e.id] = 0
        if let pk = e.package { packageOf[e.id] = pk }
        #if DEBUG
        lastBuilt = "\(e.kind) \(e.tag)"
        #endif
        // Made later than the view change: take the active view's limit straight away.
        if let exp, !exp.filter.allows(e, exp.viewState, prefs: exp.layers) { setLimit(m, false) }
        // Made while following the system: take the follow look straight away.
        if let f = currentFocus { styleForFocus(e, m, f) }
        return m
    }

    /// The floor's branches, lights and terminals — built the first time the floor is opened, a small chunk per
    /// frame. Each chunk's layer groups fade in as a whole (a few fades, not one per element).
    private func ensureDetail(_ level: Int) {
        guard detail.indices.contains(level), !detailBuilt[level] else { return }
        let items = b.elements.filter { $0.level == level && tier($0) == .floor && !(isSet && isShell($0)) }
        if isSet && items.isEmpty && !b.elements.contains(where: { $0.level == level && tier($0) == .floor }) { return }
        detailBuilt[level] = true
        enqueueElements(items, into: { [weak self] in self.flatMap { $0.detail.indices.contains(level) ? $0.detail[level] : nil } }, name: "chunk-\(level)")
    }

    private func buildRoom(_ number: String) {
        if roomDetailNumber == number { return }
        removeRoomDetail()
        if isSet { buildSetRoom(number); return }
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
        // The room's things arrive one per work item, each with its own short staggered fade.
        let items = b.elements(in: number).filter { tier($0) == .room }
        for (k, el) in items.enumerated() {
            enqueue { [weak self, weak root] in
                guard let self, let root, root.parent != nil, let ent = self.addElement(el, home: root) else { return }
                if !self.reduceMotion { self.fade(ent, from: 0, to: 1, delay: 0.05 + Double(k) * 0.012) }
            }
        }
        if !reduceMotion { fade(root, from: 0, to: 1, delay: 0) }
        roomDetail = root
        roomDetailNumber = number
    }

    private func removeRoomDetail() {
        guard let r = roomDetail else { return }
        for el in b.elements(in: roomDetailNumber ?? "") where tier(el) == .room || isSet {
            elementEntities[el.id] = nil
            edgeEntities[el.id] = nil
            fogStep[el.id] = nil
        }
        if let pk = roomDetailPackage {
            // The package may already be released from the building: sweep by the entities' own package.
            for (id, p) in packageOf where p == pk { elementEntities[id] = nil; edgeEntities[id] = nil; fogStep[id] = nil; packageOf[id] = nil }
            roomDetailPackage = nil
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
        #if DEBUG
        let applyStart = CACurrentMediaTime()
        defer { if logPerf { print(String(format: "FIELD-APPLY %.1f ms %@", (CACurrentMediaTime() - applyStart) * 1000, String(describing: scope))) } }
        #endif
        guard exp != nil else { return }
        let n = b.levels.count
        userMoved = false
        switch scope {
        case .building:
            dropRoom()
            roomServing = []
            let offs = FloorLift.offsets(levels: n, focus: liftFocus, gap: 10, spread: 1.4)
            let architectural = exp?.wholeView == .architectural
            let fullOn = !architectural && exp?.density == .full
            for i in 0..<n {
                liftGoal[i] = offs[i]
                if architectural { ensureArch(i) }
                if fullOn { ensureFull(i) }
                setVisible(arch[i], architectural)
                setVisible(ghost[i], !architectural)
                setVisible(mains[i], !architectural)
                setVisible(full[i], fullOn)
                setVisible(cut[i], false)
                setVisible(tiles[i], false)
                setVisible(detail[i], false)
                liftingAway.remove(i); setVisible(levelRoots[i], true)
            }
            setVisible(outline, !architectural && liftFocus == nil)
            for id in risers { if let e = elementEntities[id] { setOpacity(e, liftFocus == nil ? 1 : 0.18) } }
            orbitGoal.y = liftFocus == nil ? 0.56 : 0.38
            fit(buildingPoints(lift: offs, architectural: architectural, full: fullOn), yaw: nearestAngle(designYaw, to: orbitGoal.x), pitch: orbitGoal.y, filling: true)
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
            let south = r.center.y <= (b.footprintMin.y + b.footprintMax.y) / 2
            // Look in from the outside of the room's long open side.
            let yaw: Float = south ? -2.55 : -0.62
            orbitGoal.x = nearestAngle(yaw, to: orbit.x)
            orbitGoal.y = 0.92
            // What serves this room stays; the rest of the floor steps back as a whole. A package set's room
            // package has its own copy of everything in and through the room, so nothing moves.
            var serves = Set<String>()
            if !isSet { for el in b.elements(in: number) { if let u = el.upstream { serves.insert(u) } } }
            if roomDetailNumber != number { removeRoomDetail() }
            roomServing = serves
            showFloor(r.level)
            setVisible(mains[r.level], false)
            setVisible(full[r.level], false)
            setVisible(detail[r.level], false)
            // The room's own lights, cabinets, fans and heads live on the floor; they come along with what serves it.
            for el in b.elements(in: number) where !isSet && tier(el) != .room && tier(el) != .architecture { serves.insert(el.id) }
            roomServing = serves
            moveServing(level: r.level, ids: serves)
            buildRoom(number)
            fit(roomPoints(r), yaw: orbitGoal.x, pitch: orbitGoal.y)
            for (k, _) in raise { raise[k]?.goal = k == number ? 0.18 : 0 }
            for (k, t) in roomTileEntities where b.room(k)?.level == r.level { setOpacity(t, k == number ? 1 : 0.35) }
        }
        if reduceMotion { snapCamera() }
        #if DEBUG
        let tA = CACurrentMediaTime()
        #endif
        applyLayers()
        #if DEBUG
        let tB = CACurrentMediaTime()
        #endif
        refreshLabels(scope)
        #if DEBUG
        if logPerf { print(String(format: "FIELD-APPLY-PARTS body %.1f layers+limit %.1f labels %.1f", (tA - applyStart) * 1000, (tB - tA) * 1000, (CACurrentMediaTime() - tB) * 1000)) }
        #endif
        fogDirty = true
    }

    /// After the opening: build every floor's cut view in the idle frames, so opening a floor later is light.
    func prewarmFloors() {
        // A package set loads a level only when it is opened (never every level up front).
        guard !isSet else { return }
        for i in b.levels.indices { ensureCut(i) }
    }

    private func showFloor(_ i: Int) {
        let n = b.levels.count
        ensureCut(i)
        ensureDetail(i)
        ensureFull(i)
        setVisible(outline, false)
        for a in arch { setVisible(a, false) }
        for j in 0..<n {
            if j > i {
                liftGoal[j] = 16 + Float(j - i) * 2.5
                // No opacity fade (that touches every entity on the level each frame): the floors above lift out
                // of the frame and switch off when they get there.
                fades[ObjectIdentifier(levelRoots[j])] = nil
                liftingAway.insert(j)
            } else if j < i {
                liftGoal[j] = 0
                liftingAway.remove(j); setVisible(levelRoots[j], true)
                setVisible(ghost[j], true)
                setVisible(mains[j], false)
                setVisible(full[j], false)
                setVisible(detail[j], false)
                setVisible(cut[j], false)
                setVisible(tiles[j], false)
            } else {
                liftGoal[j] = 0
                liftingAway.remove(j); setVisible(levelRoots[j], true)
                setVisible(ghost[j], false)
                // A package set's floor is its level package (its own copies); the building copies step aside.
                setVisible(mains[j], !isSet)
                setVisible(full[j], !isSet)
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
    private func buildingPoints(lift offs: [Float], architectural: Bool = false, full: Bool = false) -> [SIMD3<Float>] {
        let key = architectural ? 0 : (full ? 2 : 1)
        if levelBounds[key] == nil {
            // Each level's box with what the view draws on it (risers excluded), worked out once per view.
            let lo = b.footprintMin, hi = b.footprintMax
            var boxes = b.levels.map { lv in (SIMD3<Float>(lo.x, lv.elevation - 0.3, lo.y), SIMD3<Float>(hi.x, lv.elevation + max(lv.height, 1.0), hi.y)) }
            if !architectural {
                for e in b.elements where boxes.indices.contains(e.level) && !risers.contains(e.id) && (tier(e) == .mains || (full && tier(e) == .fullMEP)) {
                    let bb = e.shape.bounds
                    boxes[e.level].0 = simd_min(boxes[e.level].0, bb.min); boxes[e.level].1 = simd_max(boxes[e.level].1, bb.max)
                }
            }
            levelBounds[key] = boxes
        }
        var pts: [SIMD3<Float>] = []
        for (i, box) in (levelBounds[key] ?? []).enumerated() {
            let up = SIMD3<Float>(0, offs.indices.contains(i) ? offs[i] : 0, 0)
            pts += SceneFraming.corners(min: box.0 + up, max: box.1 + up)
        }
        return pts
    }
    private var levelBounds: [Int: [(SIMD3<Float>, SIMD3<Float>)]] = [:]
    /// Levels lifting out of the frame (switched off when they are out of it).
    private var liftingAway: Set<Int> = []

    /// The floor's plate, its cut walls and what sits on it (not the risers that run through it).
    private func floorPoints(_ i: Int) -> [SIMD3<Float>] {
        let lo = b.footprintMin, hi = b.footprintMax
        let e = b.level(i)?.elevation ?? 0
        var mn = SIMD3<Float>(lo.x, e - 0.3, lo.y), mx = SIMD3<Float>(hi.x, e + 1.3, hi.y)
        var any = false
        for el in b.elements where el.level == i && tier(el) != .room && tier(el) != .architecture && !risers.contains(el.id) {
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
    private func fit(_ pts: [SIMD3<Float>], yaw: Float, pitch: Float, turn: Float = 0, filling: Bool = false) {
        framePoints = pts
        frameTurn = turn
        frameFilling = filling
        if filling { fillPitch = pitch }
        guard let vp = viewport, !pts.isEmpty else { return }
        // The building views fill the free area (Q6431): they may turn and look down a little to do it.
        let pose = filling
            ? SceneFraming.fitFilling(pts, yaw: yaw, pitch: fillPitch, fovYDegrees: Self.fov, viewport: vp, free: freeArea, margin: 14)
            : SceneFraming.fitTurning(pts, yaw: yaw, spread: turn, pitch: pitch, fovYDegrees: Self.fov, viewport: vp, free: freeArea, margin: 14)
        targetGoal = pose.target
        orbitGoal.y = pose.pitch
        orbitGoal.x = pose.yaw
        orbitGoal.z = pose.distance
        lastFree = freeArea
        fittedViewport = vp
    }
    /// The view size the last fit used: a fit asked for before the view had a size is redone once it has one.
    private var fittedViewport: CGSize = .zero
    private var frameTurn: Float = 0
    private var frameFilling = false
    /// The design pitch a filling fit started from (it may look down more; a refit starts from the same place).
    private var fillPitch: Float = 0.56
    /// The design angle each scope starts from (turning from the last fitted angle would drift).
    private var designYaw: Float = -2.25

    /// The chrome moved (a sheet grew, the text size changed): frame the same subject again, unless the user has
    /// moved the camera since.
    func chromeChanged() {
        guard !userMoved, !framePoints.isEmpty, freeArea != lastFree || viewport != fittedViewport else { return }
        fit(framePoints, yaw: frameTurn > 0 || frameFilling ? nearestAngle(designYaw, to: orbitGoal.x) : orbitGoal.x, pitch: frameFilling ? fillPitch : orbitGoal.y,
            turn: frameTurn, filling: frameFilling)
        if reduceMotion { snapCamera() }
    }

    // MARK: layers

    private func layersShow(_ e: BuildingElement) -> Bool { e.layer.map { exp?.layers.isOn($0) ?? true } ?? true }

    /// Fades each layer group, a handful of entities however big the building is.
    func applyLayers() {
        guard let exp else { return }
        groupList.removeAll { $0.entity == nil }
        let offered = Set(exp.activeView.layers)
        for g in groupList {
            guard let e = g.entity else { continue }
            // A switch the view does not offer: on for M/E/P, off for cabinets and furniture (the limit decides the rest).
            let on = offered.contains(g.layer) ? exp.layers.isOn(g.layer) : !(g.layer == .cabinets || g.layer == .furniture)
            setOpacity(e, on ? 1 : 0)
        }
        enforceLimit()
    }

    // MARK: follow the system

    private func styleForFocus(_ e: BuildingElement, _ ent: Entity, _ focus: SystemFocus) {
        guard let m = ent as? ModelEntity else { return }
        let key = ObjectIdentifier(m)
        // Furniture and casework are context in a room, not part of any system: they stay as they are.
        let lit = focus.lit.contains(e.sourceID)
        if !lit, e.role == .furniture || e.role == .casework { return }
        // A package set's shell copies stay as they are (they are context, merged or not).
        if isSet, isShell(e) { return }
        originals[key] = m.model?.materials
        entityByObject[key] = m
        m.model?.materials = lit ? [material(for: e, lit: true)] : [fadedMat]
        if lit, focus.lit.count < 400, e.role != .equipment || e.sourceID == focus.tapped { addHalo(e, to: m) }
        if let edge = edgeEntities[e.id], !lit { edge.isEnabled = false; hiddenGlows.append(edge) }
        for case let child as ModelEntity in m.children where isLuminaire(e) && !focus.lit.contains(e.sourceID) {
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
            guard let e = shown(s.id) else { continue }
            switch e.shape {
            case let .box(c, _): path.append((c, e.level))
            case let .run(points, _): path += points.reversed().map { ($0, e.level) }
            case .mesh: path.append((e.shape.anchor, e.level))
            }
        }
        if focus.showsDownstream, focus.steps.count <= 1, let e = shown(focus.tapped) { path = [(e.shape.anchor, e.level)] }
        pulsePath = path
        pulseT = 0
        let c = shown(focus.tapped).map { style.color($0.system).mixed(with: .white, 0.45) } ?? .white
        for (i, p) in pulse.enumerated() {
            var m = UnlitMaterial(color: c)
            if i > 0 { m.blending = .transparent(opacity: .init(floatLiteral: 0.8 - Float(i) * 0.15)) }
            p.model?.materials = [m]
        }
    }

    /// The copy of the export's element `source` the active view draws (the sample: the element itself).
    func shown(_ source: String) -> BuildingElement? {
        guard isSet else { return b.element(source) }
        let copies = b.copies(of: source)
        if let exp { let st = exp.viewState; if let c = copies.first(where: { exp.filter.allows($0, st, prefs: exp.layers) }) { return c } }
        return copies.first
    }

    private var entityByObject: [ObjectIdentifier: Entity] = [:]
    private var halos: [Entity] = []

    /// A soft sheath around a lit element, in its system colour. Same colour and opacity everywhere, no depth
    /// writes: like the ghost, it blends the same in any order.
    private func addHalo(_ e: BuildingElement, to ent: Entity) {
        var k = MeshKit()
        switch e.shape {
        case let .mesh(em): k.box(center: (em.min + em.max) / 2, size: em.max - em.min + SIMD3(repeating: 0.12))
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
        outline.removeFromParent()
        ground.removeFromParent()
        pulse.forEach { $0.removeFromParent() }
        pulse = []
        work.removeAll(); workHead = 0
        elementEntities.removeAll(); edgeEntities.removeAll(); roomTileEntities.removeAll(); raise.removeAll(); fades.removeAll()
        groups.removeAll(); groupList.removeAll(); fogStep.removeAll(); fogQueue.removeAll(); risers.removeAll(); levelBounds.removeAll(); limitKey = ""
        roomDetail = nil; roomDetailNumber = nil; roomDetailPackage = nil; packageOf.removeAll()
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
        #if DEBUG
        if logPerf, frameCount < 3 { print(String(format: "FIELD-FRAME %d at %.0f ms", frameCount, Date().timeIntervalSince(fpsStart) * 1000)) }
        #endif
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
        // The view got its size (or a new one) after the subject was chosen: frame it now.
        if let vp = viewport, vp != fittedViewport, !framePoints.isEmpty, !userMoved { chromeChanged() }
        spring.step(&target, &targetVel, toward: targetGoal, dt: dt)
        spring.step(&orbit, &orbitVel, toward: orbitGoal, dt: dt)
        for i in lift.indices {
            spring.step(&lift[i], &liftVel[i], toward: liftGoal[i], dt: dt)
            levelRoots[i].position.y = lift[i]
            if liftingAway.contains(i), lift[i] > liftGoal[i] - 4 { levelRoots[i].isEnabled = false; liftingAway.remove(i) }
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
        let t0 = CACurrentMediaTime()
        positionLabels()
        let t1 = CACurrentMediaTime()
        if frameCount % 3 == 0 || !fogQueue.isEmpty { updateFog() }
        let t2 = CACurrentMediaTime()
        exp?.tick(dt)
        // Whatever is left of this frame's few milliseconds goes to building.
        let t3 = CACurrentMediaTime()
        runWork(budget: max(Self.workBudget - (CACurrentMediaTime() - frameStart), 0.001))
        #if DEBUG
        if logPerf {
            let t4 = CACurrentMediaTime()
            if t4 - frameStart > 0.012 || dt > 0.03 {
                print(String(format: "FIELD-SLOW dt %.0f ms tick %.1f labels %.1f fog %.1f exp %.1f work %.1f", dt * 1000, (t4 - frameStart) * 1000, (t1 - t0) * 1000, (t2 - t1) * 1000, (t3 - t2) * 1000, (t4 - t3) * 1000))
            }
        }
        #endif
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
        // A pass in progress continues (a slice per call, so swapping materials never costs a long frame).
        if fogQueue.isEmpty {
            guard fogDirty || simd_distance(eye, fogEye) > 0.25 else { return }
            fogDirty = false
            fogEye = eye
            fogQueue = Array(elementEntities.keys)
        }
        var near = Float.greatestFiniteMagnitude, far: Float = 0
        for p in framePoints { let d = simd_distance(p, eye); near = min(near, d); far = max(far, d) }
        let scale: Float
        switch exp?.scope ?? .building {
        case .building: scale = 1
        case .floor: scale = 0.85
        case .room: scale = 0.3
        }
        let start = CACurrentMediaTime()
        while !fogQueue.isEmpty, CACurrentMediaTime() - start < Self.fogBudget {
            let id = fogQueue.removeLast()
            guard let ent = elementEntities[id], let e = b.element(id), let m = ent as? ModelEntity, e.role != .shell else { continue }
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
    static func labelPriority(_ e: BuildingElement) -> Int { LabelLayout.priority(e) }

    func refreshLabels(_ scope: DesignScope) {
        var want: [String: (String, SIMD3<Float>, Int, Bool, Int)] = [:]   // key → text, point, level, mono, priority
        switch scope {
        case .building:
            for e in b.elements where exp?.wholeView != .architectural && e.role == .equipment && tier(e) == .mains && e.part != .panel && e.part != .disconnect && e.part != .hoistway && layersShow(e) && labelAllowed(e) {
                want["e-\(e.id)"] = (e.tag, e.shape.bounds.max * SIMD3(0, 1, 0) + e.shape.anchor * SIMD3(1, 0, 1) + SIMD3(0, 0.6, 0), e.level, true, Self.labelPriority(e))
            }
        case let .floor(i):
            for r in b.rooms(on: i) where isSet || r.name != "Corridor" {
                let text = isSet ? r.displayNumber : (r.name == "Guest Room" ? r.number : r.name)
                let area = Int((r.max.x - r.min.x) * (r.max.y - r.min.y))
                want["r-\(r.number)"] = (text, SIMD3(r.center.x, (b.level(i)?.elevation ?? 0) + 0.1, r.center.y), i, isSet ? r.shownNumber != nil : r.name == "Guest Room", 2000 + min(area, 999))
            }
            for e in b.elements where e.level == i && e.role == .equipment && e.part != .hoistway && layersShow(e) && labelAllowed(e) {
                want["e-\(e.id)"] = (e.tag, e.shape.anchor + SIMD3(0, 1.1, 0), e.level, true, Self.labelPriority(e))
            }
        case let .room(n):
            for e in b.elements(in: n) where (e.role == .terminal || e.role == .fixture) && !e.tag.isEmpty && layersShow(e) && labelAllowed(e) {
                want["d-\(e.id)"] = (e.tag, e.shape.bounds.max * SIMD3(0, 1, 0) + e.shape.anchor * SIMD3(1, 0, 1) + SIMD3(0, 0.12, 0), e.level, true, Self.labelPriority(e))
            }
        }
        // Labels leaving go at once: a fading-out label could sit under a new one.
        for (k, l) in labels where want[k] == nil {
            l.removeFromSuperview()
            labels[k] = nil
        }
        // Every key the view no longer wants, including labels still waiting to be made.
        for k in Array(labelAnchors.keys) where want[k] == nil {
            labelAnchors[k] = nil
            labelPriority[k] = nil
            labelSlots[k] = nil
        }
        var made = 0
        for (k, (text, p, lv, mono, prio)) in want.sorted(by: { $0.value.4 > $1.value.4 }) {
            labelAnchors[k] = (p, lv)
            labelPriority[k] = prio
            guard labels[k] == nil else { continue }
            // The most important few now; the rest over the next frames (making a label costs a little).
            if made < 8 { makeLabel(k, text: text, mono: mono); made += 1 } else {
                enqueue { [weak self] in
                    guard let self, self.labelAnchors[k] != nil, self.labels[k] == nil else { return }
                    self.makeLabel(k, text: text, mono: mono)
                }
            }
        }
        leaderLayer.strokeColor = (style.isDark ? UIColor.white : UIColor(hex: 0x0E1A24)).withAlphaComponent(style.highContrast ? 0.8 : 0.5).cgColor
    }

    /// A package set labels only the copies the active view draws (one label per object, never two).
    private func labelAllowed(_ e: BuildingElement) -> Bool {
        guard isSet, let exp else { return true }
        return exp.filter.allows(e, exp.viewState, prefs: exp.layers)
    }

    private func makeLabel(_ k: String, text: String, mono: Bool) {
        do {
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
    }

    /// Screen-space layout every frame: on screen, off the chrome, no two overlapping; moved aside with a short
    /// leader, or dropped lowest priority first (LabelLayout).
    private func positionLabels() {
        guard let v = view, !labels.isEmpty else { leaderLayer.path = nil; return }
        // The tapped object's label shows even when the layout had hidden it (it outranks every other).
        let sel = exp?.selected
        let selectedKeys: Set<String> = sel.map { ["d-\($0)", "e-\($0)"] } ?? []
        var cands: [LabelCandidate] = []
        cands.reserveCapacity(labels.count)
        for (k, l) in labels {
            guard let (p, lv) = labelAnchors[k] else { continue }
            let up = lift.indices.contains(lv) ? lift[lv] : 0
            guard let s = v.project(p + SIMD3(0, up, 0)) else { continue }
            let prio = (labelPriority[k] ?? 0) + (selectedKeys.contains(k) ? LabelLayout.tappedBoost : 0)
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

// MARK: - A real building's package set

@available(iOS 18.0, *)
extension BuildingRenderer {
    /// The opening's solid exterior from the architectural package's `exterior` nodes (each level's floor plates
    /// raised to its storey), cut into bays along x so the sweep runs across it as it does on the sample.
    fileprivate func buildSetExterior() {
        let lo = b.footprintMin.x, hi = b.footprintMax.x
        let parts = b.elements.filter { $0.part == .exterior }
        guard !parts.isEmpty else { return }
        let bays = max(Int(((hi - lo) / b.bay).rounded(.up)), 1)
        let wall = pbr("facade", style.facade, rough: 0.82)
        for k in 0..<bays {
            let x0 = lo + Float(k) * b.bay, x1 = k == bays - 1 ? Float.greatestFiniteMagnitude : x0 + b.bay
            var solid = MeshKit()
            for e in parts { if case let .mesh(m) = e.shape { solid.append(m, xFrom: k == 0 ? -Float.greatestFiniteMagnitude : x0, to: x1) } }
            guard let r = solid.resource("bay-\(k)") else { continue }
            let bay = Entity()
            bay.addChild(ModelEntity(mesh: r, materials: [wall]))
            anchor.addChild(bay)
            exterior.append((bay, x0 + b.bay / 2))
        }
    }

    /// MEP's one outline, as the mains package draws it.
    fileprivate func buildSetOutline() {
        var k = MeshKit()
        for e in b.elements where e.part == .outline { if case let .mesh(m) = e.shape { k.append(m) } }
        outline = Entity()
        if let m = k.resource("outline") { outline.addChild(ModelEntity(mesh: m, materials: [edgeMat])) }
        anchor.addChild(outline)
    }

    /// The level package's own architecture (walls and doors cut at 4 ft, its floor plate) as the floor's pale
    /// context, and its room tiles as the tap targets. Built once the level's package is loaded.
    fileprivate func ensureSetCut(_ i: Int) {
        guard cut.indices.contains(i), !cutBuilt[i] else { return }
        let shell = b.elements.filter { $0.level == i && tier($0) == .floor && isShell($0) }
        guard !shell.isEmpty else { return }
        cutBuilt[i] = true
        enqueue { [weak self] in
            guard let self, self.cut.indices.contains(i) else { return }
            var faces = MeshKit(), plate = MeshKit(), edges = MeshKit()
            for e in shell {
                guard case let .mesh(m) = e.shape else { continue }
                switch e.part {
                case .roomTile: continue
                case .slab: plate.append(m)
                default:
                    faces.append(m)
                    if e.part == .wall { edges.edges(center: (m.min + m.max) / 2, size: m.max - m.min, width: 0.03) }
                }
            }
            var cutMat = UnlitMaterial(color: self.style.ghost)
            cutMat.blending = .transparent(opacity: .init(floatLiteral: min(self.style.ghostOpacity * 2.2, 0.32)))
            cutMat.writesDepth = false
            if let m = faces.resource("cut-\(i)") { self.cut[i].addChild(ModelEntity(mesh: m, materials: [cutMat])) }
            if let m = edges.resource("cutedge-\(i)") { self.cut[i].addChild(ModelEntity(mesh: m, materials: [self.edgeMat])) }
            if let m = plate.resource("plate-\(i)") { self.cut[i].addChild(ModelEntity(mesh: m, materials: [self.pbr("plate", self.style.floorPlate, rough: 0.95)])) }
        }
        for e in shell where e.part == .roomTile {
            enqueue { [weak self] in self?.buildSetTile(e) }
        }
    }

    /// A room's floor tile from its level package: lifted a hair, tappable, rising when tapped.
    fileprivate func buildSetTile(_ e: BuildingElement) {
        guard tiles.indices.contains(e.level), case let .mesh(m) = e.shape, b.room(e.sourceID) != nil, let lv = b.level(e.level) else { return }
        var k = MeshKit()
        k.append(m, offset: SIMD3(-(m.min.x + m.max.x) / 2, -lv.elevation - 0.03, -(m.min.z + m.max.z) / 2))
        guard let res = k.resource("tile-\(e.sourceID)") else { return }
        let tile = ModelEntity(mesh: res, materials: [pbr("tile", style.roomTile, rough: 0.9)])
        tile.position = SIMD3((m.min.x + m.max.x) / 2, lv.elevation + 0.03, (m.min.z + m.max.z) / 2)
        tile.components.set(RoomTagComponent(number: e.sourceID))
        let size = m.max - m.min
        tile.components.set(CollisionComponent(shapes: [.generateBox(size: SIMD3(max(size.x, 0.2), 0.3, max(size.z, 0.2)))]))
        tiles[e.level].addChild(tile)
        roomTileEntities[e.sourceID] = tile
        if raise[e.sourceID] == nil { raise[e.sourceID] = (0, 0, 0) }
    }

    /// The Architectural view of one level from the architectural package: a few merged meshes by what they are.
    fileprivate func buildSetArch(_ i: Int) {
        var slab = MeshKit(), wall = MeshKit(), pane = MeshKit(), door = MeshKit(), stair = MeshKit()
        for e in b.elements where e.level == i && tier(e) == .architecture {
            guard case let .mesh(m) = e.shape else { continue }
            switch e.part {
            case .exterior, .roomTile, .outline: continue
            case .slab: slab.append(m)
            case .window: pane.append(m)
            case .door: door.append(m)
            case .stair: stair.append(m)
            default: wall.append(m)
            }
        }
        let mats: [(MeshKit, RealityKit.Material, String)] = [
            (slab, pbr("arch-slab", style.isDark ? UIColor(hex: 0xC9CDCB) : UIColor(hex: 0xD9DDE1), rough: 0.9), "slab"),
            (wall, pbr("facade", style.facade, rough: 0.82), "wall"),
            (pane, pbr("glass", style.glass, rough: 0.12, metal: 0.4), "glass"),
            (door, pbr("arch-door", UIColor(hex: 0x8A6A4E), rough: 0.7), "door"),
            (stair, pbr("arch-stair", UIColor(hex: 0xB7BBBE), rough: 0.85), "stair"),
        ]
        for (mk, m, name) in mats { if let r = mk.resource("arch-\(name)-\(i)") { arch[i].addChild(ModelEntity(mesh: r, materials: [m])) } }
    }

    /// A room from its own package: its floor solid, its walls (clipped to the room by the export) see-through so
    /// the camera reads the inside, and every object of the package, one per work item with a short fade.
    fileprivate func buildSetRoom(_ number: String) {
        guard let r = b.room(number), levelRoots.indices.contains(r.level) else { return }
        let root = Entity()
        root.name = "room-\(number)"
        levelRoots[r.level].addChild(root)
        let items = b.elements(in: number)
        var walls = MeshKit(), fl = MeshKit()
        for e in items where isShell(e) {
            guard case let .mesh(m) = e.shape else { continue }
            if e.part == .slab { fl.append(m) } else if e.part != .roomTile { walls.append(m) }
        }
        var wallM = UnlitMaterial(color: style.isDark ? UIColor(hex: 0xE9E6DF) : UIColor(hex: 0xC9CCCF))
        wallM.blending = .transparent(opacity: .init(floatLiteral: style.highContrast ? 0.5 : 0.34))
        wallM.writesDepth = false
        if let res = walls.resource("room-walls-\(number)") { root.addChild(ModelEntity(mesh: res, materials: [wallM])) }
        if let res = fl.resource("room-floor-\(number)") { root.addChild(ModelEntity(mesh: res, materials: [pbr("roomfloor", UIColor(hex: 0xC8B9A3), rough: 0.8)])) }
        for (k, el) in items.filter({ !isShell($0) }).enumerated() {
            enqueue { [weak self, weak root] in
                guard let self, let root, root.parent != nil, let ent = self.addElement(el, home: root) else { return }
                if !self.reduceMotion { self.fade(ent, from: 0, to: 1, delay: 0.05 + Double(min(k, 40)) * 0.012) }
            }
        }
        if !reduceMotion { fade(root, from: 0, to: 1, delay: 0) }
        roomDetail = root
        roomDetailNumber = number
        roomDetailPackage = items.first?.package
    }

    /// The experience loaded or released packages: drop what was released (its entities and the containers
    /// built from it); what arrived is built by the next scope change.
    func packagesChanged(released: Set<String>) {
        guard isSet else { return }
        levelBounds.removeAll()
        limitKey = ""
        guard !released.isEmpty else { return }
        apply(focus: nil)
        if let pk = roomDetailPackage, released.contains(pk) { removeRoomDetail() }
        var gone: [String] = []
        for (id, ent) in elementEntities {
            guard let pk = packageOf[id], released.contains(pk) else { continue }
            ent.removeFromParent()
            gone.append(id)
        }
        for id in gone { elementEntities[id] = nil; edgeEntities[id] = nil; fogStep[id] = nil; packageOf[id] = nil }
        for i in levelRoots.indices {
            guard let lv = exp?.levelPackage(i), released.contains(lv) else { continue }
            for c in [cut[i], tiles[i], detail[i]] {
                for ch in Array(c.children) { forgetGroups(under: ch); ch.removeFromParent() }
                forgetGroups(under: c)
            }
            cutBuilt[i] = false
            detailBuilt[i] = false
            for (k, _) in roomTileEntities where b.room(k)?.level == i { roomTileEntities[k] = nil }
        }
    }
}

final class PaddedLabel: UILabel {
    override func drawText(in rect: CGRect) { super.drawText(in: rect.insetBy(dx: 7, dy: 3)) }
    override var intrinsicContentSize: CGSize { let s = super.intrinsicContentSize; return CGSize(width: s.width + 14, height: s.height + 6) }
    override func sizeThatFits(_ size: CGSize) -> CGSize { let s = super.sizeThatFits(size); return CGSize(width: s.width + 14, height: s.height + 6) }
}
