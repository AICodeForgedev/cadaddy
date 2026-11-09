// CADaddy Field — the building / floor / room scene. One RealityKit scene and one camera from building to floor to
// room; nothing cuts between screens. The architecture is a pale ghost; the systems are the hero.
//
// Transparency without flicker: every ghost face uses ONE material (one colour, one opacity) that reads depth but
// never writes it. Blending n layers of the same colour and opacity gives the same result in any order, so the
// renderer's per-object sort can't make ghost faces flicker or swap. Ghost hairlines, systems and equipment are
// opaque and depth-tested as usual. Fading the rest of the building back during "follow the system" swaps to a
// second shared see-through material for the same reason.

import Combine
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

    private let anchor = AnchorEntity(world: .zero)
    private let camera = PerspectiveCamera()
    private var sub: Cancellable?
    let labelLayer = UIView()

    // Per level
    private var levelRoots: [Entity] = []
    private var ghost: [Entity] = []
    private var cut: [Entity] = []
    private var mains: [Entity] = []
    private var detail: [Entity] = []
    private var detailBuilt: [Bool] = []
    private var tiles: [Entity] = []
    private var lift: [Float] = [], liftVel: [Float] = [], liftGoal: [Float] = []
    private var exterior: [(entity: Entity, x: Float)] = []
    private var ground = Entity()
    private var roomDetail: Entity?
    private var roomDetailNumber: String?

    private var elementEntities: [String: Entity] = [:]
    private var roomTileEntities: [String: ModelEntity] = [:]
    private var raise: [String: (y: Float, v: Float, goal: Float)] = [:]
    private var risers: Set<String> = []
    private var roomServing: Set<String> = []

    // Fades: OpacityComponent driven by a spring; disabled at 0.
    private struct Fade { weak var e: Entity?; var value: Float; var vel: Float; var goal: Float; var delay: Double }
    private var fades: [ObjectIdentifier: Fade] = [:]

    // Materials
    private var ghostMat: RealityKit.Material = UnlitMaterial()
    private var edgeMat: RealityKit.Material = UnlitMaterial()
    private var fadedMat: RealityKit.Material = UnlitMaterial()
    private var matCache: [String: RealityKit.Material] = [:]
    private var originals: [ObjectIdentifier: [RealityKit.Material]] = [:]
    private var glowTexture: TextureResource?
    private var shadowTexture: TextureResource?
    private var restyleNeeded = false

    // Camera: target and (yaw, pitch, distance), each on the design spring.
    private var target = SIMD3<Float>(37, 4, 9), targetVel = SIMD3<Float>.zero, targetGoal = SIMD3<Float>(37, 4, 9)
    private var orbit = SIMD3<Float>(-2.25, 0.56, 150), orbitVel = SIMD3<Float>.zero, orbitGoal = SIMD3<Float>(-2.25, 0.56, 150)
    private var panStart = SIMD3<Float>.zero
    private var pinchStart: Float = 1
    private let spring = Spring.design
    var autoOrbitSpeed: Float = 0

    // Follow the system
    private var pulse: [ModelEntity] = []
    private var pulsePath: [(SIMD3<Float>, Int)] = []
    private var pulseT: Double = 0
    private var litIDs: Set<String> = []

    // Labels
    private var labels: [String: UILabel] = [:]
    private var labelAnchors: [String: (SIMD3<Float>, Int)] = [:]

    // Frame rate
    private var frames = 0
    private var fpsStart = Date()
    private var fpsLog: [Double] = []
    private var logPerf = false
    private var lastTick = Date()

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
        camera.camera.fieldOfViewInDegrees = 42
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
        shadowTexture = MeshKit.radialTexture(inner: UIColor.black.withAlphaComponent(style.isDark ? 0.55 : 0.22), outer: UIColor.black.withAlphaComponent(0))
    }

    private func pbr(_ key: String, _ c: UIColor, rough: Float = 0.5, metal: Float = 0, emissive: Float = 0) -> RealityKit.Material {
        if let m = matCache[key] { return m }
        var m = PhysicallyBasedMaterial()
        m.baseColor = .init(tint: c)
        m.roughness = .init(floatLiteral: rough)
        m.metallic = .init(floatLiteral: metal)
        if emissive > 0 {
            m.emissiveColor = .init(color: c)
            m.emissiveIntensity = emissive
        }
        matCache[key] = m
        return m
    }

    private func material(for e: BuildingElement, lit: Bool = false) -> RealityKit.Material {
        let boost: Float = lit ? 0.9 : (style.highContrast ? 0.35 : 0.12)
        switch e.role {
        case .equipment:
            return pbr("eq-\(e.system)-\(lit)", lit ? style.color(e.system) : style.equipment(e.system), rough: 0.55, emissive: lit ? 0.5 : 0.04)
        case .main:
            return pbr("main-\(e.system)-\(lit)", style.color(e.system), rough: 0.42, emissive: boost)
        case .branch:
            return pbr("br-\(e.system)-\(lit)", lit ? style.color(e.system) : style.branch(e.system), rough: 0.5, emissive: lit ? 0.9 : 0.08)
        case .terminal:
            if e.kind == "Luminaire" { return UnlitMaterial(color: style.lightWarm) }
            return pbr("tm-\(e.system)-\(lit)", style.color(e.system), rough: 0.45, emissive: lit ? 0.9 : boost)
        case .fixture:
            return pbr("fx-\(lit)", lit ? style.color(.plumbing) : UIColor(hex: 0xF3F5F4).mixed(with: style.color(.plumbing), 0.08), rough: 0.25, emissive: lit ? 0.6 : 0)
        case .casework:
            return pbr("cw-\(e.kind)", e.kind == "Vanity" ? UIColor(hex: 0xEDEBE6) : UIColor(hex: 0xB89670), rough: 0.7)
        case .furniture:
            let c: UIColor
            switch e.kind {
            case "Bed": c = UIColor(hex: 0xF4F2EE)
            case "Headboard", "Desk", "Nightstand": c = UIColor(hex: 0x7C5E46)
            case "Chair": c = UIColor(hex: 0x3B4248)
            default: c = UIColor(hex: 0x9AA6A0)
            }
            return pbr("fu-\(e.kind)", c, rough: 0.85)
        case .shell:
            return ghostMat
        }
    }

    // MARK: static scene (shell, mains, exterior)

    private func buildStatic() {
        let n = b.levels.count
        levelRoots = (0..<n).map { _ in Entity() }
        ghost = (0..<n).map { _ in Entity() }
        cut = (0..<n).map { _ in Entity() }
        mains = (0..<n).map { _ in Entity() }
        detail = (0..<n).map { _ in Entity() }
        tiles = (0..<n).map { _ in Entity() }
        detailBuilt = Array(repeating: false, count: n)
        lift = Array(repeating: 0, count: n); liftVel = lift; liftGoal = lift
        for i in 0..<n {
            let root = levelRoots[i]
            root.name = "level-\(i)"
            anchor.addChild(root)
            for c in [ghost[i], cut[i], tiles[i], mains[i], detail[i]] { root.addChild(c) }
            cut[i].isEnabled = false
            tiles[i].isEnabled = false
            buildShell(i)
        }
        for e in b.elements where e.detail == .building { addElement(e, into: mains[e.level]) }
        for e in b.elements where e.detail == .building {
            let bb = e.shape.bounds
            if bb.max.y - bb.min.y > 5 { risers.insert(e.id) }
        }
        buildGround()
        buildExterior()
        buildPulse()
    }

    private func buildShell(_ i: Int) {
        guard let lv = b.level(i) else { return }
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
        if i == 5 {
            // Parapet as a ghost too.
            for (c, s) in [(SIMD3<Float>(28, lv.elevation + 0.45, 0.15), SIMD3<Float>(56, 0.9, 0.3)), (SIMD3<Float>(28, lv.elevation + 0.45, 17.85), SIMD3<Float>(56, 0.9, 0.3))] {
                faces.box(center: c, size: s); edges.edges(center: c, size: s, width: w)
            }
        }
        if let m = faces.resource("ghost-\(i)") { ghost[i].addChild(ModelEntity(mesh: m, materials: [ghostMat])) }
        if let m = edges.resource("edges-\(i)") { ghost[i].addChild(ModelEntity(mesh: m, materials: [edgeMat])) }
        // The cut view: walls at 4 ft, every wall gets its hairlines, a soft floor plate.
        guard lv.height > 0 || i == 5 else { return }
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
            raise[r.number] = (0, 0, 0)
        }
    }

    private func buildGround() {
        let lo = b.footprintMin, hi = b.footprintMax
        if let q = MeshKit.quad(center: SIMD3((lo.x + hi.x) / 2 - 2, -0.02, (lo.y + hi.y) / 2), size: SIMD2(hi.x - lo.x + 34, hi.y - lo.y + 30)),
           let t = shadowTexture {
            var m = UnlitMaterial()
            m.color = .init(tint: .white, texture: .init(t))
            m.blending = .transparent(opacity: .init(floatLiteral: 1))
            m.writesDepth = false
            ground = ModelEntity(mesh: q, materials: [m])
            anchor.addChild(ground)
        }
    }

    /// The solid exterior for the opening reveal: one merged piece per bay, swept from solid to clear.
    private func buildExterior() {
        let lo = b.footprintMin, hi = b.footprintMax
        let bays = Int(((hi.x - lo.x) / b.bay).rounded())
        let wall = pbr("facade", style.facade, rough: 0.82)
        let base = pbr("facade-dark", style.facadeDark, rough: 0.9)
        let glass = pbr("glass", style.glass, rough: 0.12, metal: 0.4)
        let t: Float = 0.32
        for k in 0..<bays {
            let x0 = lo.x + Float(k) * b.bay, x1 = x0 + b.bay
            var solid = MeshKit(), dark = MeshKit(), pane = MeshKit()
            for i in 1...4 {
                guard let lv = b.level(i) else { continue }
                let e = lv.elevation, h = lv.height
                for (z, out) in [(lo.y, Float(-1)), (hi.y, Float(1))] {
                    let zc = z - out * t / 2
                    let storefront = i == 1 && out < 0 && x1 <= 28
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
            let roof = b.level(5)?.elevation ?? 14.4
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

    // MARK: elements

    @discardableResult
    private func addElement(_ e: BuildingElement, into parent: Entity) -> Entity? {
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
        if case .run = e.shape {
            // Runs: a box per segment so a tap near the line hits it (not the run's whole bounding box).
            if case let .run(points, _) = e.shape, points.count >= 2 {
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
        return m
    }

    /// The floor's branches, lights and terminals — built the first time the floor is opened.
    private func ensureDetail(_ level: Int) {
        guard detail.indices.contains(level), !detailBuilt[level] else { return }
        detailBuilt[level] = true
        let items = b.elements.filter { $0.level == level && $0.detail == .floor }
        for (k, e) in items.enumerated() {
            guard let ent = addElement(e, into: detail[level]) else { continue }
            if !reduceMotion { fade(ent, from: 0, to: 1, delay: min(Double(k) * 0.003, 0.35)) }
        }
    }

    private func buildRoom(_ number: String) {
        if roomDetailNumber == number { return }
        roomDetail?.removeFromParent()
        roomDetail = nil
        roomDetailNumber = nil
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
        let bz = south ? r.max.y - 2.4 : r.min.y + 2.4
        let near2 = south ? camDir.y < 0 : camDir.y > 0
        walls.box(min: SIMD3(r.min.x, e, bz - 0.05), max: SIMD3(r.min.x + 2.4, e + (near2 ? 0.9 : h), bz + 0.05))
        walls.box(min: SIMD3(r.min.x + 2.35, e, min(bz, south ? r.max.y : r.min.y)), max: SIMD3(r.min.x + 2.45, e + 0.9, max(bz, south ? r.max.y : r.min.y)))
        bath.box(min: SIMD3(r.min.x + t, e + 0.06, min(bz, south ? r.max.y : r.min.y)), max: SIMD3(r.min.x + 2.35, e + 0.065, max(bz, south ? r.max.y : r.min.y)))
        for (mk, m) in [(walls, wallM), (fl, floorM), (bath, bathM)] {
            if let res = mk.resource("room-\(number)") { root.addChild(ModelEntity(mesh: res, materials: [m])) }
        }
        let items = b.elements(in: number).filter { $0.detail == .room }
        for (k, el) in items.enumerated() {
            guard let ent = addElement(el, into: root) else { continue }
            if !layersShow(el) { ent.isEnabled = false; continue }
            if !reduceMotion { fade(ent, from: 0, to: 1, delay: 0.12 + Double(k) * 0.012) }
        }
        if !reduceMotion { fade(root, from: 0, to: 1, delay: 0) }
        roomDetail = root
        roomDetailNumber = number
    }

    private func dropRoom() {
        guard let r = roomDetail else { return }
        for el in b.elements(in: roomDetailNumber ?? "") where el.detail == .room { elementEntities[el.id] = nil }
        r.removeFromParent()
        roomDetail = nil
        roomDetailNumber = nil
    }

    // MARK: scope changes

    var reduceMotion: Bool { UIAccessibility.isReduceMotionEnabled }

    func apply(scope: DesignScope, liftFocus: Float?) {
        guard let exp else { return }
        let n = b.levels.count
        switch scope {
        case .building:
            dropRoom()
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
            targetGoal = SIMD3(33, liftFocus == nil ? 3.0 : 8 + (liftFocus ?? 0) * 1.4, 9)
            orbitGoal.y = liftFocus == nil ? 0.56 : 0.38
            orbitGoal.z = liftFocus == nil ? 150 : 165
            for (number, _) in raise { raise[number]?.goal = 0 }
        case let .floor(i):
            dropRoom()
            showFloor(i)
            let lv = b.level(i)
            let e = lv?.elevation ?? 0
            targetGoal = i == 0 ? SIMD3(20, -2.5, 11) : SIMD3(28, e - 3.2, 9)
            orbitGoal.y = 0.74
            orbitGoal.z = i == 0 || i == 5 ? 84 : 80
            for (number, _) in raise { raise[number]?.goal = 0 }
        case let .room(number):
            guard let r = b.room(number) else { break }
            showFloor(r.level)
            let south = r.max.y <= b.footprintMax.y / 2
            // Look in from the outside of the room's long open side.
            let yaw: Float = south ? -2.55 : -0.62
            orbitGoal.x = nearestAngle(yaw, to: orbit.x)
            orbitGoal.y = 0.92
            orbitGoal.z = 33
            let e = b.level(r.level)?.elevation ?? 0
            targetGoal = SIMD3(r.center.x, e - 2.9, r.center.y)
            buildRoom(number)
            for (k, _) in raise { raise[k]?.goal = k == number ? 0.18 : 0 }
            for (k, t) in roomTileEntities where b.room(k)?.level == r.level { setOpacity(t, k == number ? 1 : 0.35) }
            // The rest of the floor steps back: only what serves this room stays, so nothing blocks the view in.
            var serving = Set<String>()
            for el in b.elements(in: number) { if let u = el.upstream { serving.insert(u) } }
            roomServing = serving
            for el in b.elements where el.room != number && (el.detail == .floor || el.detail == .building) && (el.level == r.level || risers.contains(el.id)) {
                guard let ent = elementEntities[el.id] else { continue }
                setOpacity(ent, serving.contains(el.id) && layersShow(el) ? 1 : 0)
            }
            for el in b.elements(in: number) { if let ent = elementEntities[el.id], layersShow(el) { setOpacity(ent, 1) } }
        }
        if reduceMotion { snapCamera() }
        _ = exp
        applyLayers()
        refreshLabels(scope)
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
                for el in b.elements where el.level == j && (el.detail == .floor || el.detail == .building) {
                    if let ent = elementEntities[el.id], layersShow(el) { setOpacity(ent, 1) }
                }
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

    // MARK: layers

    private func layersShow(_ e: BuildingElement) -> Bool { exp?.layers.shows(e.system) ?? true }

    func applyLayers() {
        guard let exp else { return }
        for (id, ent) in elementEntities {
            guard let e = b.element(id) else { continue }
            let on = exp.layers.shows(e.system)
            if !on { setOpacity(ent, 0) } else if (fades[ObjectIdentifier(ent)]?.goal ?? 1) == 0 || !ent.isEnabled {
                if isInCurrentScope(e) { setOpacity(ent, 1) }
            }
        }
    }

    private func isInCurrentScope(_ e: BuildingElement) -> Bool {
        guard let exp else { return true }
        switch exp.scope {
        case .building: return e.detail == .building
        case let .floor(i): return e.level == i
        case let .room(n): return e.room == n || roomServing.contains(e.id)
        }
    }

    // MARK: follow the system

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
        guard let focus else { return }
        litIDs = focus.lit
        for (id, ent) in elementEntities {
            guard let m = ent as? ModelEntity, let e = b.element(id) else { continue }
            let key = ObjectIdentifier(m)
            originals[key] = m.model?.materials
            entityByObject[key] = m
            // Furniture and casework are context in a room, not part of any system: they stay as they are.
            if !focus.lit.contains(id), e.role == .furniture || e.role == .casework { originals[key] = nil; entityByObject[key] = nil; continue }
            m.model?.materials = focus.lit.contains(id) ? [material(for: e, lit: true)] : [fadedMat]
            if focus.lit.contains(id), focus.lit.count < 400, e.role != .equipment || id == focus.tapped { addHalo(e, to: m) }
            for case let child as ModelEntity in m.children where e.kind == "Luminaire" && !focus.lit.contains(id) {
                child.isEnabled = false
                hiddenGlows.append(child)
            }
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

    /// Flies the one camera to an element, opening its floor if it is on another one.
    func frame(element id: String) {
        guard let e = b.element(id) else { return }
        let lift = liftGoal.indices.contains(e.level) ? liftGoal[e.level] : 0
        let bb = e.shape.bounds
        let span = simd_length(bb.max - bb.min)
        // Enough room around it to read what it connects to; the bottom sheet covers the lower part of the screen.
        orbitGoal.z = e.detail == .room ? 16 : min(max(span * 1.2 + 30, 40), 90)
        orbitGoal.y = 0.62
        targetGoal = e.shape.anchor + SIMD3(0, lift - orbitGoal.z * 0.1, 0)
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
            let top = v.project(SIMD3(front, 16, b.footprintMin.y)), bottom = v.project(SIMD3(front, 0, b.footprintMin.y))
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
        for r in levelRoots { r.removeFromParent() }
        for e in exterior { e.entity.removeFromParent() }
        exterior = []
        ground.removeFromParent()
        pulse.forEach { $0.removeFromParent() }
        pulse = []
        elementEntities.removeAll(); roomTileEntities.removeAll(); raise.removeAll(); fades.removeAll()
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
        let dt = min(max(dt, 0), 0.1)
        if autoOrbitSpeed != 0 { orbitGoal.x += autoOrbitSpeed * Float(dt) }
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
        exp?.tick(dt)
        measure()
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

    private func measure() {
        frames += 1
        let now = Date()
        let span = now.timeIntervalSince(fpsStart)
        guard span >= 1 else { return }
        let fps = Double(frames) / span
        exp?.fps = fps
        frames = 0
        fpsStart = now
        #if DEBUG
        if logPerf {
            fpsLog.append(fps)
            let ents = elementEntities.count
            print(String(format: "FIELD-PERF design fps %.1f scope %@ elements %d", fps, String(describing: exp?.scope ?? .building), ents))
        }
        #endif
    }

    // MARK: labels

    func refreshLabels(_ scope: DesignScope) {
        var want: [String: (String, SIMD3<Float>, Int, Bool)] = [:]   // key → text, point, level, mono
        switch scope {
        case .building:
            for e in b.elements where e.role == .equipment && e.detail == .building && e.kind != "Panelboard" {
                want["e-\(e.id)"] = (e.tag, e.shape.bounds.max * SIMD3(0, 1, 0) + e.shape.anchor * SIMD3(1, 0, 1) + SIMD3(0, 0.6, 0), e.level, true)
            }
        case let .floor(i):
            for r in b.rooms(on: i) where r.name != "Corridor" {
                let text = r.name == "Guest Room" ? r.number : r.name
                want["r-\(r.number)"] = (text, SIMD3(r.center.x, (b.level(i)?.elevation ?? 0) + 0.1, r.center.y), i, r.name == "Guest Room")
            }
            for e in b.elements where e.level == i && e.role == .equipment {
                want["e-\(e.id)"] = (e.tag, e.shape.anchor + SIMD3(0, 1.1, 0), e.level, true)
            }
        case let .room(n):
            for e in b.elements(in: n) where (e.role == .terminal || e.role == .fixture) && layersShow(e) {
                want["d-\(e.id)"] = (e.tag, e.shape.bounds.max * SIMD3(0, 1, 0) + e.shape.anchor * SIMD3(1, 0, 1) + SIMD3(0, 0.12, 0), e.level, true)
            }
        }
        for (k, l) in labels where want[k] == nil {
            UIView.animate(withDuration: reduceMotion ? 0 : 0.2, animations: { l.alpha = 0 }, completion: { _ in l.removeFromSuperview() })
            labels[k] = nil
            labelAnchors[k] = nil
        }
        for (k, (text, p, lv, mono)) in want {
            labelAnchors[k] = (p, lv)
            guard labels[k] == nil else { continue }
            let l = PaddedLabel()
            l.text = text
            let big = k.hasPrefix("r-") && !mono
            l.font = mono ? .monospacedSystemFont(ofSize: k.hasPrefix("d-") ? 11 : 12, weight: .semibold) : .systemFont(ofSize: big ? 11 : 12, weight: .semibold)
            l.textColor = style.isDark ? .white : UIColor(hex: 0x0E1A24)
            l.backgroundColor = (style.isDark ? UIColor(hex: 0x0C1A19) : .white).withAlphaComponent(style.highContrast ? 0.92 : 0.62)
            l.layer.cornerRadius = 7
            l.layer.borderWidth = style.highContrast ? 1 : 0.5
            l.layer.borderColor = (style.isDark ? UIColor.white : UIColor.black).withAlphaComponent(style.highContrast ? 0.6 : 0.12).cgColor
            l.clipsToBounds = true
            l.sizeToFit()
            l.alpha = 0
            labelLayer.addSubview(l)
            labels[k] = l
            UIView.animate(withDuration: reduceMotion ? 0 : 0.35, delay: reduceMotion ? 0 : 0.25) { l.alpha = 1 }
        }
    }

    private func positionLabels() {
        guard let v = view else { return }
        for (k, l) in labels {
            guard let (p, lv) = labelAnchors[k] else { continue }
            let y = lift.indices.contains(lv) ? lift[lv] : 0
            guard let s = v.project(p + SIMD3(0, y, 0)), v.bounds.insetBy(dx: -40, dy: -40).contains(s) else { l.isHidden = true; continue }
            l.isHidden = false
            l.center = s
        }
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
        if g.state == .began { panStart = orbitGoal }
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
        if g.state == .began { pinchStart = orbitGoal.z }
        orbitGoal.z = min(max(pinchStart / Float(max(g.scale, 0.05)), 4), 220)
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
        for lv in b.levels where lv.index >= 1 && lv.index <= 4 {
            let y = lv.elevation + lv.height / 2 + (lift.indices.contains(lv.index) ? lift[lv.index] : 0)
            for x in stride(from: Float(0), through: 56, by: 4) {
                for z in [Float(0), 9, 18] {
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
