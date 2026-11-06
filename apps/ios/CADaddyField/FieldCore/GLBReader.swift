// CADaddy Field — glTF 2.0 binary (.glb) reader.
//
// Reads exactly what a CADaddy AR export needs: triangle meshes (POSITION, NORMAL, indices), base colour
// and alpha per material, the node tree with TRS/matrix transforms, node `extras` metadata, mesh reuse
// (instancing) and EXT_mesh_gpu_instancing. Everything taken from the file is bounds-checked and capped;
// malformed input throws a FieldError with a message, it never traps.

import Foundation
import simd

public struct MeshPrimitive: Sendable {
    public var positions: [SIMD3<Float>]
    public var normals: [SIMD3<Float>]
    public var indices: [UInt32]
    /// Linear RGBA base colour.
    public var color: SIMD4<Float>
    public var boundsMin: SIMD3<Float>
    public var boundsMax: SIMD3<Float>
    public var triangleCount: Int { indices.count / 3 }
}

public struct ModelMesh: Sendable {
    public var name: String
    public var primitives: [MeshPrimitive]
    public var boundsMin: SIMD3<Float>
    public var boundsMax: SIMD3<Float>
    public var triangleCount: Int { primitives.reduce(0) { $0 + $1.triangleCount } }
}

/// One use of a mesh inside an element, placed relative to the element's own frame.
public struct PlacedMesh: Sendable {
    public var mesh: Int
    public var transform: simd_float4x4
}

/// An exported element: one tappable object (a device, a run, a wall), with its metadata.
public struct FieldElement: Sendable, Identifiable {
    public var index: Int
    public var id: String { info.id }
    public var name: String
    public var info: ElementInfo
    /// Model-space transform of the element's node (metres, Y up, anchor at the origin).
    public var transform: simd_float4x4
    public var parts: [PlacedMesh]
    /// Model-space bounding box of all parts (nil when the element has no geometry).
    public var boundsMin: SIMD3<Float>?
    public var boundsMax: SIMD3<Float>?

    /// Model-space position of the element's node origin.
    public var origin: SIMD3<Float> { SIMD3(transform.columns.3.x, transform.columns.3.y, transform.columns.3.z) }
    public var center: SIMD3<Float> {
        guard let a = boundsMin, let b = boundsMax else { return origin }
        return (a + b) * 0.5
    }
}

/// A parsed package model, independent of RealityKit.
public struct FieldModel: Sendable {
    public var meshes: [ModelMesh]
    public var elements: [FieldElement]
    /// Anchor/metadata embedded in the model file (glTF asset or scene extras), if any.
    public var embeddedSidecar: Sidecar?
    public var warnings: [String]

    public init(meshes: [ModelMesh], elements: [FieldElement], embeddedSidecar: Sidecar? = nil, warnings: [String] = []) {
        self.meshes = meshes; self.elements = elements; self.embeddedSidecar = embeddedSidecar; self.warnings = warnings
    }

    public var triangleCount: Int {
        elements.reduce(0) { acc, e in acc + e.parts.reduce(0) { $0 + (meshes.indices.contains($1.mesh) ? meshes[$1.mesh].triangleCount : 0) } }
    }
    public var trades: [String] { Array(Set(elements.map { $0.info.trade ?? "unassigned" })).sorted() }
    public var kinds: [String] { Array(Set(elements.map { $0.info.kind ?? "unknown" })).sorted() }

    public var boundsMin: SIMD3<Float>? {
        let all = elements.compactMap(\.boundsMin)
        guard let first = all.first else { return nil }
        return all.reduce(first) { simd_min($0, $1) }
    }
    public var boundsMax: SIMD3<Float>? {
        let all = elements.compactMap(\.boundsMax)
        guard let first = all.first else { return nil }
        return all.reduce(first) { simd_max($0, $1) }
    }

