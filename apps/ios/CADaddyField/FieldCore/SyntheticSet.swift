// CADaddy Field — a generated synthetic building's package set, for scale runs only: the size of a real building
// (around 200 rooms and a few hundred thousand objects), every name, number and tag invented. It is written in the
// exporter's format (building.json schema cadaddy.ar.building/1, one .glb per package with the export's header and
// node extras); `testTheGeneratedSetReadsLikeTheExportersOwn` checks it uses only keys the exporter's own fixture
// has, so it cannot drift into a format the exporter never writes. The correctness tests use the exporter's output.

import CryptoKit
import Foundation
import simd

public enum SyntheticSet {
    public struct Spec: Sendable {
        public var title: String
        public var levels: Int
        public var roomsPerLevel: Int
        /// Objects in each room's package (devices, boxes, conduit, ducts, pipes, fixtures, cabinets, furniture).
        public var perRoom: Int
        /// Objects of each level's own package outside its rooms (walls, lights, heads, branch runs).
        public var perLevel: Int

        public static func scale(rooms: Int) -> Spec {
            let levels = max(1, min(12, (rooms + 31) / 32))
            return Spec(title: "Synthetic Building (\(rooms) rooms)", levels: levels, roomsPerLevel: max(1, rooms / levels), perRoom: 1_200, perLevel: 8_000)
        }
        public static let small = Spec(title: "Synthetic Building (small)", levels: 2, roomsPerLevel: 4, perRoom: 40, perLevel: 120)
    }

    struct Node { var extras: [String: Any]; var center: SIMD3<Float>; var size: SIMD3<Float>; var material: Int }

