// CADaddy Field — a small glTF 2.0 binary writer, used to build the synthetic sample room and test packages
// in the same layout `bim.export.ar` writes (one node per element, metadata in node extras, shared meshes).

import Foundation
import simd

public struct GLBWriter {
    public struct Mesh {
        public var name: String
        public var positions: [SIMD3<Float>]
        public var normals: [SIMD3<Float>]
        public var indices: [UInt32]
        public var material: Int
        public init(name: String, positions: [SIMD3<Float>], normals: [SIMD3<Float>], indices: [UInt32], material: Int) {
            self.name = name; self.positions = positions; self.normals = normals; self.indices = indices; self.material = material
        }
    }

    public struct Node {
        public var name: String
        public var mesh: Int?
        public var translation: SIMD3<Float>
        /// Rotation about +Y, radians.
        public var yaw: Float
        public var scale: SIMD3<Float>
        public var extras: [String: Any]?
        public var children: [Int]
        public init(name: String, mesh: Int?, translation: SIMD3<Float> = .zero, yaw: Float = 0, scale: SIMD3<Float> = SIMD3(repeating: 1),
                    extras: [String: Any]? = nil, children: [Int] = []) {
            self.name = name; self.mesh = mesh; self.translation = translation; self.yaw = yaw; self.scale = scale
            self.extras = extras; self.children = children
        }
    }

    public var materials: [(name: String, color: SIMD4<Float>)] = []
    public var meshes: [Mesh] = []
    public var nodes: [Node] = []
    public var rootNodes: [Int] = []
    public var assetExtras: [String: Any]?

    public init() {}

    public mutating func addMaterial(_ name: String, _ color: SIMD4<Float>) -> Int {
        materials.append((name, color)); return materials.count - 1
    }

    public mutating func addMesh(_ m: Mesh) -> Int { meshes.append(m); return meshes.count - 1 }

    @discardableResult
    public mutating func addNode(_ n: Node, root: Bool = true) -> Int {
        nodes.append(n)
        if root { rootNodes.append(nodes.count - 1) }
        return nodes.count - 1
    }

    /// An axis-aligned box mesh centred on `center` with full extents `size`.
    public static func box(name: String, size: SIMD3<Float>, center: SIMD3<Float> = .zero, material: Int) -> Mesh {
        let h = size * 0.5
        let faces: [(SIMD3<Float>, SIMD3<Float>, SIMD3<Float>)] = [
            (SIMD3(1, 0, 0), SIMD3(0, 1, 0), SIMD3(0, 0, 1)), (SIMD3(-1, 0, 0), SIMD3(0, 1, 0), SIMD3(0, 0, -1)),
            (SIMD3(0, 1, 0), SIMD3(0, 0, 1), SIMD3(1, 0, 0)), (SIMD3(0, -1, 0), SIMD3(0, 0, -1), SIMD3(1, 0, 0)),
            (SIMD3(0, 0, 1), SIMD3(1, 0, 0), SIMD3(0, 1, 0)), (SIMD3(0, 0, -1), SIMD3(-1, 0, 0), SIMD3(0, 1, 0)),
        ]
        var pos: [SIMD3<Float>] = [], nor: [SIMD3<Float>] = [], idx: [UInt32] = []
        for (n, u, v) in faces {
            let base = UInt32(pos.count)
            let c = n * h
            let uu = u * h, vv = v * h
            for (su, sv) in [(-1, -1), (1, -1), (1, 1), (-1, 1)] as [(Float, Float)] {
                pos.append(center + c + uu * su + vv * sv); nor.append(n)
            }
            // Counter-clockwise seen from outside (u × v = n).
            idx += [base, base + 1, base + 2, base, base + 2, base + 3]
        }
        return Mesh(name: name, positions: pos, normals: nor, indices: idx, material: material)
    }