    /// Applies sidecar element rows to elements with matching id or node name (sidecar fills gaps; the
    /// model's own extras win where both have a value).
    public mutating func merge(sidecar: Sidecar) {
        guard !sidecar.elements.isEmpty else { return }
        var byKey: [String: ElementInfo] = [:]
        for row in sidecar.elements {
            byKey[row.id] = row
            if let n = row.nodeName { byKey[Self.sanitize(n)] = row }
        }
        for i in elements.indices {
            let e = elements[i]
            guard let row = byKey[e.info.id] ?? byKey[Self.sanitize(e.name)] else { continue }
            var info = e.info
            let nameOnly = info.extra["metadata"] == "from node name only"
            if nameOnly { info.id = row.id; info.kind = row.kind ?? info.kind; info.tag = row.tag ?? info.tag; info.extra.removeValue(forKey: "metadata") }
            info.kind = info.kind ?? row.kind; info.tag = info.tag ?? row.tag
            info.circuit = info.circuit ?? row.circuit; info.panel = info.panel ?? row.panel
            info.mountingHeight = info.mountingHeight ?? row.mountingHeight; info.catalog = info.catalog ?? row.catalog
            info.trade = info.trade ?? row.trade; info.source = info.source ?? row.source
            for (k, v) in row.extra where info.extra[k] == nil { info.extra[k] = v }
            elements[i].info = info
        }
    }

    /// USD prim names allow letters, digits and underscores; exporters replace the rest with "_".
    public static func sanitize(_ name: String) -> String {
        String(name.unicodeScalars.map { CharacterSet.alphanumerics.contains($0) && $0.isASCII ? Character($0) : "_" })
    }

    /// Candidate model points for two-point alignment: the anchor (origin), named reference points,
    /// and the corners of context geometry at floor level (wall/floor vertices with |y| < 5 cm).
    public func snapPoints(extra: [ReferencePoint] = [], limit: Int = 4000) -> [ReferencePoint] {
        var out: [ReferencePoint] = [ReferencePoint(name: "Anchor (origin)", position: .zero)]
        out.append(contentsOf: extra)
        var seen = Set<SIMD2<Int32>>()
        seen.insert(SIMD2(0, 0))
        for e in elements where e.info.isContext {
            for part in e.parts {
                guard meshes.indices.contains(part.mesh) else { continue }
                let m = e.transform * part.transform
                for prim in meshes[part.mesh].primitives {
                    for p in prim.positions {
                        let w = m * SIMD4(p, 1)
                        guard abs(w.y) < 0.05, w.x.isFinite, w.z.isFinite else { continue }
                        // 1 cm grid de-duplication.
                        let key = SIMD2(Int32(clamping: Int((w.x * 100).rounded())), Int32(clamping: Int((w.z * 100).rounded())))
                        if seen.insert(key).inserted {
                            out.append(ReferencePoint(name: "Corner", position: SIMD3(w.x, 0, w.z)))
                            if out.count >= limit { return out }
                        }
                    }
                }
            }
        }
        return out
    }
}

public enum GLBReader {
    static let magic: UInt32 = 0x4654_6C67 // "glTF"
    static let chunkJSON: UInt32 = 0x4E4F_534A
    static let chunkBIN: UInt32 = 0x004E_4942

    /// Reads a .glb file. Throws a FieldError with a user message on anything malformed or over a cap.
    public static func read(url: URL) throws -> FieldModel {
        let size = (try? url.resourceValues(forKeys: [.fileSizeKey]).fileSize).map(Int64.init) ?? 0
        guard size <= FieldLimits.maxModelBytes else { throw FieldError("The model is larger than \(FieldLimits.maxModelBytes / 1_048_576) MB.") }
        let data: Data
        do { data = try Data(contentsOf: url, options: .mappedIfSafe) } catch { throw FieldError("The model file can't be read.") }
        return try read(data: data)
    }

