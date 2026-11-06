import CryptoKit
import XCTest

/// A local "office server" for the tests: answers from a table of paths, or fails like a missing network.
final class StubServer: URLProtocol {
    struct Route { var status: Int = 200; var body: Data = Data(); var error: URLError.Code? }
    nonisolated(unsafe) static var routes: [String: Route] = [:]
    nonisolated(unsafe) static var requests: [String] = []
    static let lock = NSLock()

    static func reset() { lock.lock(); routes = [:]; requests = []; lock.unlock() }
    static func set(_ path: String, _ r: Route) { lock.lock(); routes[path] = r; lock.unlock() }
    static func served() -> [String] { lock.lock(); defer { lock.unlock() }; return requests }

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        guard let url = request.url else { return }
        let path = url.path
        Self.lock.lock(); Self.requests.append(path); let r = Self.routes[path]; Self.lock.unlock()
        if let code = r?.error {
            client?.urlProtocol(self, didFailWithError: URLError(code)); return
        }
        let route = r ?? Route(status: 404, body: Data("not found".utf8))
        let resp = HTTPURLResponse(url: url, statusCode: route.status, httpVersion: "HTTP/1.1", headerFields: nil)!
        client?.urlProtocol(self, didReceive: resp, cacheStoragePolicy: .notAllowed)
        // In pieces, like a real transfer.
        var i = 0
        while i < route.body.count {
            let end = min(i + 4096, route.body.count)
            client?.urlProtocol(self, didLoad: route.body.subdata(in: i..<end))
            i = end
        }
        client?.urlProtocolDidFinishLoading(self)
    }

    override func stopLoading() {}
}

final class UpdateTests: XCTestCase {
    var dir: URL!
    var lib: PackageLibrary!
    var updater: PackageUpdater!
    let base = "http://192.168.1.20:8000/tok3n"

    override func setUpWithError() throws {
        dir = try TestSupport.tempDir()
        lib = try PackageLibrary(root: dir.appendingPathComponent("lib"))
        let c = URLSessionConfiguration.ephemeral
        c.protocolClasses = [StubServer.self]
        updater = PackageUpdater(library: lib, configuration: c)
        StubServer.reset()
    }

    override func tearDownWithError() throws { try? FileManager.default.removeItem(at: dir); StubServer.reset() }

    // MARK: fixtures (made-up rooms only)

    static func hex(_ d: Data) -> String { SHA256.hash(data: d).map { String(format: "%02x", $0) }.joined() }

    /// A GLB with one triangle node per element id.
    static func model(_ ids: [String]) throws -> Data {
        try TestSupport.triangleGLB(nodes: ids.enumerated().map { i, id in
            ["mesh": 0, "name": "Receptacle_R\(i)", "translation": [Double(i), 0.4, -1.0],
             "extras": ["id": id, "kind": "Receptacle", "tag": "R\(i)", "trade": "electrical"]]
        })
    }

    static func sidecar(anchorX: Double = 10, anchorY: Double = 5, model: String = "room-a.glb") throws -> Data {
        try JSONSerialization.data(withJSONObject: [
            "schema": "cadaddy.ar/1", "version": 1, "units": "m", "up_axis": "Y", "model": model, "glb": model,
            "anchor": ["kind": "room_corner", "name": "Sample corner 1", "plan_x": anchorX, "plan_y": anchorY, "plan_units": "ft", "level": "Level 1"],
        ], options: [.sortedKeys])
    }

    /// Serves one package `room-a` (and optionally more) and its manifest.
    func serve(_ packages: [(name: String, model: Data, sidecar: Data)], manifestExtra: [String: Any] = [:], fileOverride: [String: Any] = [:]) throws {
        var list: [[String: Any]] = []
        for p in packages {
            StubServer.set("/tok3n/\(p.name).glb", .init(body: p.model))
            StubServer.set("/tok3n/\(p.name).json", .init(body: p.sidecar))
            var glb: [String: Any] = ["name": "\(p.name).glb", "role": "glb", "bytes": p.model.count, "sha256": Self.hex(p.model)]
            for (k, v) in fileOverride { glb[k] = v }
            list.append(["name": p.name, "label": "Label \(p.name)", "level": "Level 1", "exported": "2026-11-06T04:00:00Z",
                         "files": [glb, ["name": "\(p.name).json", "role": "sidecar", "bytes": p.sidecar.count, "sha256": Self.hex(p.sidecar)]]])
        }
        var m: [String: Any] = ["format": "cadaddy-ar-manifest", "version": 1, "generated": "2026-11-06T04:00:01Z", "packages": list]
        for (k, v) in manifestExtra { m[k] = v }
        StubServer.set("/tok3n/manifest.json", .init(body: try JSONSerialization.data(withJSONObject: m)))
    }

