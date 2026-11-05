import simd
import XCTest

final class MarksAndLibraryTests: XCTestCase {
    var dir: URL!
    var lib: PackageLibrary!

    override func setUpWithError() throws {
        dir = try TestSupport.tempDir()
        lib = try PackageLibrary(root: dir.appendingPathComponent("lib"))
    }

    override func tearDownWithError() throws { try? FileManager.default.removeItem(at: dir) }

    func sampleFolder() throws -> URL {
        try SamplePackage.write(to: dir.appendingPathComponent("pkg"))
    }

    // MARK: marks JSON

    func testMarksExportRoundTrip() throws {
        let pose = Mark.flatten(YawAlignment(yaw: 0.4, translation: SIMD3(1, 1.5, -2)).matrix)
        let marks = MarkStatus.allCases.enumerated().map { i, s in
            Mark(elementID: "E-\(i)", elementKind: "Receptacle", elementTag: "R\(i)", status: s, note: i == 0 ? "cover plate missing" : nil,
                 photo: i == 1 ? "p.jpg" : nil, time: Date(timeIntervalSince1970: 1_800_000_000.125 + Double(i)),
                 devicePoseWorld: pose, devicePoseModel: pose, pointWorld: s == .moved ? [1, 0.45, -2] : nil, pointModel: s == .moved ? [0.5, 0.45, -3] : nil)
        }
        let export = MarksExport(app: "CADaddy Field 0.1.0", exported: Date(timeIntervalSince1970: 1_800_000_100.5),
                                 package: .init(name: "Sample Guest Room", modelFile: "model.glb", modelSHA256: "ab12",
                                                anchor: AnchorInfo(kind: "room_corner", name: "corner", planX: 0, planY: 0)),
                                 alignment: .init(method: "two_point", residualInches: 0.42),
                                 marks: marks.map { MarksExport.Entry(mark: $0, photo: $0.photo == nil ? nil : MarkPhoto(name: "p.jpg", mime: "image/jpeg", dataBase64: "AAEC")) })
        let data = try export.encoded()
        let back = try MarksExport.decode(data)
        XCTAssertEqual(back, export)
        // The documented field names are what is written.
        let obj = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        XCTAssertEqual(obj["schema"] as? String, "cadaddy-field-marks")
        XCTAssertEqual(obj["version"] as? Int, 1)
        let first = try XCTUnwrap((obj["marks"] as? [[String: Any]])?.first)
        for key in ["id", "element_id", "element_kind", "element_tag", "status", "note", "time", "device_pose_world", "device_pose_model"] {
            XCTAssertNotNil(first[key], key)
        }
        XCTAssertEqual(first["time"] as? String, "2027-01-15T08:00:00.125Z")
        let moved = try XCTUnwrap((obj["marks"] as? [[String: Any]])?.first { $0["status"] as? String == "moved" })
        XCTAssertEqual((moved["point_model"] as? [Double])?.count, 3)
        let withPhoto = try XCTUnwrap((obj["marks"] as? [[String: Any]])?.first { $0["photo_data"] != nil })
        XCTAssertEqual((withPhoto["photo_data"] as? [String: Any])?["data_base64"] as? String, "AAEC")
        XCTAssertEqual(((obj["package"] as? [String: Any])?["anchor"] as? [String: Any])?["plan_x"] as? Double, 0)
        XCTAssertEqual((obj["alignment"] as? [String: Any])?["residual_in"] as? Double, 0.42)
    }

    func testMarksDecodeRefusesOtherFiles() throws {
        XCTAssertThrowsFieldError(try MarksExport.decode(Data("{}".utf8)), contains: "not a CADaddy Field marks")
        var x = MarksExport(app: "a", package: .init(name: "n", modelFile: "m", modelSHA256: "s", anchor: nil), alignment: nil, marks: [])
        x.schema = "something-else"
        XCTAssertThrowsFieldError(try MarksExport.decode(try x.encoded()), contains: "not a CADaddy Field marks")
        x.schema = MarksExport.schemaID
        x.version = 99
        XCTAssertThrowsFieldError(try MarksExport.decode(try x.encoded()), contains: "newer version")
    }