    /// Serialises to .glb bytes.
    public func encode() throws -> Data {
        var bin = Data()
        var views: [[String: Any]] = []
        var accessors: [[String: Any]] = []
        func align4() { while bin.count % 4 != 0 { bin.append(0) } }
        func addView(_ bytes: Data, target: Int) -> Int {
            align4()
            views.append(["buffer": 0, "byteOffset": bin.count, "byteLength": bytes.count, "target": target])
            bin.append(bytes)
            return views.count - 1
        }
        var gltfMeshes: [[String: Any]] = []
        for m in meshes {
            var pdata = Data(), ndata = Data(), idata = Data()
            var lo = m.positions.first ?? .zero, hi = lo
            for p in m.positions { lo = simd_min(lo, p); hi = simd_max(hi, p); for c in [p.x, p.y, p.z] { withUnsafeBytes(of: c.bitPattern.littleEndian) { pdata.append(contentsOf: $0) } } }
            for n in m.normals { for c in [n.x, n.y, n.z] { withUnsafeBytes(of: c.bitPattern.littleEndian) { ndata.append(contentsOf: $0) } } }
            for i in m.indices { withUnsafeBytes(of: i.littleEndian) { idata.append(contentsOf: $0) } }
            let pv = addView(pdata, target: 34962)
            accessors.append(["bufferView": pv, "componentType": 5126, "count": m.positions.count, "type": "VEC3",
                              "min": [lo.x, lo.y, lo.z], "max": [hi.x, hi.y, hi.z]])
            var attrs: [String: Any] = ["POSITION": accessors.count - 1]
            if m.normals.count == m.positions.count {
                let nv = addView(ndata, target: 34962)
                accessors.append(["bufferView": nv, "componentType": 5126, "count": m.normals.count, "type": "VEC3"])
                attrs["NORMAL"] = accessors.count - 1
            }
            let iv = addView(idata, target: 34963)
            accessors.append(["bufferView": iv, "componentType": 5125, "count": m.indices.count, "type": "SCALAR"])
            gltfMeshes.append(["name": m.name, "primitives": [["attributes": attrs, "indices": accessors.count - 1, "material": m.material, "mode": 4]]])
        }
        align4()
        let mats: [[String: Any]] = materials.map { mat in
            var d: [String: Any] = ["name": mat.name, "pbrMetallicRoughness": ["baseColorFactor": [mat.color.x, mat.color.y, mat.color.z, mat.color.w], "metallicFactor": 0.0, "roughnessFactor": 0.8]]
            if mat.color.w < 1 { d["alphaMode"] = "BLEND" }
            return d
        }
        let gnodes: [[String: Any]] = nodes.map { n in
            var d: [String: Any] = ["name": n.name]
            if let m = n.mesh { d["mesh"] = m }
            if n.translation != .zero { d["translation"] = [n.translation.x, n.translation.y, n.translation.z] }
            if n.yaw != 0 { d["rotation"] = [0, sin(n.yaw / 2), 0, cos(n.yaw / 2)] }
            if n.scale != SIMD3(repeating: 1) { d["scale"] = [n.scale.x, n.scale.y, n.scale.z] }
            if let e = n.extras { d["extras"] = e }
            if !n.children.isEmpty { d["children"] = n.children }
            return d
        }
        var asset: [String: Any] = ["version": "2.0", "generator": "CADaddy Field sample writer"]
        if let e = assetExtras { asset["extras"] = e }
        var root: [String: Any] = ["asset": asset, "scene": 0, "scenes": [["nodes": rootNodes]], "nodes": gnodes,
                                   "meshes": gltfMeshes, "materials": mats, "accessors": accessors, "bufferViews": views]
        if !bin.isEmpty { root["buffers"] = [["byteLength": bin.count]] }
        var json = try JSONSerialization.data(withJSONObject: root, options: [.sortedKeys])
        while json.count % 4 != 0 { json.append(0x20) }
        var out = Data()
        func u32(_ v: UInt32) { withUnsafeBytes(of: v.littleEndian) { out.append(contentsOf: $0) } }
        let total = 12 + 8 + json.count + (bin.isEmpty ? 0 : 8 + bin.count)
        u32(GLBReader.magic); u32(2); u32(UInt32(total))
        u32(UInt32(json.count)); u32(GLBReader.chunkJSON); out.append(json)
        if !bin.isEmpty { u32(UInt32(bin.count)); u32(GLBReader.chunkBIN); out.append(bin) }
        return out
    }
}