    func run() async throws -> UpdateReport { try await updater.run(address: base + "/") }

    func onlyRecord() throws -> PackageRecord { try XCTUnwrap(lib.list().first) }

    func folderSnapshot(_ r: PackageRecord) throws -> [String: Data] {
        let f = lib.folder(r.id)
        var out: [String: Data] = [:]
        for name in try FileManager.default.subpathsOfDirectory(atPath: f.path) {
            var isDir: ObjCBool = false
            let u = f.appendingPathComponent(name)
            if FileManager.default.fileExists(atPath: u.path, isDirectory: &isDir), !isDir.boolValue { out[name] = try Data(contentsOf: u) }
        }
        return out
    }

    // MARK: manifest parsing

    func testManifestParsesAndPicksGLBThenSidecar() throws {
        let sha = String(repeating: "a", count: 64)
        let m = try UpdateManifest.parse(JSONSerialization.data(withJSONObject: [
            "format": "cadaddy-ar-manifest", "version": 1,
            "packages": [["name": "sub/room-a", "label": "Room A", "level": "Level 2", "exported": "2026-11-06T04:00:00Z",
                          "files": [["name": "sub/room-a.usdz", "role": "usdz", "bytes": 10, "sha256": sha],
                                    ["name": "sub/room-a.glb", "bytes": 12, "sha256": sha.uppercased()],
                                    ["name": "sub/room-a.json", "bytes": 3, "sha256": sha]]]],
        ]))
        XCTAssertEqual(m.packages.count, 1)
        let p = m.packages[0]
        XCTAssertEqual(p.label, "Room A"); XCTAssertEqual(p.level, "Level 2"); XCTAssertEqual(p.exported, "2026-11-06T04:00:00Z")
        XCTAssertEqual(p.model?.name, "sub/room-a.glb", "glb preferred, role from the extension")
        XCTAssertEqual(p.model?.sha256, sha, "hash lower-cased")
        XCTAssertEqual(p.sidecar?.name, "sub/room-a.json")
        XCTAssertEqual(p.baseName, "room-a")
    }

    func testManifestRefusals() throws {
        func data(_ o: Any) throws -> Data { try JSONSerialization.data(withJSONObject: o) }
        XCTAssertThrowsFieldError(try UpdateManifest.parse(Data("not json".utf8)), contains: "not valid JSON")
        XCTAssertThrowsFieldError(try UpdateManifest.parse(Data("[1]".utf8)), contains: "not a JSON object")
        XCTAssertThrowsFieldError(try UpdateManifest.parse(try data(["format": "something-else", "version": 1, "packages": []])), contains: "doesn't serve")
        XCTAssertThrowsFieldError(try UpdateManifest.parse(try data(["format": "cadaddy-ar-manifest", "version": 9, "packages": []])), contains: "newer")
        XCTAssertThrowsFieldError(try UpdateManifest.parse(try data(["format": "cadaddy-ar-manifest", "version": 1])), contains: "no package list")
        XCTAssertThrowsFieldError(try UpdateManifest.parse(Data(count: Int(FieldLimits.maxManifestBytes) + 1)), contains: "too large")
        let many = Array(repeating: ["name": "x"], count: FieldLimits.maxManifestPackages + 1)
        XCTAssertThrowsFieldError(try UpdateManifest.parse(try data(["format": "cadaddy-ar-manifest", "version": 1, "packages": many])), contains: "too many")
    }

