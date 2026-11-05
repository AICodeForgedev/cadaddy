import Combine
import RealityKit
import simd
import UIKit

/// Tags an entity as an exported element (index into FieldModel.elements).
struct ElementComponent: Component {
    var index: Int
}

/// Builds RealityKit entities from a parsed package, progressively, so large packages appear in batches
/// instead of freezing the screen. Meshes are created once and shared by every element that uses them.
@MainActor
final class SceneBuilder {
    /// The model root: its transform is the alignment (world from model).
    let root = Entity()
    private(set) var elementEntities: [Entity?] = []
    private var meshCache: [Int: (MeshResource, [RealityKit.Material])] = [:]
    private var highlight: ModelEntity?
    private var labels: [Int: Entity] = [:]
    let labelRoot = Entity()
    private(set) var cancelled = false

    func cancel() { cancelled = true }

    /// Builds the GLB model. `progress` gets 0…1. Returns warnings (meshes that could not be made).
    func build(_ model: FieldModel, batch: Int = 150, progress: (Double) -> Void) async -> [String] {
        root.name = "CADaddyModel"
        elementEntities = Array(repeating: nil, count: model.elements.count)
        var warnings: [String] = []
        let total = max(model.elements.count, 1)
        // Devices first, the building shell last: the useful part shows up first on big packages.
        let order = model.elements.indices.sorted { (model.elements[$0].info.isContext ? 1 : 0) < (model.elements[$1].info.isContext ? 1 : 0) }
        var done = 0
        for i in order {
            if cancelled { return warnings }
            let e = model.elements[i]
            let ent = Entity()
            ent.name = e.name
            ent.transform = Transform(matrix: e.transform)
            ent.components.set(ElementComponent(index: i))
            var localMin = SIMD3<Float>(repeating: .greatestFiniteMagnitude), localMax = -localMin
            for part in e.parts {
                guard let (mesh, mats) = resource(model, part.mesh, warnings: &warnings) else { continue }
                let child = ModelEntity(mesh: mesh, materials: mats)
                child.transform = Transform(matrix: part.transform)
                ent.addChild(child)
                if model.meshes.indices.contains(part.mesh) {
                    let m = model.meshes[part.mesh]
                    let (a, b) = GLBReader.Parser.transformBounds(min: m.boundsMin, max: m.boundsMax, by: part.transform)
                    localMin = simd_min(localMin, a); localMax = simd_max(localMax, b)
                }
            }
            if !e.info.isContext, localMin.x <= localMax.x {
                // Tap target: the element's box, at least 6 cm each way so small devices are easy to hit.
                let size = simd_max(localMax - localMin, SIMD3(repeating: 0.06))
                let shape = ShapeResource.generateBox(size: size).offsetBy(translation: (localMin + localMax) / 2)
                ent.components.set(CollisionComponent(shapes: [shape]))
            }
            root.addChild(ent)
            elementEntities[i] = ent
            done += 1
            if done % batch == 0 {
                progress(Double(done) / Double(total))
                await Task.yield()
                try? await Task.sleep(nanoseconds: 1_000_000)
            }
        }
        progress(1)
        return warnings
    }