    /// Writes the set into `dir` (replacing what is there).
    public static func write(to dir: URL, spec: Spec) throws {
        let fm = FileManager.default
        try? fm.removeItem(at: dir)
        for sub in ["", "levels", "rooms"] { try fm.createDirectory(at: dir.appendingPathComponent(sub), withIntermediateDirectories: true) }
        let ft = Float(Units.metersPerFoot)
        let roomW: Float = 18, roomD: Float = 28, corridor: Float = 8
        let perSide = (spec.roomsPerLevel + 1) / 2
        let length = Float(perSide) * roomW, depth = 2 * roomD + corridor
        let storey: Float = 10
        var serial = 0x10000
        func nid() -> String { serial += 1; return String(serial, radix: 16, uppercase: true) }
        func m(_ x: Float, _ y: Float, _ z: Float) -> SIMD3<Float> { SIMD3(x * ft, z * ft, -y * ft) }   // plan ft (x, y), up ft → model

        var graph: [[String: Any]] = []
        var packages: [[String: Any]] = [], roomPackages: [[String: Any]] = []
        var levelsJSON: [[String: Any]] = [], roomsJSON: [[String: Any]] = []
        var byLevel: [String: [String]] = [:], byRoom: [String: [String]] = [:]
        var mains: [Node] = [], full: [Node] = [], arch: [Node] = []
        var graphPackages: [String: [String]] = [:]
        func link(_ id: String, _ pkg: String) { graphPackages[id, default: []].append(pkg) }

        let gearID = nid(), housePanel = nid()
        mains.append(Node(extras: ["id": gearID, "kind": "equipment", "role": "gear", "group": "electrical", "trade": "electrical", "tag": "SWB-1", "level": "L1", "category": "Equipment"],
                          center: m(10, 4, 3.75), size: SIMD3(6, 7.5, 3) * ft, material: 1))
        mains.append(Node(extras: ["id": housePanel, "kind": "equipment", "role": "panel", "group": "electrical", "trade": "electrical", "tag": "HP-1", "level": "L1", "category": "Equipment"],
                          center: m(20, 2, 4), size: SIMD3(2, 5, 0.6) * ft, material: 1))
        graph.append(["id": gearID, "kind": "equipment", "role": "gear", "group": "electrical", "level": "L1", "tag": "SWB-1", "fed_by": [], "link": "unknown"])
        graph.append(["id": housePanel, "kind": "equipment", "role": "panel", "group": "electrical", "level": "L1", "tag": "HP-1",
                      "fed_by": [["id": gearID, "tag": "SWB-1", "link": "known", "via": "F-1"]], "link": "known"])
        link(gearID, "building-mep-mains"); link(housePanel, "building-mep-mains")
        mains.append(Node(extras: ["id": "outline", "kind": "outline", "role": "outline", "group": "base"], center: m(length / 2, depth / 2, 0), size: SIMD3(length, 0.05, depth) * ft, material: 0))

        for lv in 0..<spec.levels {
            let lname = "L\(lv + 1)", e = Float(lv) * storey
            let lpkg = "levels/\(lname)"
            byLevel[lname] = [lpkg]
            levelsJSON.append(["name": lname, "elevation_ft": Double(e), "height_ft": Double(storey), "package": lpkg, "rooms": spec.roomsPerLevel,
                               "outline": [[[0, 0], [Double(length), 0], [Double(length), Double(depth)], [0, Double(depth)], [0, 0]]]])
            // Building packages: the floor (mains context), architecture, a riser, mains and full runs.
            let floorID = nid()
            mains.append(Node(extras: ["id": floorID, "kind": "floor", "group": "base", "level": lname, "trade": "architecture"], center: m(length / 2, depth / 2, e - 0.25), size: SIMD3(length, 0.5, depth) * ft, material: 0))
            arch.append(Node(extras: ["id": floorID, "kind": "floor", "group": "base", "level": lname, "trade": "architecture"], center: m(length / 2, depth / 2, e - 0.25), size: SIMD3(length, 0.5, depth) * ft, material: 0))
            arch.append(Node(extras: ["id": "exterior:\(lname)", "kind": "exterior", "role": "exterior", "group": "base", "level": lname, "reveal": true], center: m(length / 2, depth / 2, e + storey / 2), size: SIMD3(length, storey, depth) * ft, material: 0))
            let levelPanel = nid()
            mains.append(Node(extras: ["id": levelPanel, "kind": "equipment", "role": "panel", "group": "electrical", "trade": "electrical", "tag": "LP-\(lv + 1)", "level": lname],
                              center: m(30, roomD + 1, e + 4), size: SIMD3(2, 4, 0.6) * ft, material: 1))
            graph.append(["id": levelPanel, "kind": "equipment", "role": "panel", "group": "electrical", "level": lname, "tag": "LP-\(lv + 1)",
                          "fed_by": [["id": housePanel, "tag": "HP-1", "link": "known", "via": "F-L\(lv + 1)"]], "link": "known"])
            link(levelPanel, "building-mep-mains")
            for k in 0..<250 {
                let x = Float(k % 50) * length / 50, sys = k % 3
                let id = nid()
                let kind = sys == 0 ? "duct" : (sys == 1 ? "pipe" : "conduit"), group = sys == 0 ? "mechanical" : (sys == 1 ? "plumbing" : "electrical")
                let node = Node(extras: ["id": id, "kind": kind, "role": "main", "group": group, "trade": group, "level": lname],
                                center: m(x + 2, roomD + 2 + Float(sys) * 1.5, e + 8.5), size: SIMD3(length / 50 - 0.2, 1, sys == 0 ? 2 : 0.5) * ft, material: 2 + sys)
                if k < 60 { mains.append(node); link(id, "building-mep-mains") } else { full.append(node); link(id, "building-mep-full") }
                graph.append(["id": id, "kind": kind, "role": "main", "group": group, "level": lname, "fed_by": sys == 2 ? [["id": levelPanel, "tag": "LP-\(lv + 1)", "link": "known"]] : [], "link": sys == 2 ? "known" : "unknown"])
            }
            for k in 0..<1_100 {
                let id = nid(), x = Float(k % 110) * length / 110, row = Float(k / 110)
                full.append(Node(extras: ["id": id, "kind": "duct", "role": "branch", "group": "mechanical", "trade": "mechanical", "level": lname],
                                 center: m(x + 1, 6 + row * 5, e + 8.8), size: SIMD3(1, 0.8, 4) * ft, material: 2))
                link(id, "building-mep-full")
                graph.append(["id": id, "kind": "duct", "role": "branch", "group": "mechanical", "level": lname, "fed_by": [], "link": "unknown"])
            }
            // The level package: its walls (cut at 4 ft), tiles, and what is on the floor outside the rooms.
            var lnodes: [Node] = []
            var rooms: [(id: String, number: String, x0: Float, y0: Float)] = []
            for r in 0..<spec.roomsPerLevel {
                let south = r < perSide
                let x0 = Float(south ? r : r - perSide) * roomW, y0: Float = south ? 0 : roomD + corridor
                let number = String(format: "%d%02d", lv + 1, r + 1)
                let rid = nid()
                rooms.append((rid, number, x0, y0))
                lnodes.append(Node(extras: ["id": rid, "kind": "room", "role": "room_tile", "group": "base", "level": lname, "number": number, "name": "ROOM", "tag": "\(number) ROOM"],
                                   center: m(x0 + roomW / 2, y0 + roomD / 2, e + 0.03), size: SIMD3(roomW - 0.5, 0.06, roomD - 0.5) * ft, material: 0))
                arch.append(Node(extras: ["id": rid, "kind": "room", "role": "room_tile", "group": "base", "level": lname, "number": number, "name": "ROOM"],
                                 center: m(x0 + roomW / 2, y0 + roomD / 2, e + 0.03), size: SIMD3(roomW - 0.5, 0.06, roomD - 0.5) * ft, material: 0))
                for (c, sz) in [(m(x0 + roomW / 2, y0, e), SIMD3(roomW, 0, 0.5)), (m(x0, y0 + roomD / 2, e), SIMD3(0.5, 0, roomD))] {
                    let wid = nid()
                    lnodes.append(Node(extras: ["id": wid, "kind": "wall", "group": "base", "level": lname, "trade": "architecture", "context": true],
                                       center: c + SIMD3(0, 2 * ft, 0), size: SIMD3(max(sz.x, 0.5), 4, max(sz.z, 0.5)) * ft, material: 0))
                    arch.append(Node(extras: ["id": wid, "kind": "wall", "group": "base", "level": lname, "trade": "architecture"],
                                     center: c + SIMD3(0, storey / 2 * ft, 0), size: SIMD3(max(sz.x, 0.5), storey, max(sz.z, 0.5)) * ft, material: 0))
                    graph.append(["id": wid, "kind": "wall", "group": "base", "level": lname, "packages": ["building-architectural", lpkg]])
                }
                let rpkg = "rooms/\(lname)-\(number)-ROOM"
                byRoom[rid] = [rpkg]
                roomsJSON.append(["id": rid, "number": number, "name": "ROOM", "label": "\(number) ROOM", "level": lname, "package": rpkg, "area_sf": Double(roomW * roomD),
                                  "bounds": ["min": [Double(x0), Double(y0)], "max": [Double(x0 + roomW), Double(y0 + roomD)]],
                                  "anchor": ["kind": "room_corner", "name": "\(number) corner 1", "plan_x": Double(x0), "plan_y": Double(y0), "floor_elevation_ft": Double(e)]])
                graph.append(["id": rid, "kind": "room", "group": "base", "level": lname, "packages": ["building-architectural", lpkg]])
            }
            for k in 0..<max(spec.perLevel - lnodes.count, 0) {
                let id = nid(), x = Float(k % 200) * length / 200, row = Float((k / 200) % 4)
                let isHead = k % 2 == 0
                let kind = isHead ? "fixture" : "light_fixture", group = isHead ? "mechanical" : "electrical"
                lnodes.append(Node(extras: ["id": id, "kind": kind, "group": group, "trade": isHead ? "fire_protection" : "electrical", "level": lname, "category": isHead ? "Sprinklers" : "Lighting Fixtures"],
                                   center: m(x + 0.5, roomD + 1 + row * 2, e + 9 - Float(k / 800) * 0.01), size: SIMD3(0.5, 0.3, 0.5) * ft, material: isHead ? 4 : 5))
                graph.append(["id": id, "kind": kind, "group": group, "level": lname,
                              "fed_by": isHead ? [] : [["id": levelPanel, "tag": "LP-\(lv + 1)", "circuit": "\(k % 42 + 1)", "link": k % 7 == 0 ? "inferred" : "known"]], "link": isHead ? "unknown" : (k % 7 == 0 ? "inferred" : "known")])
            }
            try writePackage(lnodes, name: lpkg, view: "floor", level: lname, room: nil, into: dir, origin: .zero, list: &packages, generic: false)
            // Each room's own package: everything in it, positioned from the room's anchor.
            for r in rooms {
                let e0 = e
                let unitPanel = nid()
                var rnodes: [Node] = []
                let origin = m(r.x0, r.y0, e0)
                rnodes.append(Node(extras: ["id": unitPanel, "kind": "equipment", "role": "panel", "group": "electrical", "trade": "electrical", "tag": "U\(r.number)", "level": lname, "room": "\(r.number) ROOM"],
                                   center: m(r.x0 + 1, r.y0 + 1, e0 + 5), size: SIMD3(1.2, 2, 0.4) * ft, material: 1))
                graph.append(["id": unitPanel, "kind": "equipment", "role": "panel", "group": "electrical", "level": lname, "tag": "U\(r.number)",
                              "fed_by": [["id": levelPanel, "tag": "LP-\(lv + 1)", "link": "known"]], "link": "known", "packages": ["rooms/\(lname)-\(r.number)-ROOM"]])
                let kinds: [(String, String, String, Int)] = [("receptacle", "electrical", "Devices", 5), ("box", "electrical", "Boxes", 5), ("conduit", "electrical", "Conduit", 5),
                                                               ("duct", "mechanical", "Duct", 2), ("pipe", "plumbing", "Pipe", 3), ("fixture", "plumbing", "Plumbing Fixtures", 3),
                                                               ("casework", "cabinets", "Casework", 6), ("furniture", "furniture", "Furniture", 6)]
                for k in 0..<spec.perRoom {
                    let (kind, group, cat, mat) = kinds[k % kinds.count]
                    let id = nid()
                    let gx = Float(k % 30) / 30, gy = Float((k / 30) % 30) / 30
                    rnodes.append(Node(extras: ["id": id, "kind": kind, "group": group, "trade": group, "level": lname, "room": "\(r.number) ROOM", "category": cat],
                                       center: m(r.x0 + 1 + gx * (roomW - 2), r.y0 + 1 + gy * (roomD - 2), e0 + 1 + Float(k % 7)), size: SIMD3(0.4, 0.4, 0.3) * ft, material: mat))
                    let fed: [[String: Any]] = group == "electrical" ? [["id": unitPanel, "tag": "U\(r.number)", "circuit": "\(k % 12 + 1)", "link": k % 11 == 0 ? "inferred" : "known"]] : []
                    graph.append(["id": id, "kind": kind, "group": group, "level": lname, "fed_by": fed, "link": fed.isEmpty ? "unknown" : (k % 11 == 0 ? "inferred" : "known"),
                                  "packages": ["rooms/\(lname)-\(r.number)-ROOM"]])
                }
                // Room package nodes are relative to the room's anchor.
                let rel = rnodes.map { n -> Node in var n = n; n.center -= origin; return n }
                try writePackage(rel, name: "rooms/\(lname)-\(r.number)-ROOM", view: "room", level: lname, room: "\(r.number) ROOM", into: dir, origin: origin, list: &roomPackages, generic: false,
                                 anchor: ["kind": "room_corner", "name": "\(r.number) corner 1", "plan_x": Double(r.x0), "plan_y": Double(r.y0), "plan_units": "ft", "level": lname, "floor_elevation_ft": Double(e0)])
            }
        }
        try writePackage(mains, name: "building-mep-mains", view: "mep", level: nil, room: nil, into: dir, origin: .zero, list: &packages, generic: true)
        try writePackage(full, name: "building-mep-full", view: "mep", level: nil, room: nil, into: dir, origin: .zero, list: &packages, generic: true)
        try writePackage(arch, name: "building-architectural", view: "architectural", level: nil, room: nil, into: dir, origin: .zero, list: &packages, generic: true)
        for i in graph.indices {
            guard let id = graph[i]["id"] as? String else { continue }
            if graph[i]["packages"] == nil { graph[i]["packages"] = graphPackages[id] ?? ["levels/\(graph[i]["level"] as? String ?? "L1")"] }
        }
        let index: [String: Any] = [
            "schema": PackageSetIndex.schema, "title": spec.title, "exported": "2026-01-01T00:00:00Z", "generator": "CADaddy Field synthetic scale set",
            "frame": ["origin_plan_ft": [0, 0], "datum_elevation_ft": 0], "units": ["models": "m", "plan": "ft", "up_axis": "Y"],
            "levels": levelsJSON, "rooms": roomsJSON, "packages": packages, "room_packages": roomPackages,
            "views": ["default": ["view": "mep", "density": "mains"], "architectural": ["packages": ["building-architectural"]],
                      "mep": ["default_density": "mains", "densities": ["mains": ["building-mep-mains"], "full": ["building-mep-mains", "building-mep-full"]]],
                      "floor": ["by_level": byLevel], "room": ["by_room": byRoom]],
            "groups": ["base": ["default_on": true, "always_on": true], "electrical": ["default_on": true], "mechanical": ["default_on": true],
                       "plumbing": ["default_on": true], "cabinets": ["default_on": true], "furniture": ["default_on": false]],
            "graph": ["elements": graph, "links": ["known": graph.filter { $0["link"] as? String == "known" }.count,
                                                   "inferred": graph.filter { $0["link"] as? String == "inferred" }.count,
                                                   "unknown": graph.filter { $0["link"] as? String == "unknown" }.count]],
            "left_out": ["pipe_size_unknown": 0],
        ]
        try JSONSerialization.data(withJSONObject: index).write(to: dir.appendingPathComponent("building.json"))
    }