    public static func read(data: Data) throws -> FieldModel {
        guard Int64(data.count) <= FieldLimits.maxModelBytes else { throw FieldError("The model is too large.") }
        guard data.count >= 20 else { throw FieldError("This is not a glTF binary (.glb) file: it is too short.") }
        guard u32(data, 0) == magic else { throw FieldError("This is not a glTF binary (.glb) file.") }
        guard u32(data, 4) == 2 else { throw FieldError("Only glTF 2.0 binary files are supported.") }
        let declared = Int(u32(data, 8) ?? 0)
        guard declared <= data.count, declared >= 20 else { throw FieldError("The .glb file is truncated.") }
        var offset = 12
        var json: Data?
        var bin: Data?
        var chunks = 0
        while offset + 8 <= declared, chunks < 16 {
            chunks += 1
            guard let len32 = u32(data, offset), let type = u32(data, offset + 4) else { break }
            let len = Int(len32)
            let start = offset + 8
            guard len >= 0, start <= declared, len <= declared - start else { throw FieldError("A .glb chunk runs past the end of the file.") }
            let body = data.subdata(in: (data.startIndex + start)..<(data.startIndex + start + len))
            if type == chunkJSON, json == nil { json = body }
            else if type == chunkBIN, bin == nil { bin = body }
            offset = start + ((len + 3) & ~3)
        }
        guard let jsonData = json else { throw FieldError("The .glb file has no JSON chunk.") }
        guard jsonData.count <= FieldLimits.maxGLBJSONBytes else { throw FieldError("The .glb JSON chunk is too large.") }
        let root: [String: Any]
        do {
            guard let r = try JSONSerialization.jsonObject(with: jsonData, options: []) as? [String: Any] else {
                throw FieldError("The .glb JSON is not an object.")
            }
            root = r
        } catch let e as FieldError { throw e } catch { throw FieldError("The .glb JSON is not valid.") }
        var parser = Parser(root: root, bin: bin ?? Data())
        return try parser.run()
    }

    static func u32(_ d: Data, _ off: Int) -> UInt32? {
        guard off >= 0, off + 4 <= d.count else { return nil }
        return d.withUnsafeBytes { $0.loadUnaligned(fromByteOffset: off, as: UInt32.self) }.littleEndian
    }

    // MARK: -

    struct Parser {
        let root: [String: Any]
        let bin: Data
        var warnings: [String] = []
        var meshCache: [Int: Int] = [:]   // glTF mesh index -> output mesh index
        var meshes: [ModelMesh] = []
        var materialColors: [SIMD4<Float>] = []
        var totalTriangles = 0
        var visits = 0

        let nodesJSON: [[String: Any]]
        let meshesJSON: [[String: Any]]
        let accessorsJSON: [[String: Any]]
        let viewsJSON: [[String: Any]]

        init(root: [String: Any], bin: Data) {
            self.root = root
            self.bin = bin
            nodesJSON = (root["nodes"] as? [Any])?.map { $0 as? [String: Any] ?? [:] } ?? []
            meshesJSON = (root["meshes"] as? [Any])?.map { $0 as? [String: Any] ?? [:] } ?? []
            accessorsJSON = (root["accessors"] as? [Any])?.map { $0 as? [String: Any] ?? [:] } ?? []
            viewsJSON = (root["bufferViews"] as? [Any])?.map { $0 as? [String: Any] ?? [:] } ?? []
        }