    /// Loads a USDZ with RealityKit and derives elements from its named entities, filling metadata from
    /// sidecar rows where names match.
    func buildUSDZ(url: URL, sidecar: Sidecar) async throws -> FieldModel {
        let loaded: Entity
        do {
            if #available(iOS 18.0, *) { loaded = try await Entity(contentsOf: url) } else { loaded = try Self.loadBlocking(url) }
        } catch { throw FieldError("This USDZ could not be opened: \(error.localizedDescription)") }
        root.addChild(loaded)
        var elements: [FieldElement] = []
        var entities: [Entity?] = []
        func visit(_ e: Entity, depth: Int) {
            guard depth < FieldLimits.maxNodeDepth, elements.count < FieldLimits.maxNodes else { return }
            if e is ModelEntity || e.components.has(ModelComponent.self), !e.name.isEmpty {
                // The element is the named entity holding the geometry (or its named parent for an unnamed mesh).
                let owner = (e.name.isEmpty ? e.parent : e) ?? e
                let info = ElementInfo.fromNodeName(owner.name)
                let b = owner.visualBounds(relativeTo: root)
                let idx = elements.count
                elements.append(FieldElement(index: idx, name: owner.name, info: info, transform: owner.transformMatrix(relativeTo: root), parts: [],
                                             boundsMin: b.isEmpty ? nil : b.min, boundsMax: b.isEmpty ? nil : b.max))
                owner.components.set(ElementComponent(index: idx))
                let lb = owner.visualBounds(relativeTo: owner)
                if !lb.isEmpty {
                    owner.components.set(CollisionComponent(shapes: [ShapeResource.generateBox(size: simd_max(lb.extents, SIMD3(repeating: 0.06))).offsetBy(translation: lb.center)]))
                }
                entities.append(owner)
                return
            }
            for c in e.children { visit(c, depth: depth + 1) }
        }
        visit(loaded, depth: 0)
        var model = FieldModel(meshes: [], elements: elements)
        model.merge(sidecar: sidecar)
        elementEntities = entities
        return model
    }

    /// iOS 17 has no async entity loader.
    private static func loadBlocking(_ url: URL) throws -> Entity { try Entity.load(contentsOf: url) }

    private func resource(_ model: FieldModel, _ index: Int, warnings: inout [String]) -> (MeshResource, [RealityKit.Material])? {
        if let r = meshCache[index] { return r }
        guard model.meshes.indices.contains(index) else { return nil }
        let m = model.meshes[index]
        var descriptors: [MeshDescriptor] = []
        var materials: [RealityKit.Material] = []
        for (pi, p) in m.primitives.enumerated() {
            var d = MeshDescriptor(name: "\(m.name)-\(pi)")
            d.positions = MeshBuffer(p.positions)
            if p.normals.count == p.positions.count { d.normals = MeshBuffer(p.normals) }
            d.primitives = .triangles(p.indices)
            d.materials = .allFaces(UInt32(pi))
            descriptors.append(d)
            materials.append(Self.material(p.color))
        }
        do {
            let mesh = try MeshResource.generate(from: descriptors)
            meshCache[index] = (mesh, materials)
            return (mesh, materials)
        } catch {
            warnings.append("Mesh \(m.name) could not be built.")
            return nil
        }
    }

    static func material(_ c: SIMD4<Float>) -> RealityKit.Material {
        var mat = PhysicallyBasedMaterial()
        mat.baseColor = .init(tint: UIColor(red: CGFloat(c.x), green: CGFloat(c.y), blue: CGFloat(c.z), alpha: 1))
        mat.roughness = .init(floatLiteral: 0.8)
        mat.metallic = .init(floatLiteral: 0)
        if c.w < 0.999 { mat.blending = .transparent(opacity: .init(floatLiteral: c.w)) }
        return mat
    }

    // MARK: state

    func applyVisibility(_ session: FieldSession) {
        guard let model = session.model else { return }
        for (i, ent) in elementEntities.enumerated() {
            guard let ent, model.elements.indices.contains(i) else { continue }
            let e = model.elements[i]
            ent.isEnabled = session.isVisible(e)
            if e.info.isContext { setTranslucent(ent, session.wallMode == .translucent) }
        }
    }

    private var originalMaterials: [ObjectIdentifier: [RealityKit.Material]] = [:]

    /// Translucent context (walls, floors, ceilings) by material swap — OpacityComponent needs iOS 18.
    private func setTranslucent(_ ent: Entity, _ on: Bool) {
        for case let child as ModelEntity in ent.children {
            guard var model = child.model else { continue }
            let key = ObjectIdentifier(child)
            if on {
                if originalMaterials[key] == nil { originalMaterials[key] = model.materials }
                model.materials = (originalMaterials[key] ?? model.materials).map { m in
                    guard var pbr = m as? PhysicallyBasedMaterial else { return m }
                    if case .opaque = pbr.blending { pbr.blending = .transparent(opacity: .init(floatLiteral: 0.3)) }
                    return pbr
                }
            } else if let orig = originalMaterials[key] {
                model.materials = orig
            } else {
                continue
            }
            child.model = model
        }
    }

