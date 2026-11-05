// CADaddy Field — minimal, defensive .zip reader (stored and deflate entries) for package archives.
// Uses Apple's Compression framework for raw DEFLATE. Zip64, encryption and multi-disk archives are refused.

import Compression
import Foundation

public struct ZipEntry: Equatable, Sendable {
    public var name: String
    public var method: UInt16
    public var compressedSize: Int
    public var uncompressedSize: Int
    public var localHeaderOffset: Int
    public var isDirectory: Bool { name.hasSuffix("/") }
}

public struct ZipReader {
    public let data: Data
    public private(set) var entries: [ZipEntry] = []

    public init(data: Data) throws {
        self.data = data
        entries = try Self.readDirectory(data)
    }

    static func u16(_ d: Data, _ o: Int) -> UInt16? {
        guard o >= 0, o + 2 <= d.count else { return nil }
        return d.withUnsafeBytes { $0.loadUnaligned(fromByteOffset: o, as: UInt16.self) }.littleEndian
    }

    static func u32(_ d: Data, _ o: Int) -> UInt32? {
        guard o >= 0, o + 4 <= d.count else { return nil }
        return d.withUnsafeBytes { $0.loadUnaligned(fromByteOffset: o, as: UInt32.self) }.littleEndian
    }

    static func readDirectory(_ d: Data) throws -> [ZipEntry] {
        guard d.count >= 22 else { throw FieldError("The zip file is too short.") }
        let lowest = max(0, d.count - 22 - 65_535)
        var eocd: Int?
        var o = d.count - 22
        while o >= lowest {
            if u32(d, o) == 0x0605_4B50 { eocd = o; break }
            o -= 1
        }
        guard let e = eocd, let disk = u16(d, e + 4), let cdDisk = u16(d, e + 6), let total = u16(d, e + 10),
              let cdSize = u32(d, e + 12), let cdOffset = u32(d, e + 16) else { throw FieldError("This is not a zip file.") }
        guard disk == 0, cdDisk == 0 else { throw FieldError("Split zip archives are not supported.") }
        guard total != 0xFFFF, cdOffset != 0xFFFF_FFFF, cdSize != 0xFFFF_FFFF else { throw FieldError("Zip64 archives are not supported. Zip the package with standard settings.") }
        guard Int(total) <= FieldLimits.maxZipEntries else { throw FieldError("The zip has too many entries.") }
        guard Int(cdOffset) + Int(cdSize) <= e else { throw FieldError("The zip directory is damaged.") }
        var out: [ZipEntry] = []
        var p = Int(cdOffset)
        var totalBytes: Int64 = 0
        for _ in 0..<Int(total) {
            guard u32(d, p) == 0x0201_4B50, let flags = u16(d, p + 8), let method = u16(d, p + 10), let csize = u32(d, p + 20),
                  let usize = u32(d, p + 24), let nlen = u16(d, p + 28), let xlen = u16(d, p + 30), let clen = u16(d, p + 32),
                  let lho = u32(d, p + 42) else { throw FieldError("The zip directory is damaged.") }
            guard flags & 1 == 0 else { throw FieldError("Encrypted zip files are not supported.") }
            guard csize != 0xFFFF_FFFF, usize != 0xFFFF_FFFF, lho != 0xFFFF_FFFF else { throw FieldError("Zip64 archives are not supported.") }
            let nameStart = p + 46
            guard nameStart + Int(nlen) <= d.count else { throw FieldError("The zip directory is damaged.") }
            let nameData = d.subdata(in: (d.startIndex + nameStart)..<(d.startIndex + nameStart + Int(nlen)))
            let name = String(data: nameData, encoding: .utf8) ?? String(decoding: nameData, as: UTF8.self)
            totalBytes += Int64(usize)
            guard totalBytes <= FieldLimits.maxZipTotalBytes else { throw FieldError("The zip expands to more than 1 GB.") }
            out.append(ZipEntry(name: name, method: method, compressedSize: Int(csize), uncompressedSize: Int(usize), localHeaderOffset: Int(lho)))
            p = nameStart + Int(nlen) + Int(xlen) + Int(clen)
        }
        return out
    }

    /// A safe relative path for an entry, or nil for absolute paths, `..`, and macOS metadata.
    public static func safeName(_ raw: String) -> String? {
        let name = raw.replacingOccurrences(of: "\\", with: "/")
        guard !name.hasPrefix("/"), !name.contains("\0") else { return nil }
        let comps = name.split(separator: "/").map(String.init)
        guard !comps.isEmpty, !comps.contains(".."), !comps.contains("."), comps.first != "__MACOSX",
              comps.last.map({ !$0.hasPrefix("._") && $0 != ".DS_Store" }) ?? false else { return nil }
        return comps.joined(separator: "/")
    }

    /// The uncompressed bytes of one entry.
    public func extract(_ e: ZipEntry, maxBytes: Int64 = FieldLimits.maxModelBytes) throws -> Data {
        guard Int64(e.uncompressedSize) <= maxBytes else { throw FieldError("\(e.name) is too large.") }
        let h = e.localHeaderOffset
        guard Self.u32(data, h) == 0x0403_4B50, let nlen = Self.u16(data, h + 26), let xlen = Self.u16(data, h + 28) else {
            throw FieldError("The zip entry \(e.name) is damaged.")
        }
        let start = h + 30 + Int(nlen) + Int(xlen)
        guard start >= 0, start <= data.count, e.compressedSize <= data.count - start else { throw FieldError("The zip entry \(e.name) is damaged.") }
        let body = data.subdata(in: (data.startIndex + start)..<(data.startIndex + start + e.compressedSize))
        switch e.method {
        case 0:
            guard body.count == e.uncompressedSize else { throw FieldError("The zip entry \(e.name) is damaged.") }
            return body
        case 8:
            if e.uncompressedSize == 0 { return Data() }
            var out = Data(count: e.uncompressedSize)
            let written = out.withUnsafeMutableBytes { dst -> Int in
                body.withUnsafeBytes { src -> Int in
                    guard let d = dst.bindMemory(to: UInt8.self).baseAddress, let s = src.bindMemory(to: UInt8.self).baseAddress else { return 0 }
                    return compression_decode_buffer(d, e.uncompressedSize, s, body.count, nil, COMPRESSION_ZLIB)
                }
            }
            guard written == e.uncompressedSize else { throw FieldError("The zip entry \(e.name) could not be decompressed.") }
            return out
        default:
            throw FieldError("The zip entry \(e.name) uses an unsupported compression method (\(e.method)).")
        }
    }
}