        mutating func run() throws -> FieldModel {
            guard nodesJSON.count <= FieldLimits.maxNodes else { throw FieldError("The model has too many nodes (\(nodesJSON.count)).") }
            guard meshesJSON.count <= FieldLimits.maxMeshes else { throw FieldError("The model has too many meshes.") }
            guard accessorsJSON.count <= FieldLimits.maxAccessors else { throw FieldError("The model has too many accessors.") }
            if let buffers = root["buffers"] as? [Any] {
                for (i, b) in buffers.enumerated() {
                    if let bd = b as? [String: Any], bd["uri"] != nil {
                        throw FieldError("Buffer \(i) points to an external file; CADaddy Field reads self-contained .glb files only.")
                    }
                }
            }
            if let required = root["extensionsRequired"] as? [String] {
                let unsupported = required.filter { $0 != "EXT_mesh_gpu_instancing" }
                if !unsupported.isEmpty { throw FieldError("The model needs glTF extensions this app doesn't read: \(unsupported.prefix(4).joined(separator: ", ")).") }
            }
            let mats = (root["materials"] as? [Any]) ?? []
            guard mats.count <= FieldLimits.maxMaterials else { throw FieldError("The model has too many materials.") }
            materialColors = mats.map { Self.color(of: $0 as? [String: Any] ?? [:]) }

            var embedded: Sidecar?
            if let asset = root["asset"] as? [String: Any], let ex = asset["extras"] as? [String: Any] {
                embedded = Self.sidecarFrom(extras: ex)
            }

            // Scene roots.
            var roots: [Int] = []
            let sceneIndex = (root["scene"] as? NSNumber)?.intValue ?? 0
            if let scenes = root["scenes"] as? [Any], !scenes.isEmpty {
                let s = (scenes.indices.contains(sceneIndex) ? scenes[sceneIndex] : scenes[0]) as? [String: Any] ?? [:]
                roots = Self.ints(s["nodes"])
                if embedded == nil, let ex = s["extras"] as? [String: Any] { embedded = Self.sidecarFrom(extras: ex) }
            } else {
                var isChild = Set<Int>()
                for n in nodesJSON { for c in Self.ints(n["children"]) { isChild.insert(c) } }
                roots = nodesJSON.indices.filter { !isChild.contains($0) }
            }

            var elements: [FieldElement] = []
            var path = Set<Int>()
            for r in roots {
                try walk(node: r, parentWorld: matrix_identity_float4x4, element: nil, relToElement: matrix_identity_float4x4,
                         depth: 0, path: &path, elements: &elements)
            }
            for i in elements.indices { computeBounds(&elements[i]) }
            return FieldModel(meshes: meshes, elements: elements, embeddedSidecar: embedded, warnings: Array(warnings.prefix(50)))
        }

        static func sidecarFrom(extras: [String: Any]) -> Sidecar? {
            let obj = (extras["cadaddy"] as? [String: Any]) ?? extras
            guard obj["anchor"] != nil else { return nil }
            return try? Sidecar.parse(object: obj)
        }

        mutating func walk(node: Int, parentWorld: simd_float4x4, element: Int?, relToElement: simd_float4x4, depth: Int,
                           path: inout Set<Int>, elements: inout [FieldElement]) throws {
            guard nodesJSON.indices.contains(node) else { warnings.append("A node reference is out of range."); return }
            guard depth < FieldLimits.maxNodeDepth else { throw FieldError("The model's node tree is nested too deeply.") }
            guard !path.contains(node) else { throw FieldError("The model's node tree has a cycle.") }
            visits += 1
            guard visits <= FieldLimits.maxNodeVisits else { throw FieldError("The model's node tree is too large.") }
            path.insert(node)
            defer { path.remove(node) }

            let n = nodesJSON[node]
            let local = localTransform(n)
            let world = parentWorld * local
            let name = (n["name"] as? String)?.clipped() ?? "node\(node)"
            var currentElement = element
            var rel = relToElement * local
            if let info = ElementInfo.parse(n["extras"], fallbackID: name, nodeName: name) {
                elements.append(FieldElement(index: elements.count, name: name, info: info, transform: world, parts: []))
                currentElement = elements.count - 1
                rel = matrix_identity_float4x4
            }
            if let meshIndex = (n["mesh"] as? NSNumber)?.intValue {
                if let out = try mesh(meshIndex) {
                    if currentElement == nil {
                        let info = ElementInfo.fromNodeName(name)
                        elements.append(FieldElement(index: elements.count, name: name, info: info, transform: world, parts: []))
                        currentElement = elements.count - 1
                        rel = matrix_identity_float4x4
                    }
                    if let ce = currentElement, elements.indices.contains(ce) {
                        let instances = try instanceTransforms(n)
                        if instances.isEmpty {
                            elements[ce].parts.append(PlacedMesh(mesh: out, transform: rel))
                        } else {
                            for t in instances { elements[ce].parts.append(PlacedMesh(mesh: out, transform: rel * t)) }
                        }
                    }
                }
            }
            for c in Self.ints(n["children"]) {
                try walk(node: c, parentWorld: world, element: currentElement, relToElement: rel, depth: depth + 1, path: &path, elements: &elements)
            }
        }