    func testHostilePackageEntriesAreLeftOutAndReported() throws {
        let sha = String(repeating: "b", count: 64)
        func pkg(_ name: Any, _ file: Any, bytes: Any = 10, hash: Any = sha) -> [String: Any] {
            ["name": name, "files": [["name": file, "bytes": bytes, "sha256": hash]]]
        }
        let bad: [[String: Any]] = [
            pkg("../escape", "x.glb"), pkg("/abs", "x.glb"), pkg("ok1", "../../etc/x.glb"), pkg("ok2", "/etc/x.glb"),
            pkg("ok3", "a\\b.glb"), pkg("ok4", "a/./b.glb"), pkg("ok5", "a//b.glb"), pkg("ok6", "%2e%2e/x.glb"), pkg("ok7", "c:x.glb"),
            pkg("ok8", "x.glb", bytes: -1), pkg("ok9", "x.glb", bytes: FieldLimits.maxModelBytes + 1), pkg("ok10", "x.glb", bytes: true),
            pkg("ok11", "x.glb", bytes: "10"), pkg("ok12", "x.glb", hash: "abc"), pkg("ok13", "x.glb", hash: String(repeating: "z", count: 64)),
            pkg("ok14", "x.txt"), pkg(7, "x.glb"), ["name": "nofiles"], pkg("ok15", "x.json", bytes: FieldLimits.maxSidecarBytes + 1),
            ["name": "dup", "files": [["name": "d.glb", "bytes": 1, "sha256": sha]]], ["name": "dup", "files": [["name": "d.glb", "bytes": 1, "sha256": sha]]],
        ]
        let m = try UpdateManifest.parse(JSONSerialization.data(withJSONObject: ["format": "cadaddy-ar-manifest", "version": 1, "packages": bad]))
        XCTAssertEqual(m.packages.map(\.name), ["dup"], "only the first well-formed entry survives")
        XCTAssertEqual(m.problems.count, bad.count - 1, m.problems.joined(separator: "\n"))
    }

    // MARK: address

    func testAddressIsLocalNetworkOnlyAndKeepsItsPathToken() throws {
        XCTAssertEqual(try UpdateServer.manifestURL("http://192.168.1.20:8000/tok3n").absoluteString, "http://192.168.1.20:8000/tok3n/manifest.json")
        XCTAssertEqual(try UpdateServer.manifestURL(" http://10.0.0.5/a b/ ").absoluteString, "http://10.0.0.5/a%20b/manifest.json")
        XCTAssertEqual(try UpdateServer.manifestURL("https://office-pc.local/x/manifest.json").absoluteString, "https://office-pc.local/x/manifest.json")
        XCTAssertEqual(try UpdateServer.manifestURL("http://officepc:8080").absoluteString, "http://officepc:8080/manifest.json")
        for ok in ["172.16.0.1", "172.31.255.255", "127.0.0.1", "169.254.3.4", "100.100.1.2", "[fe80::1]", "[fd12::5]", "[::1]", "nas.home.arpa"] {
            XCTAssertNoThrow(try UpdateServer.manifestURL("http://\(ok)/"), ok)
        }
        for bad in ["", "192.168.1.20/x", "ftp://192.168.1.20/", "file:///etc/", "http://example.com/", "http://8.8.8.8/", "http://172.32.0.1/",
                    "http://192.169.0.1/", "http://user:pw@192.168.1.20/", "http://[2001:db8::1]/", "http://300.1.1.1/", "javascript:alert(1)"] {
            XCTAssertThrowsFieldError(try UpdateServer.manifestURL(bad))
        }
        let m = try UpdateServer.manifestURL("http://192.168.1.20:8000/tok3n/")
        XCTAssertEqual(UpdateServer.fileURL(manifest: m, name: "sub/Room #2?.glb")?.absoluteString, "http://192.168.1.20:8000/tok3n/sub/Room%20%232%3F.glb")
        XCTAssertNil(UpdateServer.fileURL(manifest: m, name: "../x.glb"))
    }

    // MARK: the diff

