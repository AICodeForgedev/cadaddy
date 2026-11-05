import simd
import XCTest

final class PackageParsingTests: XCTestCase {
    func testSampleRoomParsesWithMetadataAndPositions() throws {
        let model = try GLBReader.read(data: try SamplePackage.makeGLB())
        // 7 walls + floor + ceiling + 20 devices + 2 conduit runs.
        XCTAssertEqual(model.elements.count, 31)
        XCTAssertTrue(model.warnings.isEmpty, "\(model.warnings)")
        // Repeated parts are shared: 3 unit context boxes + 8 device meshes + 1 conduit mesh.
        XCTAssertEqual(model.meshes.count, 12)
        XCTAssertEqual(model.triangleCount, 31 * 12)
        let r1 = try XCTUnwrap(model.elements.first { $0.info.tag == "R1" })
        XCTAssertEqual(r1.info.kind, "Receptacle")
        XCTAssertEqual(r1.info.circuit, "A-3")
        XCTAssertEqual(r1.info.panel, "PANEL-A")
        XCTAssertEqual(r1.info.catalog, "SAMPLE-DUPLEX-20A")
        XCTAssertEqual(r1.info.trade, "electrical")
        XCTAssertEqual(r1.info.source, "synthetic sample")
        XCTAssertTrue(r1.info.id.hasPrefix("E-"))
        XCTAssertEqual(try XCTUnwrap(r1.info.mountingHeight), 0.4572, accuracy: 1e-6)
        XCTAssertNear(r1.origin, SIMD3(0.02, 0.4572, -3.5), 0.001)
        XCTAssertFalse(r1.info.isContext)
        let wall = try XCTUnwrap(model.elements.first { $0.info.kind == "Wall" })
        XCTAssertTrue(wall.info.isContext)
        // The anchor is at the origin and the floor top at y = 0.
        let floor = try XCTUnwrap(model.elements.first { $0.info.kind == "Floor" })
        XCTAssertEqual(try XCTUnwrap(floor.boundsMax).y, 0, accuracy: 1e-5)
        XCTAssertNear(try XCTUnwrap(model.boundsMin) * SIMD3(1, 0, 0), SIMD3(-0.15, 0, 0), 0.001)
        XCTAssertEqual(model.embeddedSidecar?.anchor.kind, "room_corner")
        XCTAssertEqual(Set(model.trades), ["context", "electrical", "fire-alarm", "low-voltage", "mechanical"])
        // A rotated device: its plate faces into the room.
        let r4 = try XCTUnwrap(model.elements.first { $0.info.tag == "R4" })
        XCTAssertNear(r4.origin, SIMD3(3.98, 0.4572, -4.0), 0.001)
    }

    func testSnapPointsIncludeAnchorAndRoomCorners() throws {
        let model = try GLBReader.read(data: try SamplePackage.makeGLB())
        let pts = model.snapPoints()
        XCTAssertEqual(pts.first?.position, .zero)
        // The far interior corner of the window wall and left wall is (0, 0, -7.6).
        XCTAssertTrue(pts.contains { simd_length($0.position - SIMD3(0, 0, -7.6)) < 0.01 })
        XCTAssertTrue(pts.allSatisfy { $0.position.y == 0 })
    }

    func testLargeSampleLoads() throws {
        let model = try GLBReader.read(data: try SamplePackage.makeGLB(rooms: 40))
        XCTAssertEqual(model.elements.count, 31 * 40)
        XCTAssertEqual(model.meshes.count, 12)
        XCTAssertEqual(Set(model.elements.map(\.info.id)).count, 31 * 40, "ids are unique")
    }