    func testMarkTimeIsWholeMilliseconds() {
        let t = Mark.now().timeIntervalSince1970 * 1000
        XCTAssertEqual(t, t.rounded(.down), accuracy: 1e-3)
    }

    // MARK: library

    func testImportFolderStoresPackageLocally() throws {
        let r = try lib.importItem(at: try sampleFolder())
        XCTAssertEqual(r.name, "Sample Guest Room")
        XCTAssertEqual(r.format, .glb)
        XCTAssertEqual(r.elementCount, 31)
        XCTAssertEqual(r.anchor?.kind, "room_corner")
        XCTAssertEqual(r.sha256.count, 64)
        XCTAssertEqual(lib.list().map(\.id), [r.id])
        XCTAssertTrue(FileManager.default.fileExists(atPath: lib.modelURL(r).path))
        XCTAssertNotNil(lib.sidecarURL(r))
        let excluded = try lib.root.resourceValues(forKeys: [.isExcludedFromBackupKey]).isExcludedFromBackup
        XCTAssertEqual(excluded, true, "packages stay on the device")
        let (model, sidecar) = try lib.loadModel(r)
        XCTAssertEqual(model.elements.count, 31)
        XCTAssertEqual(sidecar.points.count, 3)
    }

    func testImportSingleGLBFindsSiblingSidecar() throws {
        let f = try sampleFolder()
        let r = try lib.importItem(at: f.appendingPathComponent("Sample Guest Room.glb"))
        XCTAssertNotNil(r.sidecarFile)
        XCTAssertEqual(r.anchor?.name, "Room corner nearest the entry")
    }

    func testImportZipStoredAndDeflated() throws {
        let f = try sampleFolder()
        let glb = try Data(contentsOf: f.appendingPathComponent("Sample Guest Room.glb"))
        let json = try Data(contentsOf: f.appendingPathComponent("Sample Guest Room.json"))
        for deflate in [false, true] {
            let z = TestSupport.zip([.init(name: "export/", data: Data()), .init(name: "export/Sample Guest Room.glb", data: glb, deflate: deflate),
                                     .init(name: "export/Sample Guest Room.json", data: json, deflate: deflate),
                                     .init(name: "__MACOSX/._x", data: Data([1, 2]))])
            let zurl = dir.appendingPathComponent("p\(deflate).zip")
            try z.write(to: zurl)
            let r = try lib.importItem(at: zurl)
            XCTAssertEqual(r.elementCount, 31)
            XCTAssertEqual(r.anchor?.kind, "room_corner")
        }
    }

    func testZipSlipAndDamageRefused() throws {
        XCTAssertNil(ZipReader.safeName("../evil.glb"))
        XCTAssertNil(ZipReader.safeName("/abs/evil.glb"))
        XCTAssertNil(ZipReader.safeName("a/../../b.glb"))
        XCTAssertNil(ZipReader.safeName("a\\..\\b.glb"))
        XCTAssertNil(ZipReader.safeName("__MACOSX/a.glb"))
        XCTAssertEqual(ZipReader.safeName("export\\room.glb"), "export/room.glb")
        // Only an escaping entry: nothing usable inside.
        let glb = try SamplePackage.makeGLB()
        let evil = dir.appendingPathComponent("evil.zip")
        try TestSupport.zip([.init(name: "../evil.glb", data: glb)]).write(to: evil)
        XCTAssertThrowsFieldError(try lib.importItem(at: evil), contains: "no .glb")
        // Truncated archive.
        let cut = dir.appendingPathComponent("cut.zip")
        try TestSupport.zip([.init(name: "a.glb", data: glb)]).prefix(100).write(to: cut)
        XCTAssertThrowsFieldError(try lib.importItem(at: cut))
        // Encrypted flag.
        var enc = TestSupport.zip([.init(name: "a.glb", data: glb)])
        let cd = try XCTUnwrap(enc.range(of: Data([0x50, 0x4B, 0x01, 0x02])))
        enc[cd.lowerBound + 8] = 1
        XCTAssertThrowsFieldError(try ZipReader(data: enc), contains: "Encrypted")
        XCTAssertTrue(lib.list().isEmpty)
    }