    static func writePackage(_ nodes: [Node], name: String, view: String, level: String?, room: String?, into dir: URL, origin: SIMD3<Float>,
                             list: inout [[String: Any]], generic: Bool, anchor: [String: Any]? = nil) throws {
        var w = GLBWriter()
        let colors: [SIMD4<Float>] = [SIMD4(0.84, 0.83, 0.81, 1), SIMD4(0.9, 0.55, 0.2, 1), SIMD4(0.3, 0.6, 0.9, 1), SIMD4(0.3, 0.75, 0.5, 1),
                                      SIMD4(0.85, 0.25, 0.25, 1), SIMD4(0.95, 0.85, 0.5, 1), SIMD4(0.6, 0.5, 0.4, 1)]
        var meshes: [Int] = []
        for (i, c) in colors.enumerated() {
            let mat = w.addMaterial("m\(i)", c)
            meshes.append(w.addMesh(GLBWriter.box(name: "box\(i)", size: SIMD3(repeating: 1), material: mat)))
        }
        var pkg: [String: Any] = ["name": name, "view": view]
        if let level { pkg["level"] = level }
        if let room { pkg["room"] = room }
        var header: [String: Any] = ["schema": "cadaddy.ar/1", "package": pkg, "generic": generic, "field_alignment": !generic]
        if let anchor { header["anchor"] = anchor }
        w.assetExtras = ["cadaddy": header]
        for n in nodes {
            var x = n.extras
            x["node"] = (x["kind"] as? String) ?? "node"
            w.addNode(GLBWriter.Node(name: (x["kind"] as? String) ?? "node", mesh: meshes[min(n.material, meshes.count - 1)], translation: n.center, scale: simd_max(n.size, SIMD3(repeating: 0.005)), extras: x))
        }
        let data = try w.encode()
        let glbPath = "\(name).glb", sidePath = "\(name).json"
        try data.write(to: dir.appendingPathComponent(glbPath))
        var side: [String: Any] = ["schema": "cadaddy.ar/1", "version": 1, "units": "m", "up_axis": "Y", "glb": (glbPath as NSString).lastPathComponent,
                                   "model": (glbPath as NSString).lastPathComponent, "generic": generic, "field_alignment": !generic]
        side["anchor"] = anchor ?? ["kind": "building_origin", "name": "building origin", "plan_x": 0, "plan_y": 0, "plan_units": "ft"]
        let sdata = try JSONSerialization.data(withJSONObject: side)
        try sdata.write(to: dir.appendingPathComponent(sidePath))
        let sha = { (d: Data) in SHA256.hash(data: d).map { String(format: "%02x", $0) }.joined() }
        var entry: [String: Any] = ["name": name, "view": view, "generic": generic, "field_alignment": !generic, "nodes": nodes.count, "triangles": nodes.count * 12,
                                    "content_hash": sha(data), "simplified": "nothing",
                                    "files": ["glb": ["path": glbPath, "bytes": data.count, "sha256": sha(data)], "sidecar": ["path": sidePath, "bytes": sdata.count, "sha256": sha(sdata)]]]
        if let level { entry["level"] = level }
        if let anchor { entry["frame"] = ["anchor": anchor] }
        list.append(entry)
    }
}