    func testMatrixTransformHierarchyAndElementGrouping() throws {
        // Element node (with extras) at x=2, its child geometry node offset 1 m in y.
        let nodes: [[String: Any]] = [
            ["name": "Panel_P1", "matrix": [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 2, 0, 0, 1], "children": [1],
             "extras": ["cadaddy": ["id": "E-9", "kind": "Panel", "tag": "P1", "mounting_height": "6' 6\""]]],
            ["name": "geom", "mesh": 0, "translation": [0, 1, 0]],
        ]
        let model = try GLBReader.read(data: try TestSupport.triangleGLB(nodes: nodes, scenes: [0]))
        XCTAssertEqual(model.elements.count, 1)
        let e = model.elements[0]
        XCTAssertEqual(e.info.id, "E-9")
        XCTAssertEqual(try XCTUnwrap(e.info.mountingHeight), 1.9812, accuracy: 1e-6)
        XCTAssertNear(e.origin, SIMD3(2, 0, 0))
        XCTAssertNear(try XCTUnwrap(e.boundsMin), SIMD3(2, 1, 0))
        XCTAssertNear(try XCTUnwrap(e.boundsMax), SIMD3(3, 2, 0))
    }

    func testGPUInstancingExpands() throws {
        let pos = TestSupport.floats([0, 0, 0, 1, 0, 0, 0, 1, 0])
        let trans = TestSupport.floats([0, 0, 0, 5, 0, 0, 10, 0, 0])
        let root: [String: Any] = [
            "asset": ["version": "2.0"], "extensionsUsed": ["EXT_mesh_gpu_instancing"], "extensionsRequired": ["EXT_mesh_gpu_instancing"],
            "buffers": [["byteLength": pos.count + trans.count]],
            "bufferViews": [["buffer": 0, "byteLength": pos.count], ["buffer": 0, "byteOffset": pos.count, "byteLength": trans.count]],
            "accessors": [["bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3"], ["bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC3"]],
            "meshes": [["primitives": [["attributes": ["POSITION": 0]]]]],
            "nodes": [["name": "Strap", "mesh": 0, "extensions": ["EXT_mesh_gpu_instancing": ["attributes": ["TRANSLATION": 1]]]]],
        ]
        let model = try GLBReader.read(data: try TestSupport.glb(root, bin: pos + trans))
        XCTAssertEqual(model.elements.count, 1)
        XCTAssertEqual(model.elements[0].parts.count, 3)
        XCTAssertNear(try XCTUnwrap(model.elements[0].boundsMax), SIMD3(11, 1, 0))
    }

    // MARK: hostile input — refused with a message, never a crash

    func testMalformedContainersRefused() throws {
        XCTAssertThrowsFieldError(try GLBReader.read(data: Data()), contains: "too short")
        XCTAssertThrowsFieldError(try GLBReader.read(data: Data(repeating: 0x41, count: 64)), contains: "not a glTF")
        var good = try SamplePackage.makeGLB()
        XCTAssertThrowsFieldError(try GLBReader.read(data: good.prefix(40)), contains: "truncated")
        // Version 1.
        good.replaceSubrange(4..<8, with: [1, 0, 0, 0])
        XCTAssertThrowsFieldError(try GLBReader.read(data: good), contains: "2.0")
        // A JSON chunk that claims to run past the end.
        var bad = try SamplePackage.makeGLB()
        bad.replaceSubrange(12..<16, with: [0xFF, 0xFF, 0xFF, 0x7F])
        XCTAssertThrowsFieldError(try GLBReader.read(data: bad), contains: "past the end")
        XCTAssertThrowsFieldError(try GLBReader.read(data: try TestSupport.glb([:]).prefix(20) + Data(count: 8)))
        // JSON that is not an object, and invalid JSON.
        var arr = Data("[1,2,3] ".utf8)
        var raw = Data()
        func u32(_ v: UInt32) { withUnsafeBytes(of: v.littleEndian) { raw.append(contentsOf: $0) } }
        u32(0x4654_6C67); u32(2); u32(UInt32(20 + arr.count)); u32(UInt32(arr.count)); u32(0x4E4F_534A); raw.append(arr)
        XCTAssertThrowsFieldError(try GLBReader.read(data: raw), contains: "not an object")
        arr = Data("{nope    ".utf8)
        raw = Data(); u32(0x4654_6C67); u32(2); u32(UInt32(20 + arr.count)); u32(UInt32(arr.count)); u32(0x4E4F_534A); raw.append(arr)
        XCTAssertThrowsFieldError(try GLBReader.read(data: raw), contains: "not valid")
    }