        mutating func computeBounds(_ e: inout FieldElement) {
            var lo: SIMD3<Float>?
            var hi: SIMD3<Float>?
            for p in e.parts where meshes.indices.contains(p.mesh) {
                let m = meshes[p.mesh]
                let (a, b) = Self.transformBounds(min: m.boundsMin, max: m.boundsMax, by: e.transform * p.transform)
                lo = lo.map { simd_min($0, a) } ?? a
                hi = hi.map { simd_max($0, b) } ?? b
            }
            e.boundsMin = lo
            e.boundsMax = hi
        }

        static func transformBounds(min a: SIMD3<Float>, max b: SIMD3<Float>, by m: simd_float4x4) -> (SIMD3<Float>, SIMD3<Float>) {
            var lo = SIMD3<Float>(repeating: .greatestFiniteMagnitude)
            var hi = SIMD3<Float>(repeating: -.greatestFiniteMagnitude)
            for i in 0..<8 {
                let c = SIMD3<Float>(i & 1 == 0 ? a.x : b.x, i & 2 == 0 ? a.y : b.y, i & 4 == 0 ? a.z : b.z)
                let w = m * SIMD4(c, 1)
                let p = SIMD3(w.x, w.y, w.z)
                lo = simd_min(lo, p); hi = simd_max(hi, p)
            }
            return (lo, hi)
        }

        // MARK: transforms

        mutating func localTransform(_ n: [String: Any]) -> simd_float4x4 {
            if let m = n["matrix"] as? [Any] {
                let v = m.compactMap { ($0 as? NSNumber)?.floatValue }
                guard v.count == 16, v.allSatisfy({ $0.isFinite }) else {
                    warnings.append("A node matrix is not 16 finite numbers; identity used.")
                    return matrix_identity_float4x4
                }
                return simd_float4x4(columns: (SIMD4(v[0], v[1], v[2], v[3]), SIMD4(v[4], v[5], v[6], v[7]),
                                               SIMD4(v[8], v[9], v[10], v[11]), SIMD4(v[12], v[13], v[14], v[15])))
            }
            let t = floats(n["translation"], 3) ?? [0, 0, 0]
            let r = floats(n["rotation"], 4) ?? [0, 0, 0, 1]
            let s = floats(n["scale"], 3) ?? [1, 1, 1]
            return Self.trs(SIMD3(t[0], t[1], t[2]), simd_quatf(ix: r[0], iy: r[1], iz: r[2], r: r[3]), SIMD3(s[0], s[1], s[2]))
        }

        static func trs(_ t: SIMD3<Float>, _ q: simd_quatf, _ s: SIMD3<Float>) -> simd_float4x4 {
            let len = simd_length(q.vector)
            let qn = len > 1e-6 && len.isFinite ? simd_quatf(vector: q.vector / len) : simd_quatf(ix: 0, iy: 0, iz: 0, r: 1)
            var m = simd_float4x4(qn)
            m.columns.0 *= s.x
            m.columns.1 *= s.y
            m.columns.2 *= s.z
            m.columns.3 = SIMD4(t, 1)
            return m
        }

        mutating func floats(_ v: Any?, _ count: Int) -> [Float]? {
            guard let a = v as? [Any] else { return nil }
            let f = a.compactMap { ($0 as? NSNumber)?.floatValue }
            guard f.count == count else { warnings.append("A node transform has the wrong number of values."); return nil }
            guard f.allSatisfy({ $0.isFinite }) else { warnings.append("A node transform is not finite; ignored."); return nil }
            return f
        }