    func testPlanMatchesByStoredSourceNameAndHashes() throws {
        let sha = String(repeating: "c", count: 64), side = String(repeating: "d", count: 64)
        func rec(_ src: String, remote: String? = nil, model: String = sha, sidecar: String? = side) -> PackageRecord {
            var r = PackageRecord(id: UUID(), name: src, modelFile: "model.glb", format: .glb, sidecarFile: "sidecar.json", sourceName: src, added: Date(),
                                  modelBytes: 1, sha256: model, elementCount: 1, triangleCount: 1, anchor: nil, alignmentMethod: nil, residualInches: nil, warnings: [])
            r.remoteName = remote; r.sidecarSHA256 = sidecar
            return r
        }
        func p(_ name: String, model: String = sha, sidecar: String = side) -> UpdateManifest.Package {
            .init(name: name, label: name, level: nil, exported: nil,
                  files: [.init(name: "\(name).glb", role: "glb", bytes: 1, sha256: model), .init(name: "\(name).json", role: "sidecar", bytes: 1, sha256: sidecar)])
        }
        let same = rec("x", remote: "room-a"), handGlb = rec("Room-B.glb"), changed = rec("room-c", remote: "room-c", model: String(repeating: "e", count: 64))
        let sideOnly = rec("room-d", remote: "room-d", sidecar: nil)
        let m = UpdateManifest(version: 1, generated: nil, packages: [p("room-a"), p("sub/room-b"), p("room-c"), p("room-d"), p("room-new")], problems: [])
        let plan = UpdatePlanner.plan(m, records: [same, handGlb, changed, sideOnly], sidecarHash: { $0.id == sideOnly.id ? String(repeating: "f", count: 64) : nil })
        XCTAssertEqual(plan.map(\.action), [.unchanged(same.id), .unchanged(handGlb.id), .changed(changed.id), .changed(sideOnly.id), .new])
    }

    // MARK: end to end through the stub server

    func testNewThenUnchangedThenUpdatedDownloadsOnlyWhatChanged() async throws {
        // Built once each: JSON key order isn't fixed, so a rebuilt fixture is a different file.
        let v1 = try Self.model(["E-1", "E-2"]), s1 = try Self.sidecar()
        let b = try Self.model(["E-9"]), sb = try Self.sidecar(model: "room-b.glb")
        try serve([("room-a", v1, s1), ("room-b", b, sb)])
        var r = try await run()
        XCTAssertEqual(r.summary, "0 updated, 2 new, 0 unchanged, 0 failed", "\(r.failed)")
        XCTAssertEqual(lib.list().count, 2)
        let a = try XCTUnwrap(lib.list().first { $0.remoteName == "room-a" })
        XCTAssertEqual(a.name, "Label room-a"); XCTAssertEqual(a.sha256, Self.hex(v1)); XCTAssertEqual(a.exported, "2026-11-06T04:00:00Z")
        XCTAssertEqual(a.anchor?.planX, 10)

        StubServer.reset()
        try serve([("room-a", v1, s1), ("room-b", b, sb)])
        r = try await run()
        XCTAssertEqual(r.summary, "0 updated, 0 new, 2 unchanged, 0 failed")
        XCTAssertEqual(StubServer.served(), ["/tok3n/manifest.json"], "nothing downloaded when nothing changed")

        StubServer.reset()
        let v2 = try Self.model(["E-1", "E-3"])
        try serve([("room-a", v2, s1), ("room-b", b, sb)])
        r = try await run()
        XCTAssertEqual(r.summary, "1 updated, 0 new, 1 unchanged, 0 failed")
        XCTAssertEqual(Set(StubServer.served()), ["/tok3n/manifest.json", "/tok3n/room-a.glb", "/tok3n/room-a.json"])
        let a2 = try XCTUnwrap(lib.list().first { $0.remoteName == "room-a" })
        XCTAssertEqual(a2.id, a.id, "the same package, replaced in place")
        XCTAssertEqual(a2.sha256, Self.hex(v2))
        XCTAssertEqual(try Data(contentsOf: lib.modelURL(a2)), v2)
        XCTAssertNotNil(a2.updated)
    }

    func testAHandImportedPackageIsMatchedByItsFileName() async throws {
        let src = dir.appendingPathComponent("hand", isDirectory: true)
        try FileManager.default.createDirectory(at: src, withIntermediateDirectories: true)
        let v1 = try Self.model(["E-1"]), s1 = try Self.sidecar()
        try v1.write(to: src.appendingPathComponent("room-a.glb")); try s1.write(to: src.appendingPathComponent("room-a.json"))
        let hand = try lib.importItem(at: src.appendingPathComponent("room-a.glb"))
        try serve([("room-a", v1, s1)])
        var r = try await run()
        XCTAssertEqual(r.summary, "0 updated, 0 new, 1 unchanged, 0 failed", "matched by source name + hash, nothing fetched")
        StubServer.reset()
        try serve([("room-a", try Self.model(["E-1", "E-2"]), s1)])
        r = try await run()
        XCTAssertEqual(r.summary, "1 updated, 0 new, 0 unchanged, 0 failed")
        XCTAssertEqual(try onlyRecord().id, hand.id)
    }