    func testHostileGraphRefused() throws {
        // Cycle: 0 -> 1 -> 0.
        let cyc = try TestSupport.triangleGLB(nodes: [["mesh": 0, "children": [1]], ["mesh": 0, "children": [0]]], scenes: [0])
        XCTAssertThrowsFieldError(try GLBReader.read(data: cyc), contains: "cycle")
        // 100-deep chain.
        var chain: [[String: Any]] = (0..<100).map { ["mesh": 0, "children": [$0 + 1]] }
        chain.append(["mesh": 0])
        XCTAssertThrowsFieldError(try GLBReader.read(data: try TestSupport.triangleGLB(nodes: chain, scenes: [0])), contains: "deeply")
        // External buffer.
        let ext = try TestSupport.glb(["asset": ["version": "2.0"], "buffers": [["uri": "../../secret.bin", "byteLength": 4]]])
        XCTAssertThrowsFieldError(try GLBReader.read(data: ext), contains: "external")
        // Required extension we can't read.
        let req = try TestSupport.triangleGLB(nodes: [["mesh": 0]], extra: ["extensionsRequired": ["KHR_draco_mesh_compression"]])
        XCTAssertThrowsFieldError(try GLBReader.read(data: req), contains: "extensions")
        // Too many nodes.
        let many = try TestSupport.glb(["asset": ["version": "2.0"], "nodes": Array(repeating: [String: Any](), count: FieldLimits.maxNodes + 1)])
        XCTAssertThrowsFieldError(try GLBReader.read(data: many), contains: "too many nodes")
    }

    func testBadGeometryIsSkippedNotTrusted() throws {
        // Out-of-range index: primitive skipped with a warning, nothing to show.
        let oob = try GLBReader.read(data: try TestSupport.triangleGLB(nodes: [["mesh": 0]], indices: [0, 1, 99]))
        XCTAssertTrue(oob.elements.isEmpty)
        XCTAssertTrue(oob.warnings.contains { $0.contains("out-of-range indices") })
        // NaN translation: identity used, warned.
        let nan = try GLBReader.read(data: try TestSupport.triangleGLB(nodes: [["mesh": 0, "translation": [0, 1, 2]]]))
        XCTAssertEqual(nan.elements.count, 1)
        // JSON can't carry NaN, so check a huge scale (non-finite after Float conversion) instead.
        let inf = try GLBReader.read(data: try TestSupport.triangleGLB(nodes: [["mesh": 0, "scale": [1e300, 1, 1]]]))
        XCTAssertTrue(inf.warnings.contains { $0.contains("not finite") })
        XCTAssertNear(try XCTUnwrap(inf.elements.first?.boundsMax), SIMD3(1, 1, 0))
        // An accessor that claims more vertices than its buffer holds.
        let pos = TestSupport.floats([0, 0, 0, 1, 0, 0, 0, 1, 0])
        let lying: [String: Any] = [
            "asset": ["version": "2.0"], "buffers": [["byteLength": pos.count]], "bufferViews": [["buffer": 0, "byteLength": pos.count]],
            "accessors": [["bufferView": 0, "componentType": 5126, "count": 10_000_000, "type": "VEC3"]],
            "meshes": [["primitives": [["attributes": ["POSITION": 0]]]]], "nodes": [["mesh": 0]],
        ]
        let l = try GLBReader.read(data: try TestSupport.glb(lying, bin: pos))
        XCTAssertTrue(l.elements.isEmpty)
        // Bad references everywhere.
        let refs = try GLBReader.read(data: try TestSupport.triangleGLB(nodes: [["mesh": 7, "children": [42, -1]]], scenes: [0, 99]))
        XCTAssertTrue(refs.elements.isEmpty)
    }

    func testRandomBytesNeverCrash() {
        var rng = SystemRandomNumberGenerator()
        let base = (try? SamplePackage.makeGLB()) ?? Data()
        for i in 0..<300 {
            var d = base
            // Flip a handful of random bytes (header, JSON or BIN).
            for _ in 0..<(1 + i % 8) {
                let at = Int.random(in: 0..<d.count, using: &rng)
                d[at] = UInt8.random(in: 0...255, using: &rng)
            }
            _ = try? GLBReader.read(data: d)
            _ = try? ZipReader(data: d)
        }
        for n in [0, 1, 7, 20, 64, 4096] {
            let junk = Data((0..<n).map { _ in UInt8.random(in: 0...255, using: &rng) })
            _ = try? GLBReader.read(data: junk)
            _ = try? ZipReader(data: junk)
            _ = try? Sidecar.parse(junk)
        }
    }