        mutating func instanceTransforms(_ n: [String: Any]) throws -> [simd_float4x4] {
            guard let ext = (n["extensions"] as? [String: Any])?["EXT_mesh_gpu_instancing"] as? [String: Any],
                  let attrs = ext["attributes"] as? [String: Any] else { return [] }
            let t = try (attrs["TRANSLATION"] as? NSNumber).map { try readFloats($0.intValue, expect: "VEC3") } ?? nil
            let r = try (attrs["ROTATION"] as? NSNumber).map { try readFloats($0.intValue, expect: "VEC4") } ?? nil
            let s = try (attrs["SCALE"] as? NSNumber).map { try readFloats($0.intValue, expect: "VEC3") } ?? nil
            let count = [t.map { $0.count / 3 }, r.map { $0.count / 4 }, s.map { $0.count / 3 }].compactMap { $0 }.min() ?? 0
            guard count <= FieldLimits.maxInstancesPerNode else { throw FieldError("A node has too many instances.") }
            var out: [simd_float4x4] = []
            out.reserveCapacity(count)
            for i in 0..<count {
                let tv = t.map { SIMD3($0[i * 3], $0[i * 3 + 1], $0[i * 3 + 2]) } ?? .zero
                let rv = r.map { simd_quatf(ix: $0[i * 4], iy: $0[i * 4 + 1], iz: $0[i * 4 + 2], r: $0[i * 4 + 3]) } ?? simd_quatf(ix: 0, iy: 0, iz: 0, r: 1)
                let sv = s.map { SIMD3($0[i * 3], $0[i * 3 + 1], $0[i * 3 + 2]) } ?? SIMD3(repeating: 1)
                guard tv.x.isFinite, tv.y.isFinite, tv.z.isFinite, sv.x.isFinite, sv.y.isFinite, sv.z.isFinite else { continue }
                out.append(Self.trs(tv, rv, sv))
            }
            return out
        }

        // MARK: meshes

        mutating func mesh(_ index: Int) throws -> Int? {
            if let done = meshCache[index] { return done }
            guard meshesJSON.indices.contains(index) else { warnings.append("A mesh reference is out of range."); return nil }
            let m = meshesJSON[index]
            let prims = (m["primitives"] as? [Any]) ?? []
            guard prims.count <= FieldLimits.maxPrimitivesPerMesh else { throw FieldError("A mesh has too many primitives.") }
            var out: [MeshPrimitive] = []
            for p in prims {
                guard let pd = p as? [String: Any] else { continue }
                if let prim = try primitive(pd) { out.append(prim) }
            }
            guard !out.isEmpty else { meshCache[index] = nil; return nil }
            var lo = out[0].boundsMin, hi = out[0].boundsMax
            for p in out.dropFirst() { lo = simd_min(lo, p.boundsMin); hi = simd_max(hi, p.boundsMax) }
            meshes.append(ModelMesh(name: (m["name"] as? String)?.clipped() ?? "mesh\(index)", primitives: out, boundsMin: lo, boundsMax: hi))
            meshCache[index] = meshes.count - 1
            return meshes.count - 1
        }

        mutating func primitive(_ p: [String: Any]) throws -> MeshPrimitive? {
            let mode = (p["mode"] as? NSNumber)?.intValue ?? 4
            guard mode == 4 else { warnings.append("Skipped a primitive that is not triangles (mode \(mode))."); return nil }
            guard let attrs = p["attributes"] as? [String: Any], let posIndex = (attrs["POSITION"] as? NSNumber)?.intValue else {
                warnings.append("Skipped a primitive without positions."); return nil
            }
            if p["targets"] != nil { warnings.append("Morph targets are ignored.") }
            guard let posFlat = try readFloats(posIndex, expect: "VEC3") else { warnings.append("Skipped a primitive with unreadable positions."); return nil }
            let vcount = posFlat.count / 3
            guard vcount > 0 else { return nil }
            guard vcount <= FieldLimits.maxVerticesPerPrimitive else { throw FieldError("A mesh has too many vertices (\(vcount)).") }
            var positions = [SIMD3<Float>]()
            positions.reserveCapacity(vcount)
            for i in 0..<vcount {
                let v = SIMD3(posFlat[i * 3], posFlat[i * 3 + 1], posFlat[i * 3 + 2])
                guard v.x.isFinite, v.y.isFinite, v.z.isFinite, abs(v.x) < 1e6, abs(v.y) < 1e6, abs(v.z) < 1e6 else {
                    warnings.append("Skipped a primitive with non-finite or out-of-range positions."); return nil
                }
                positions.append(v)
            }
            var indices: [UInt32]
            if let idx = (p["indices"] as? NSNumber)?.intValue {
                guard let read = try readIndices(idx) else { warnings.append("Skipped a primitive with unreadable indices."); return nil }
                indices = read
            } else {
                indices = (0..<UInt32(clamping: vcount)).map { $0 }
            }
            indices.removeLast(indices.count % 3)
            guard !indices.isEmpty else { return nil }
            guard indices.allSatisfy({ Int($0) < vcount }) else { warnings.append("Skipped a primitive with out-of-range indices."); return nil }
            totalTriangles += indices.count / 3
            guard totalTriangles <= FieldLimits.maxTotalTriangles else {
                throw FieldError("The model has more than \(FieldLimits.maxTotalTriangles / 1_000_000) million triangles. Export a smaller scope.")
            }
            var normals: [SIMD3<Float>] = []
            if let nIndex = (attrs["NORMAL"] as? NSNumber)?.intValue, let nf = try readFloats(nIndex, expect: "VEC3"), nf.count == vcount * 3 {
                normals.reserveCapacity(vcount)
                for i in 0..<vcount {
                    let n = SIMD3(nf[i * 3], nf[i * 3 + 1], nf[i * 3 + 2])
                    normals.append(n.x.isFinite && n.y.isFinite && n.z.isFinite ? n : SIMD3(0, 1, 0))
                }
            } else {
                normals = Self.computeNormals(positions, indices)
            }
            var lo = positions[0], hi = positions[0]
            for v in positions { lo = simd_min(lo, v); hi = simd_max(hi, v) }
            var color = SIMD4<Float>(0.8, 0.8, 0.8, 1)
            if let mi = (p["material"] as? NSNumber)?.intValue, materialColors.indices.contains(mi) { color = materialColors[mi] }
            return MeshPrimitive(positions: positions, normals: normals, indices: indices, color: color, boundsMin: lo, boundsMax: hi)
        }

