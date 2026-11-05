// CADaddy Field — a synthetic sample package in the `bim.export.ar` layout: a made-up guest-room-sized
// room with walls, floor, a translucent ceiling and devices of several trades at typical heights.
// No job data: every name, tag, circuit and catalog number here is invented.

import Foundation
import simd

public enum SamplePackage {
    public static let title = "Sample Guest Room"

    struct Device {
        var kind: String, tag: String, trade: String, catalog: String
        var circuit: String?
        var at: SIMD3<Float>          // device centre (x, mounting height, z)
        var yaw: Float                 // facing into the room
        var mesh: String
    }

    /// Writes `<title>.glb` and `<title>.json` into `folder` and returns the folder. `rooms` > 1 repeats the
    /// room along +X (a larger package for load and frame-rate checks).
    @discardableResult
    public static func write(to folder: URL, rooms: Int = 1, title: String = SamplePackage.title) throws -> URL {
        let n = max(1, min(rooms, 400))
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let glb = try makeGLB(rooms: n)
        try glb.write(to: folder.appendingPathComponent("\(title).glb"), options: .atomic)
        let sidecar: [String: Any] = [
            "format": "cadaddy-ar", "version": 1, "title": title, "model": "\(title).glb", "units": "m", "up_axis": "Y",
            "anchor": ["kind": "room_corner", "name": "Room corner nearest the entry", "plan_x": 0.0, "plan_y": 0.0,
                       "plan_units": "ft", "level": "Level 1", "room": "Sample Room"],
            "points": [["name": "Far corner (window wall, left)", "position": [0.0, 0.0, -7.6]],
                       ["name": "Far corner (window wall, right)", "position": [4.0, 0.0, -7.6]],
                       ["name": "Entry-side corner (right)", "position": [4.0, 0.0, 0.0]]],
        ]
        let json = try JSONSerialization.data(withJSONObject: sidecar, options: [.prettyPrinted, .sortedKeys])
        try json.write(to: folder.appendingPathComponent("\(title).json"), options: .atomic)
        return folder
    }