    func testMalformedPackagesRefusedWithoutLeftovers() throws {
        let bad = dir.appendingPathComponent("broken.glb")
        try Data("glTF but not really".utf8).write(to: bad)
        XCTAssertThrowsFieldError(try lib.importItem(at: bad), contains: "glTF")
        let usdz = dir.appendingPathComponent("broken.usdz")
        try Data(repeating: 7, count: 100).write(to: usdz)
        XCTAssertThrowsFieldError(try lib.importItem(at: usdz), contains: "USDZ")
        let json = dir.appendingPathComponent("only.json")
        try Data("{}".utf8).write(to: json)
        XCTAssertThrowsFieldError(try lib.importItem(at: json), contains: "not the .json alone")
        let txt = dir.appendingPathComponent("x.txt")
        try Data().write(to: txt)
        XCTAssertThrowsFieldError(try lib.importItem(at: txt))
        // A good model with a broken sidecar is refused, not half-imported.
        let f = try sampleFolder()
        try Data("{broken".utf8).write(to: f.appendingPathComponent("Sample Guest Room.json"))
        XCTAssertThrowsFieldError(try lib.importItem(at: f), contains: "not valid JSON")
        XCTAssertThrowsFieldError(try lib.importItem(at: dir.appendingPathComponent("missing.glb")), contains: "could not be found")
        XCTAssertTrue(lib.list().isEmpty)
        let leftovers = try FileManager.default.contentsOfDirectory(atPath: lib.root.path)
        XCTAssertTrue(leftovers.isEmpty, "\(leftovers)")
    }

    func testUSDZWithoutSidecarImportsWithNote() throws {
        // A USDZ is a zip whose first file is the root layer.
        let usdz = TestSupport.zip([.init(name: "root.usda", data: Data("#usda 1.0\n".utf8))])
        let url = dir.appendingPathComponent("room.usdz")
        try usdz.write(to: url)
        let r = try lib.importItem(at: url)
        XCTAssertEqual(r.format, .usdz)
        XCTAssertTrue(r.warnings.contains { $0.contains("USDZ without a sidecar") })
    }

    func testMarksPersistAndExportWithPhoto() throws {
        let r = try lib.importItem(at: try sampleFolder())
        let photo = try lib.savePhoto(Data([0xFF, 0xD8, 0xFF, 0xD9]), for: r)
        let m1 = Mark(elementID: "E-1001", status: .installed, note: "ok", photo: photo)
        let m2 = Mark(elementID: "E-1002", status: .moved, pointWorld: [1, 0, 1], pointModel: [0.2, 0, -1])
        try lib.saveMarks([m1, m2], for: r)
        XCTAssertEqual(lib.marks(r), [m1, m2])
        let url = try lib.exportMarks(r, app: "CADaddy Field test")
        let x = try MarksExport.decode(Data(contentsOf: url))
        XCTAssertEqual(x.marks.map(\.mark), [m1, m2])
        XCTAssertEqual(x.marks.first?.photo?.dataBase64, Data([0xFF, 0xD8, 0xFF, 0xD9]).base64EncodedString())
        XCTAssertEqual(x.package.modelSHA256, r.sha256)
        XCTAssertThrowsFieldError(try lib.savePhoto(Data(count: FieldLimits.maxPhotoBytes + 1), for: r), contains: "too large")
        try lib.delete(r.id)
        XCTAssertTrue(lib.list().isEmpty)
    }
}