    func select(_ index: Int?, model: FieldModel?) {
        highlight?.removeFromParent()
        highlight = nil
        guard let index, elementEntities.indices.contains(index), let ent = elementEntities[index] else { return }
        let b = ent.visualBounds(relativeTo: ent)
        guard !b.isEmpty else { return }
        var mat = UnlitMaterial(color: UIColor.systemCyan)
        mat.blending = .transparent(opacity: 0.45)
        let box = ModelEntity(mesh: .generateBox(size: b.extents + SIMD3(repeating: 0.03)), materials: [mat])
        box.position = b.center
        ent.addChild(box)
        highlight = box
    }

    func entity(for index: Int) -> Entity? { elementEntities.indices.contains(index) ? elementEntities[index] : nil }

    /// World-space centre and node origin of an element as currently placed.
    func worldPoints(_ index: Int) -> (center: SIMD3<Float>, origin: SIMD3<Float>)? {
        guard let ent = entity(for: index) else { return nil }
        let b = ent.visualBounds(relativeTo: nil)
        return (b.isEmpty ? ent.position(relativeTo: nil) : b.center, ent.position(relativeTo: nil))
    }

    // MARK: height labels

    /// Shows mounting-height labels over the visible non-context elements (capped), or removes them.
    func updateLabels(_ session: FieldSession, cap: Int = 300) {
        guard session.showHeights, let model = session.model else {
            labels.values.forEach { $0.removeFromParent() }; labels.removeAll(); return
        }
        var wanted = Set<Int>()
        for e in model.elements where !e.info.isContext && session.isVisible(e) && e.info.mountingHeight != nil {
            if wanted.count >= cap { break }
            wanted.insert(e.index)
        }
        for (i, l) in labels where !wanted.contains(i) { l.removeFromParent(); labels.removeValue(forKey: i) }
        for i in wanted where labels[i] == nil {
            guard model.elements.indices.contains(i), let h = model.elements[i].info.mountingHeight else { continue }
            labels[i] = makeLabel("\(model.elements[i].info.tag ?? "") \(Units.feetInches(h))".trimmingCharacters(in: .whitespaces))
            if let l = labels[i] { labelRoot.addChild(l) }
        }
        positionLabels()
    }

    func positionLabels() {
        for (i, l) in labels {
            guard let ent = entity(for: i), ent.isEnabled else { l.isEnabled = false; continue }
            l.isEnabled = true
            let b = ent.visualBounds(relativeTo: nil)
            let top = b.isEmpty ? ent.position(relativeTo: nil) : SIMD3(b.center.x, b.max.y, b.center.z)
            l.setPosition(top + SIMD3(0, 0.06, 0), relativeTo: nil)
        }
    }

    /// Turns labels to face the camera.
    func faceLabels(camera: SIMD3<Float>) {
        for l in labels.values where l.isEnabled {
            let p = l.position(relativeTo: nil)
            var away = p - camera
            away.y = 0
            guard simd_length(away) > 1e-3 else { continue }
            l.look(at: p + away, from: p, relativeTo: nil)
        }
    }

    private func makeLabel(_ text: String) -> Entity {
        let mesh = MeshResource.generateText(text, extrusionDepth: 0.001, font: .systemFont(ofSize: 0.04, weight: .semibold),
                                             containerFrame: .zero, alignment: .center, lineBreakMode: .byTruncatingTail)
        let t = ModelEntity(mesh: mesh, materials: [UnlitMaterial(color: .white)])
        let b = t.visualBounds(relativeTo: t)
        t.position = SIMD3(-b.center.x, -b.center.y, 0.001)
        var bg = UnlitMaterial(color: UIColor.black)
        bg.blending = .transparent(opacity: 0.6)
        let plate = ModelEntity(mesh: .generatePlane(width: b.extents.x + 0.03, height: b.extents.y + 0.02, cornerRadius: 0.008), materials: [bg])
        let holder = Entity()
        holder.addChild(plate)
        holder.addChild(t)
        // faceLabels() points the label's −Z away from the camera, so its +Z (the text's front) faces the camera.
        return holder
    }
}