    public static func makeGLB(rooms: Int = 1) throws -> Data {
        var w = GLBWriter()
        let wallMat = w.addMaterial("context-wall", SIMD4(0.86, 0.85, 0.82, 1))
        let floorMat = w.addMaterial("context-floor", SIMD4(0.55, 0.52, 0.48, 1))
        let ceilMat = w.addMaterial("context-ceiling", SIMD4(0.95, 0.95, 0.95, 0.25))
        let elecMat = w.addMaterial("trade-electrical", SIMD4(1.0, 0.72, 0.1, 1))
        let lvMat = w.addMaterial("trade-low-voltage", SIMD4(0.2, 0.5, 1.0, 1))
        let faMat = w.addMaterial("trade-fire-alarm", SIMD4(0.9, 0.15, 0.15, 1))
        let mechMat = w.addMaterial("trade-mechanical", SIMD4(0.2, 0.75, 0.35, 1))
        let unitWall = w.addMesh(GLBWriter.box(name: "unit-wall", size: SIMD3(1, 1, 1), material: wallMat))
        let unitFloor = w.addMesh(GLBWriter.box(name: "unit-floor", size: SIMD3(1, 1, 1), material: floorMat))
        let unitCeil = w.addMesh(GLBWriter.box(name: "unit-ceiling", size: SIMD3(1, 1, 1), material: ceilMat))
        let meshes: [String: Int] = [
            "device-e": w.addMesh(GLBWriter.box(name: "device-plate", size: SIMD3(0.07, 0.115, 0.04), material: elecMat)),
            "device-lv": w.addMesh(GLBWriter.box(name: "device-plate-lv", size: SIMD3(0.07, 0.115, 0.04), material: lvMat)),
            "tstat": w.addMesh(GLBWriter.box(name: "thermostat", size: SIMD3(0.09, 0.09, 0.03), material: mechMat)),
            "smoke": w.addMesh(GLBWriter.box(name: "smoke-detector", size: SIMD3(0.13, 0.04, 0.13), material: faMat)),
            "lum": w.addMesh(GLBWriter.box(name: "luminaire", size: SIMD3(0.15, 0.06, 0.15), material: elecMat)),
            "sconce": w.addMesh(GLBWriter.box(name: "sconce", size: SIMD3(0.12, 0.2, 0.08), material: elecMat)),
            "fan": w.addMesh(GLBWriter.box(name: "exhaust-fan", size: SIMD3(0.25, 0.12, 0.25), material: mechMat)),
            "jbox": w.addMesh(GLBWriter.box(name: "junction-box", size: SIMD3(0.1, 0.1, 0.06), material: elecMat)),
        ]
        let unitConduit = w.addMesh(GLBWriter.box(name: "unit-conduit", size: SIMD3(1, 1, 1), material: elecMat))
        let H: Float = 2.74
        let walls: [(String, SIMD3<Float>, SIMD3<Float>)] = [
            ("W1", SIMD3(1.25, H / 2, 0.075), SIMD3(2.8, H, 0.15)),       // entry wall, left of the door
            ("W2", SIMD3(3.825, H / 2, 0.075), SIMD3(0.65, H, 0.15)),     // entry wall, right of the door
            ("W3", SIMD3(2.0, H / 2, -7.675), SIMD3(4.3, H, 0.15)),       // window wall
            ("W4", SIMD3(-0.075, H / 2, -3.8), SIMD3(0.15, H, 7.6)),      // left wall
            ("W5", SIMD3(4.075, H / 2, -3.8), SIMD3(0.15, H, 7.6)),       // right wall
            ("W6", SIMD3(1.2, H / 2, -2.45), SIMD3(2.4, H, 0.1)),         // bath partition
            ("W7", SIMD3(2.45, H / 2, -1.25), SIMD3(0.1, H, 2.3)),        // bath partition
        ]
        let qL: Float = .pi / 2, qR: Float = -.pi / 2, qN: Float = 0, qS: Float = .pi
        let devices: [Device] = [
            Device(kind: "Receptacle", tag: "R1", trade: "electrical", catalog: "SAMPLE-DUPLEX-20A", circuit: "A-3", at: SIMD3(0.02, 0.4572, -3.5), yaw: qL, mesh: "device-e"),
            Device(kind: "Receptacle", tag: "R2", trade: "electrical", catalog: "SAMPLE-DUPLEX-20A", circuit: "A-3", at: SIMD3(0.02, 0.4572, -6.0), yaw: qL, mesh: "device-e"),
            Device(kind: "Receptacle", tag: "R3", trade: "electrical", catalog: "SAMPLE-DUPLEX-20A", circuit: "A-5", at: SIMD3(1.0, 0.4572, -7.58), yaw: qN, mesh: "device-e"),
            Device(kind: "Receptacle", tag: "R4", trade: "electrical", catalog: "SAMPLE-DUPLEX-20A", circuit: "A-5", at: SIMD3(3.98, 0.4572, -4.0), yaw: qR, mesh: "device-e"),
            Device(kind: "Receptacle", tag: "R5", trade: "electrical", catalog: "SAMPLE-DUPLEX-20A", circuit: "A-5", at: SIMD3(3.98, 0.4572, -6.5), yaw: qR, mesh: "device-e"),
            Device(kind: "Receptacle", tag: "R6", trade: "electrical", catalog: "SAMPLE-DUPLEX-20A", circuit: "A-7", at: SIMD3(3.98, 1.524, -5.2), yaw: qR, mesh: "device-e"),
            Device(kind: "GFCI Receptacle", tag: "G1", trade: "electrical", catalog: "SAMPLE-GFCI-20A", circuit: "A-1", at: SIMD3(0.02, 1.0668, -1.2), yaw: qL, mesh: "device-e"),
            Device(kind: "Switch", tag: "S1", trade: "electrical", catalog: "SAMPLE-SW-1P", circuit: "A-9", at: SIMD3(2.52, 1.2192, -0.6), yaw: qL, mesh: "device-e"),
            Device(kind: "Switch", tag: "S2", trade: "electrical", catalog: "SAMPLE-SW-1P", circuit: "A-1", at: SIMD3(2.38, 1.2192, -0.6), yaw: qR, mesh: "device-e"),
            Device(kind: "Switch", tag: "S3", trade: "electrical", catalog: "SAMPLE-SW-3W", circuit: "A-9", at: SIMD3(0.02, 1.2192, -5.0), yaw: qL, mesh: "device-e"),
            Device(kind: "Data Outlet", tag: "D1", trade: "low-voltage", catalog: "SAMPLE-CAT6-1P", circuit: nil, at: SIMD3(3.98, 0.4572, -5.4), yaw: qR, mesh: "device-lv"),
            Device(kind: "Thermostat", tag: "T1", trade: "mechanical", catalog: "SAMPLE-TSTAT", circuit: nil, at: SIMD3(3.98, 1.524, -3.0), yaw: qR, mesh: "tstat"),
            Device(kind: "Smoke Detector", tag: "SD1", trade: "fire-alarm", catalog: "SAMPLE-SD-120V", circuit: "A-11", at: SIMD3(2.0, H - 0.02, -4.5), yaw: qN, mesh: "smoke"),
            Device(kind: "Luminaire", tag: "L1", trade: "electrical", catalog: "SAMPLE-DL-6IN", circuit: "A-9", at: SIMD3(3.2, H - 0.03, -1.2), yaw: qN, mesh: "lum"),
            Device(kind: "Luminaire", tag: "L2", trade: "electrical", catalog: "SAMPLE-DL-6IN", circuit: "A-1", at: SIMD3(1.2, H - 0.03, -1.2), yaw: qN, mesh: "lum"),
            Device(kind: "Sconce", tag: "SC1", trade: "electrical", catalog: "SAMPLE-SCONCE", circuit: "A-9", at: SIMD3(0.04, 1.6764, -4.3), yaw: qL, mesh: "sconce"),
            Device(kind: "Sconce", tag: "SC2", trade: "electrical", catalog: "SAMPLE-SCONCE", circuit: "A-9", at: SIMD3(0.04, 1.6764, -6.7), yaw: qL, mesh: "sconce"),
            Device(kind: "Exhaust Fan", tag: "EF1", trade: "mechanical", catalog: "SAMPLE-EF-80", circuit: "A-1", at: SIMD3(0.8, H - 0.06, -0.6), yaw: qS, mesh: "fan"),
            Device(kind: "Junction Box", tag: "JB1", trade: "electrical", catalog: "SAMPLE-4SQ", circuit: "A-9", at: SIMD3(0.3, 2.9, -3.0), yaw: qN, mesh: "jbox"),
            Device(kind: "Junction Box", tag: "JB2", trade: "electrical", catalog: "SAMPLE-4SQ", circuit: "A-5", at: SIMD3(3.8, 2.9, -3.0), yaw: qN, mesh: "jbox"),
        ]
        var serial = 1000
        for r in 0..<rooms {
            let dx = Float(r) * 4.3
            let suffix = rooms > 1 ? "-\(r + 1)" : ""
            for (tag, c, s) in walls {
                serial += 1
                w.addNode(.init(name: "Wall_\(tag)\(suffix)", mesh: unitWall, translation: c + SIMD3(dx, 0, 0), scale: s,
                                extras: ["id": "X-\(serial)", "kind": "Wall", "tag": tag + suffix, "trade": "context", "source": "synthetic sample"]))
            }
            serial += 1
            w.addNode(.init(name: "Floor\(suffix)", mesh: unitFloor, translation: SIMD3(2 + dx, -0.05, -3.8), scale: SIMD3(4, 0.1, 7.6),
                            extras: ["id": "X-\(serial)", "kind": "Floor", "trade": "context", "source": "synthetic sample"]))
            serial += 1
            w.addNode(.init(name: "Ceiling\(suffix)", mesh: unitCeil, translation: SIMD3(2 + dx, H + 0.008, -3.8), scale: SIMD3(4, 0.016, 7.6),
                            extras: ["id": "X-\(serial)", "kind": "Ceiling", "trade": "context", "source": "synthetic sample"]))
            for d in devices {
                serial += 1
                var ex: [String: Any] = ["id": "E-\(serial)", "kind": d.kind, "tag": d.tag + suffix, "trade": d.trade, "catalog": d.catalog,
                                         "mounting_height_m": Double(d.at.y), "source": "synthetic sample"]
                if let c = d.circuit { ex["circuit"] = c; ex["panel"] = "PANEL-A" }
                w.addNode(.init(name: "\(d.kind.replacingOccurrences(of: " ", with: "_"))_\(d.tag)\(suffix)", mesh: meshes[d.mesh],
                                translation: d.at + SIMD3(dx, 0, 0), yaw: d.yaw, extras: ex))
            }
            // Two conduit runs above the ceiling, 3/4" EMT.
            let runs: [(String, SIMD3<Float>, SIMD3<Float>)] = [
                ("C1", SIMD3(2.05 + dx, 2.9, -3.0), SIMD3(3.5, 0.0235, 0.0235)),
                ("C2", SIMD3(0.3 + dx, 2.9, -4.0), SIMD3(0.0235, 0.0235, 6.0)),
            ]
            for (tag, c, s) in runs {
                serial += 1
                w.addNode(.init(name: "Conduit_\(tag)\(suffix)", mesh: unitConduit, translation: c, scale: s,
                                extras: ["id": "E-\(serial)", "kind": "Conduit", "tag": tag + suffix, "trade": "electrical",
                                         "catalog": "SAMPLE-EMT-3/4", "circuit": "A-9", "panel": "PANEL-A",
                                         "mounting_height_m": Double(c.y), "source": "synthetic sample"]))
            }
        }
        w.assetExtras = ["cadaddy": ["anchor": ["kind": "room_corner", "name": "Room corner nearest the entry", "plan_x": 0.0, "plan_y": 0.0]]]
        return try w.encode()
    }
}
