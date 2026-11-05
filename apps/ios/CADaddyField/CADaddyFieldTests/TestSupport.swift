import Compression
import Foundation
import simd
import XCTest

enum TestSupport {
    /// A .glb from a JSON object and a BIN chunk.
    static func glb(_ json: [String: Any], bin: Data = Data()) throws -> Data {
        var j = try JSONSerialization.data(withJSONObject: json)
        while j.count % 4 != 0 { j.append(0x20) }
        var b = bin
        while b.count % 4 != 0 { b.append(0) }
        var out = Data()
        func u32(_ v: UInt32) { withUnsafeBytes(of: v.littleEndian) { out.append(contentsOf: $0) } }
        u32(0x4654_6C67); u32(2); u32(UInt32(12 + 8 + j.count + (b.isEmpty ? 0 : 8 + b.count)))
        u32(UInt32(j.count)); u32(0x4E4F_534A); out.append(j)
        if !b.isEmpty { u32(UInt32(b.count)); u32(0x004E_4942); out.append(b) }
        return out
    }

    static func floats(_ v: [Float]) -> Data {
        var d = Data(); for f in v { withUnsafeBytes(of: f.bitPattern.littleEndian) { d.append(contentsOf: $0) } }; return d
    }

    static func u32s(_ v: [UInt32]) -> Data {
        var d = Data(); for x in v { withUnsafeBytes(of: x.littleEndian) { d.append(contentsOf: $0) } }; return d
    }

    /// One triangle mesh (3 vertices, indices 0 1 2) plus the given nodes.
    static func triangleGLB(nodes: [[String: Any]], scenes: [Int]? = nil, indices: [UInt32] = [0, 1, 2], extra: [String: Any] = [:]) throws -> Data {
        let pos = floats([0, 0, 0, 1, 0, 0, 0, 1, 0])
        let idx = u32s(indices)
        var root: [String: Any] = [
            "asset": ["version": "2.0"],
            "buffers": [["byteLength": pos.count + idx.count]],
            "bufferViews": [["buffer": 0, "byteOffset": 0, "byteLength": pos.count], ["buffer": 0, "byteOffset": pos.count, "byteLength": idx.count]],
            "accessors": [["bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3"],
                          ["bufferView": 1, "componentType": 5125, "count": indices.count, "type": "SCALAR"]],
            "meshes": [["primitives": [["attributes": ["POSITION": 0], "indices": 1]]]],
            "nodes": nodes,
        ]
        if let s = scenes { root["scenes"] = [["nodes": s]]; root["scene"] = 0 }
        for (k, v) in extra { root[k] = v }
        return try glb(root, bin: pos + idx)
    }

    struct ZipFile { var name: String; var data: Data; var deflate: Bool = false }

    /// A minimal zip writer (stored or raw-deflate entries); CRCs are not checked by the reader.
    static func zip(_ files: [ZipFile]) -> Data {
        var out = Data(), central = Data()
        func u16(_ v: UInt16, _ d: inout Data) { withUnsafeBytes(of: v.littleEndian) { d.append(contentsOf: $0) } }
        func u32(_ v: UInt32, _ d: inout Data) { withUnsafeBytes(of: v.littleEndian) { d.append(contentsOf: $0) } }
        for f in files {
            let name = Data(f.name.utf8)
            var body = f.data
            var method: UInt16 = 0
            if f.deflate, let c = deflate(f.data) { body = c; method = 8 }
            let offset = UInt32(out.count)
            u32(0x0403_4B50, &out); u16(20, &out); u16(0, &out); u16(method, &out); u16(0, &out); u16(0, &out)
            u32(0, &out); u32(UInt32(body.count), &out); u32(UInt32(f.data.count), &out); u16(UInt16(name.count), &out); u16(0, &out)
            out.append(name); out.append(body)
            u32(0x0201_4B50, &central); u16(20, &central); u16(20, &central); u16(0, &central); u16(method, &central); u16(0, &central); u16(0, &central)
            u32(0, &central); u32(UInt32(body.count), &central); u32(UInt32(f.data.count), &central); u16(UInt16(name.count), &central)
            u16(0, &central); u16(0, &central); u16(0, &central); u16(0, &central); u32(0, &central); u32(offset, &central)
            central.append(name)
        }
        let cdOffset = UInt32(out.count)
        out.append(central)
        u32(0x0605_4B50, &out); u16(0, &out); u16(0, &out); u16(UInt16(files.count), &out); u16(UInt16(files.count), &out)
        u32(UInt32(central.count), &out); u32(cdOffset, &out); u16(0, &out)
        return out
    }

    static func deflate(_ d: Data) -> Data? {
        var dst = Data(count: d.count + 1024)
        let n = dst.withUnsafeMutableBytes { o in d.withUnsafeBytes { i in
            compression_encode_buffer(o.bindMemory(to: UInt8.self).baseAddress!, d.count + 1024, i.bindMemory(to: UInt8.self).baseAddress!, d.count, nil, COMPRESSION_ZLIB)
        } }
        guard n > 0 else { return nil }
        return dst.prefix(n)
    }

    static func tempDir(_ name: String = UUID().uuidString) throws -> URL {
        let u = FileManager.default.temporaryDirectory.appendingPathComponent("fieldtests-\(name)", isDirectory: true)
        try? FileManager.default.removeItem(at: u)
        try FileManager.default.createDirectory(at: u, withIntermediateDirectories: true)
        return u
    }
}

func XCTAssertNear(_ a: SIMD3<Float>, _ b: SIMD3<Float>, _ tol: Float = 0.001, file: StaticString = #filePath, line: UInt = #line) {
    XCTAssertLessThanOrEqual(simd_length(a - b), tol, "\(a) vs \(b)", file: file, line: line)
}

func XCTAssertThrowsFieldError(_ expr: @autoclosure () throws -> Any, contains: String? = nil, file: StaticString = #filePath, line: UInt = #line) {
    do {
        _ = try expr()
        XCTFail("expected a FieldError", file: file, line: line)
    } catch let e as FieldError {
        if let c = contains { XCTAssertTrue(e.message.localizedCaseInsensitiveContains(c), "message: \(e.message)", file: file, line: line) }
    } catch {
        XCTFail("unexpected error type \(error)", file: file, line: line)
    }
}