        static func computeNormals(_ pos: [SIMD3<Float>], _ idx: [UInt32]) -> [SIMD3<Float>] {
            var n = [SIMD3<Float>](repeating: .zero, count: pos.count)
            var i = 0
            while i + 2 < idx.count {
                let a = Int(idx[i]), b = Int(idx[i + 1]), c = Int(idx[i + 2])
                if pos.indices.contains(a), pos.indices.contains(b), pos.indices.contains(c) {
                    let f = simd_cross(pos[b] - pos[a], pos[c] - pos[a])
                    n[a] += f; n[b] += f; n[c] += f
                }
                i += 3
            }
            return n.map { simd_length($0) > 1e-12 ? simd_normalize($0) : SIMD3(0, 1, 0) }
        }

        static func color(of m: [String: Any]) -> SIMD4<Float> {
            let pbr = m["pbrMetallicRoughness"] as? [String: Any] ?? [:]
            let f = ((pbr["baseColorFactor"] as? [Any]) ?? []).compactMap { ($0 as? NSNumber)?.floatValue }
            var c = SIMD4<Float>(1, 1, 1, 1)
            if f.count == 4, f.allSatisfy({ $0.isFinite }) { c = simd_clamp(SIMD4(f[0], f[1], f[2], f[3]), .zero, SIMD4(repeating: 1)) }
            if (m["alphaMode"] as? String) != "BLEND" { c.w = 1 }
            return c
        }

        // MARK: accessors

        static func components(_ type: String) -> Int? {
            switch type {
            case "SCALAR": return 1
            case "VEC2": return 2
            case "VEC3": return 3
            case "VEC4": return 4
            case "MAT4": return 16
            default: return nil
            }
        }

        static func componentBytes(_ ct: Int) -> Int? {
            switch ct {
            case 5120, 5121: return 1
            case 5122, 5123: return 2
            case 5125, 5126: return 4
            default: return nil
            }
        }

        struct View { let start: Int; let stride: Int; let count: Int; let ncomp: Int; let ctype: Int; let normalized: Bool }