    // MARK: field work survives

    func testMarksAndPhotosCarryOverAndAGoneElementsMarkStays() async throws {
        let s1 = try Self.sidecar()
        try serve([("room-a", try Self.model(["E-1", "E-2"]), s1)])
        _ = try await run()
        let old = try onlyRecord()
        let photo = try lib.savePhoto(Data([0xFF, 0xD8, 0xFF, 0xD9]), for: old)
        let m1 = Mark(elementID: "E-1", elementKind: "Receptacle", elementTag: "R0", status: .installed, photo: photo, packageVersion: old.currentVersion)
        let m2 = Mark(elementID: "E-2", elementKind: "Receptacle", elementTag: "R1", status: .missing, note: "not roughed in")
        try lib.saveMarks([m1, m2], for: old)

        StubServer.reset()
        try serve([("room-a", try Self.model(["E-1", "E-3"]), s1)])
        let r = try await run()
        XCTAssertEqual(r.summary, "1 updated, 0 new, 0 unchanged, 0 failed")
        XCTAssertTrue(r.notes.contains { $0.contains("1 mark on objects no longer in the model") }, "\(r.notes)")
        let new = try onlyRecord()
        let marks = lib.marks(new)
        XCTAssertEqual(marks.map(\.id), [m1.id, m2.id], "every mark kept, in order")
        XCTAssertNil(marks[0].elementGone)
        XCTAssertEqual(marks[1].elementGone, true, "E-2 is gone: the mark stays, flagged")
        XCTAssertEqual(marks[1].note, "not roughed in")
        XCTAssertEqual(marks[1].packageVersion, old.currentVersion, "a mark says which version it was made against")
        XCTAssertNotEqual(marks[1].packageVersion, new.currentVersion)
        XCTAssertEqual(try Data(contentsOf: lib.photosURL(new).appendingPathComponent(photo)), Data([0xFF, 0xD8, 0xFF, 0xD9]))

        // The export names the versions.
        let x = try MarksExport.decode(Data(contentsOf: lib.exportMarks(new, app: "test")))
        XCTAssertEqual(x.package.version, new.currentVersion)
        XCTAssertEqual(x.marks[1].mark.packageVersion, old.currentVersion)
        XCTAssertEqual(x.marks[1].mark.elementGone, true)
        let raw = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: lib.exportMarks(new, app: "test"))) as? [String: Any])
        let row = try XCTUnwrap((raw["marks"] as? [[String: Any]])?.last)
        XCTAssertEqual(row["element_gone"] as? Bool, true)
        XCTAssertNotNil((row["package_version"] as? [String: Any])?["model_sha256"])

        // The element comes back in a later version: the flag clears.
        StubServer.reset()
        try serve([("room-a", try Self.model(["E-1", "E-2", "E-3"]), s1)])
        _ = try await run()
        XCTAssertNil(lib.marks(try onlyRecord())[1].elementGone)
    }

    func testTheSavedRoomIsKeptWhenTheAnchorIsTheSameAndReAlignAskedWhenItMoved() async throws {
        try serve([("room-a", try Self.model(["E-1"]), try Self.sidecar())])
        _ = try await run()
        var old = try onlyRecord()
        old.alignmentMethod = "two_point"; old.residualInches = 0.3
        try lib.save(old)
        try Data("worldmap".utf8).write(to: lib.worldMapURL(old))

        // Same anchor (within 1/8"), new model: the room and alignment are kept.
        StubServer.reset()
        try serve([("room-a", try Self.model(["E-1", "E-2"]), try Self.sidecar(anchorX: 10.005))])
        var r = try await run()
        var now = try onlyRecord()
        XCTAssertTrue(lib.hasWorldMap(now)); XCTAssertEqual(now.alignmentMethod, "two_point"); XCTAssertNil(now.needsRealign)
        XCTAssertTrue(r.notes.isEmpty, "\(r.notes)")

        // The anchor moved 2 ft: the old anchor is kept for display and a re-align is asked for.
        StubServer.reset()
        try serve([("room-a", try Self.model(["E-1", "E-2"]), try Self.sidecar(anchorX: 12))])
        r = try await run()
        now = try onlyRecord()
        XCTAssertEqual(now.needsRealign, true)
        XCTAssertEqual(now.previousAnchor?.planX, 10.005)
        XCTAssertNil(now.alignmentMethod)
        XCTAssertFalse(lib.hasWorldMap(now), "the old room alignment no longer fits")
        XCTAssertTrue(r.notes.contains { $0.contains("align again") }, "\(r.notes)")
        let p = try XCTUnwrap(now.anchor?.position(of: try XCTUnwrap(now.previousAnchor)))
        XCTAssertNear(p, SIMD3(Float(-1.995 * 0.3048), 0, 0))

        // Moved again before re-aligning: the anchor the room was last aligned to is still the one shown.
        StubServer.reset()
        try serve([("room-a", try Self.model(["E-1", "E-2"]), try Self.sidecar(anchorX: 14))])
        _ = try await run()
        XCTAssertEqual(try onlyRecord().previousAnchor?.planX, 10.005)
    }

    // MARK: hostile and offline: plain message, old package untouched

    func testEveryFailureLeavesTheOldPackageWorking() async throws {
        let v1 = try Self.model(["E-1"]), s1 = try Self.sidecar()
        try serve([("room-a", v1, s1)])
        _ = try await run()
        let old = try onlyRecord()
        try lib.saveMarks([Mark(elementID: "E-1", status: .installed)], for: old)
        let before = try folderSnapshot(old)
        let v2 = try Self.model(["E-1", "E-2"])

        typealias Case = (String, () throws -> Void, String)
        let cases: [Case] = [
            ("hash mismatch", { try self.serve([("room-a", v2, s1)], fileOverride: ["sha256": String(repeating: "0", count: 64)]) }, "SHA-256"),
            ("short file", { try self.serve([("room-a", v2, s1)], fileOverride: ["bytes": v2.count + 10]) }, "not the"),
            ("oversized file", { try self.serve([("room-a", v2, s1)], fileOverride: ["bytes": 100]) }, "larger than the manifest says"),
            ("wrong file (not a GLB)", {
                let junk = Data(repeating: 0x41, count: 300)
                try self.serve([("room-a", junk, s1)])
            }, "not a glTF binary"),
            ("bad sidecar", { try self.serve([("room-a", v2, Data("{nope".utf8))]) }, "not valid JSON"),
            ("Z-up sidecar", { try self.serve([("room-a", v2, Data(#"{"up_axis":"Z"}"#.utf8))]) }, "up axis"),
            ("model missing on the server", {
                try self.serve([("room-a", v2, s1)])
                StubServer.set("/tok3n/room-a.glb", .init(status: 404))
            }, "404"),
            ("connection lost mid-package", {
                try self.serve([("room-a", v2, s1)])
                StubServer.set("/tok3n/room-a.glb", .init(error: .networkConnectionLost))
            }, "No network"),
        ]
        for (what, setup, says) in cases {
            StubServer.reset()
            try setup()
            let r = try await run()
            XCTAssertEqual(r.failed.count, 1, "\(what): \(r.summary)")
            XCTAssertTrue(r.failed.first?.message.localizedCaseInsensitiveContains(says) ?? false, "\(what): \(r.failed.first?.message ?? "-")")
            XCTAssertFalse(r.failed.first?.message.isEmpty ?? true, what)
            XCTAssertEqual(try folderSnapshot(try onlyRecord()), before, "\(what): the old package is untouched")
            XCTAssertEqual(lib.list().count, 1, what)
            let leftovers = try FileManager.default.contentsOfDirectory(atPath: lib.root.path).filter { $0.hasPrefix(".") }
            XCTAssertEqual(leftovers, [], "\(what): no staging left behind")
        }
        // The old version still opens.
        XCTAssertEqual(try lib.loadModel(try onlyRecord()).0.elements.map(\.info.id), ["E-1"])
    }

    func testServerUnreachableOfflineAndBadManifestsFailTheRunWithAPlainMessage() async throws {
        let cases: [(StubServer.Route?, String, String)] = [
            (StubServer.Route(error: .cannotConnectToHost), base + "/", "can't be reached"),
            (StubServer.Route(error: .notConnectedToInternet), base + "/", "No network"),
            (StubServer.Route(error: .timedOut), base + "/", "in time"),
            (nil, base + "/", "no manifest.json"),
            (StubServer.Route(status: 403), base + "/", "403"),
            (StubServer.Route(body: Data("<html>".utf8)), base + "/", "not valid JSON"),
            (StubServer.Route(body: Data(#"{"format":"cadaddy-ar-manifest","version":2,"packages":[]}"#.utf8)), base + "/", "newer"),
            (StubServer.Route(body: Data(count: Int(FieldLimits.maxManifestBytes) + 10)), base + "/", "too large"),
            (nil, "http://example.com/x/", "local network"),
            (nil, "not an address", "http://"),
        ]
        for (route, address, says) in cases {
            StubServer.reset()
            if let route { StubServer.set("/tok3n/manifest.json", route) }
            do {
                _ = try await updater.run(address: address)
                XCTFail("\(says): expected a failure")
            } catch let e as FieldError {
                XCTAssertTrue(e.message.localizedCaseInsensitiveContains(says), "\(says): \(e.message)")
            }
            XCTAssertEqual(lib.list().count, 0)
        }
        XCTAssertFalse(StubServer.served().contains { $0.contains("example") }, "a public address is refused before any request")
    }

    func testAnInterruptedSwapIsUndoneOnTheNextLaunch() throws {
        let src = try SamplePackage.write(to: dir.appendingPathComponent("pkg"))
        let r = try lib.importItem(at: src)
        // As if the app stopped after moving the live folder aside and before putting the new one in place.
        let aside = lib.root.appendingPathComponent(".old-\(r.id.uuidString)")
        try FileManager.default.moveItem(at: lib.folder(r.id), to: aside)
        try FileManager.default.createDirectory(at: lib.root.appendingPathComponent(".staging-x"), withIntermediateDirectories: true)
        try FileManager.default.createDirectory(at: lib.root.appendingPathComponent(".incoming-y"), withIntermediateDirectories: true)
        XCTAssertEqual(lib.list().count, 0, "hidden folders are never listed as packages")
        let again = try PackageLibrary(root: lib.root)
        XCTAssertEqual(again.list().map(\.id), [r.id])
        XCTAssertEqual(try FileManager.default.contentsOfDirectory(atPath: lib.root.path).filter { $0.hasPrefix(".") }, [])
    }

    func testSwapExchangesFoldersAtomically() throws {
        let a = lib.root.appendingPathComponent(".staging-a"), b = lib.root.appendingPathComponent(UUID().uuidString)
        for (u, s) in [(a, "new"), (b, "old")] {
            try FileManager.default.createDirectory(at: u, withIntermediateDirectories: true)
            try Data(s.utf8).write(to: u.appendingPathComponent("f"))
        }
        try lib.swapFolders(a, b, id: UUID())
        XCTAssertEqual(try String(contentsOf: b.appendingPathComponent("f"), encoding: .utf8), "new")
        XCTAssertEqual(try String(contentsOf: a.appendingPathComponent("f"), encoding: .utf8), "old")
    }

    func testAnchorComparison() {
        let a = AnchorInfo(kind: "room_corner", planX: 1, planY: 2, planUnits: "ft", level: "L")
        var b = a; b.planX = 1.01
        XCTAssertTrue(AnchorInfo.sameSpot(a, b))
        b.planX = 1.02; XCTAssertFalse(AnchorInfo.sameSpot(a, b))
        b = a; b.level = "M"; XCTAssertFalse(AnchorInfo.sameSpot(a, b))
        b = a; b.kind = "grid_intersection"; XCTAssertFalse(AnchorInfo.sameSpot(a, b))
        XCTAssertTrue(AnchorInfo.sameSpot(nil, nil)); XCTAssertFalse(AnchorInfo.sameSpot(a, nil))
        b = a; b.planX = .infinity; XCTAssertFalse(AnchorInfo.sameSpot(a, b))
        XCTAssertNil(a.position(of: AnchorInfo(kind: "room_corner", planX: 1, planY: 2, planUnits: "ft", level: "other")))
        XCTAssertNear(a.position(of: AnchorInfo(kind: "room_corner", planX: 2, planY: 3, planUnits: "ft", level: "L"))!, SIMD3(0.3048, 0, -0.3048))
    }
}