    // MARK: sidecar

    func testSidecarParsesSampleAndLenientKeys() throws {
        let dir = try TestSupport.tempDir()
        try SamplePackage.write(to: dir)
        let s = try Sidecar.parse(Data(contentsOf: dir.appendingPathComponent("Sample Guest Room.json")))
        XCTAssertEqual(s.anchor.kind, "room_corner")
        XCTAssertEqual(s.anchor.planX, 0)
        XCTAssertEqual(s.anchor.level, "Level 1")
        XCTAssertEqual(s.title, "Sample Guest Room")
        XCTAssertEqual(s.points.count, 3)
        XCTAssertEqual(s.modelFile, "Sample Guest Room.glb")

        let lenient = try Sidecar.parse(Data("""
        {"cadaddy": {"anchor": {"type": "Grid Intersection", "name": "B/3", "plan": [120.5, -33.25], "storey": "L2"},
         "units": "ft", "points": [{"name": "C/3", "xyz": [10, 0, 0]}],
         "elements": [{"element_id": "E-1", "kind": "Receptacle", "node_name": "Receptacle R1", "mountingHeight": "18\\"", "panel": "P"}]}}
        """.utf8))
        XCTAssertEqual(lenient.anchor.kind, "grid_intersection")
        XCTAssertEqual(lenient.anchor.planX, 120.5)
        XCTAssertEqual(lenient.anchor.planY, -33.25)
        XCTAssertEqual(lenient.anchor.level, "L2")
        XCTAssertEqual(lenient.metersPerUnit, 0.3048, accuracy: 1e-12)
        XCTAssertEqual(try XCTUnwrap(lenient.points.first).position.x, 3.048, accuracy: 1e-5)
        XCTAssertEqual(lenient.elements.first?.id, "E-1")
        XCTAssertEqual(try XCTUnwrap(lenient.elements.first?.mountingHeight), 0.4572, accuracy: 1e-9)
    }

    func testSidecarRefusals() {
        XCTAssertThrowsFieldError(try Sidecar.parse(Data("not json".utf8)), contains: "not valid JSON")
        XCTAssertThrowsFieldError(try Sidecar.parse(Data("[1]".utf8)), contains: "not a JSON object")
        XCTAssertThrowsFieldError(try Sidecar.parse(Data(#"{"up_axis": "Z", "anchor": {}}"#.utf8)), contains: "up axis")
        XCTAssertThrowsFieldError(try Sidecar.parse(Data(#"{"meters_per_unit": -1}"#.utf8)), contains: "unit scale")
        // A JSON true is not a height of 1 m; 0/1 numbers are.
        let s = try? Sidecar.parse(Data(#"{"elements": [{"id": "a", "kind": "X", "mounting_height": true}, {"id": "b", "kind": "X", "mounting_height": 1}]}"#.utf8))
        XCTAssertNil(s?.elements.first?.mountingHeight)
        XCTAssertEqual(s?.elements.last?.mountingHeight, 1)
    }

    func testSidecarRowsFillUSDStyleNames() {
        var model = FieldModel(meshes: [], elements: [
            FieldElement(index: 0, name: "Receptacle_R1", info: ElementInfo.fromNodeName("Receptacle_R1"), transform: matrix_identity_float4x4, parts: []),
        ])
        let sc = Sidecar(anchor: AnchorInfo(kind: "room_corner"), elements: [
            ElementInfo(id: "E-77", kind: "Receptacle", tag: "R1", circuit: "A-3", mountingHeight: 0.4572, nodeName: "Receptacle R1"),
        ])
        model.merge(sidecar: sc)
        XCTAssertEqual(model.elements[0].info.id, "E-77")
        XCTAssertEqual(model.elements[0].info.circuit, "A-3")
        XCTAssertEqual(model.elements[0].info.mountingHeight, 0.4572)
    }
}