        func view(_ index: Int) throws -> View? {
            guard accessorsJSON.indices.contains(index) else { return nil }
            let a = accessorsJSON[index]
            if a["sparse"] != nil { return nil }
            guard let type = a["type"] as? String, let ncomp = Self.components(type),
                  let ctype = (a["componentType"] as? NSNumber)?.intValue, let cbytes = Self.componentBytes(ctype),
                  let count = (a["count"] as? NSNumber)?.intValue, count >= 0,
                  let vi = (a["bufferView"] as? NSNumber)?.intValue, viewsJSON.indices.contains(vi) else { return nil }
            let v = viewsJSON[vi]
            guard ((v["buffer"] as? NSNumber)?.intValue ?? 0) == 0 else { return nil }
            let vOff = (v["byteOffset"] as? NSNumber)?.intValue ?? 0
            guard let vLen = (v["byteLength"] as? NSNumber)?.intValue else { return nil }
            let aOff = (a["byteOffset"] as? NSNumber)?.intValue ?? 0
            let elem = ncomp * cbytes
            let stride = (v["byteStride"] as? NSNumber)?.intValue ?? elem
            guard vOff >= 0, vLen >= 0, aOff >= 0, stride >= elem, stride <= 252 else { return nil }
            guard vOff <= bin.count, vLen <= bin.count - vOff else { return nil }
            if count == 0 { return View(start: 0, stride: stride, count: 0, ncomp: ncomp, ctype: ctype, normalized: false) }
            let (span, o1) = stride.multipliedReportingOverflow(by: count - 1)
            guard !o1 else { return nil }
            let (need, o2) = aOff.addingReportingOverflow(span)
            guard !o2, need <= vLen - elem else { return nil }
            return View(start: vOff + aOff, stride: stride, count: count, ncomp: ncomp, ctype: ctype,
                        normalized: (a["normalized"] as? NSNumber)?.boolValue ?? false)
        }

        func readFloats(_ index: Int, expect type: String) throws -> [Float]? {
            guard let a = accessorsJSON.indices.contains(index) ? accessorsJSON[index] : nil,
                  (a["type"] as? String) == type, let v = try view(index) else { return nil }
            guard v.count <= FieldLimits.maxVerticesPerPrimitive else { throw FieldError("An accessor is too large.") }
            var out = [Float]()
            out.reserveCapacity(v.count * v.ncomp)
            let cb = Self.componentBytes(v.ctype) ?? 4
            bin.withUnsafeBytes { raw in
                for i in 0..<v.count {
                    let base = v.start + i * v.stride
                    for c in 0..<v.ncomp {
                        let off = base + c * cb
                        switch v.ctype {
                        case 5126: out.append(raw.loadUnaligned(fromByteOffset: off, as: Float.self))
                        case 5123: let x = Float(raw.loadUnaligned(fromByteOffset: off, as: UInt16.self)); out.append(v.normalized ? x / 65535 : x)
                        case 5122: let x = Float(raw.loadUnaligned(fromByteOffset: off, as: Int16.self)); out.append(v.normalized ? max(x / 32767, -1) : x)
                        case 5121: let x = Float(raw.loadUnaligned(fromByteOffset: off, as: UInt8.self)); out.append(v.normalized ? x / 255 : x)
                        case 5120: let x = Float(raw.loadUnaligned(fromByteOffset: off, as: Int8.self)); out.append(v.normalized ? max(x / 127, -1) : x)
                        default: out.append(0)
                        }
                    }
                }
            }
            return out
        }

        func readIndices(_ index: Int) throws -> [UInt32]? {
            guard let a = accessorsJSON.indices.contains(index) ? accessorsJSON[index] : nil, (a["type"] as? String) == "SCALAR",
                  let v = try view(index), [5121, 5123, 5125].contains(v.ctype) else { return nil }
            guard v.count <= FieldLimits.maxVerticesPerPrimitive * 3 else { throw FieldError("An index list is too large.") }
            var out = [UInt32]()
            out.reserveCapacity(v.count)
            bin.withUnsafeBytes { raw in
                for i in 0..<v.count {
                    let off = v.start + i * v.stride
                    switch v.ctype {
                    case 5125: out.append(raw.loadUnaligned(fromByteOffset: off, as: UInt32.self))
                    case 5123: out.append(UInt32(raw.loadUnaligned(fromByteOffset: off, as: UInt16.self)))
                    default: out.append(UInt32(raw.loadUnaligned(fromByteOffset: off, as: UInt8.self)))
                    }
                }
            }
            return out
        }

        static func ints(_ v: Any?) -> [Int] {
            ((v as? [Any]) ?? []).prefix(FieldLimits.maxNodes).compactMap { ($0 as? NSNumber)?.intValue }
        }
    }
}
