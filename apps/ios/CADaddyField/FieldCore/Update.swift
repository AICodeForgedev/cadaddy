// CADaddy Field — the Update button: pull the latest room packages from the office over the local network.
//
// The office runs `bim.export.ar.manifest {dir}` on a folder of AR exports and serves the folder with any static web
// server. The app GETs `manifest.json` at the stored address, compares each package with the library by its stored
// source name and hashes, downloads only what changed, verifies size + SHA-256 and that the model opens, then swaps
// the package folder atomically (PackageLibrary.applyUpdate). Format: docs/ar-export.md "Serving exports to the field app".
//
// Network use: only when the user taps Update; local network only (the address must be a private, link-local or
// loopback IP, a `.local` name or a single-label host name). No accounts, no cloud, no background checks.

import CryptoKit
import Foundation

// MARK: - Manifest

public struct UpdateManifest: Equatable, Sendable {
    public static let formatID = "cadaddy-ar-manifest"
    public static let currentVersion = 1

    public struct File: Equatable, Sendable {
        /// Relative name under the manifest's folder, `/`-separated, already checked safe.
        public var name: String
        /// glb | usdz | sidecar | other
        public var role: String
        public var bytes: Int64
        /// Lowercase hex SHA-256.
        public var sha256: String
    }

    public struct Package: Equatable, Sendable {
        /// Unique in the manifest: the export's relative name without extension.
        public var name: String
        public var label: String
        public var level: String?
        public var exported: String?
        public var files: [File]

        /// The model the app downloads (.glb preferred: it carries per-object metadata), and the sidecar.
        public var model: File? { files.first { $0.role == "glb" } ?? files.first { $0.role == "usdz" } }
        public var sidecar: File? { files.first { $0.role == "sidecar" } }
        /// The last part of `name` (what a hand-imported package's source name would be).
        public var baseName: String { name.split(separator: "/").last.map(String.init) ?? name }
    }

    public var version: Int
    public var generated: String?
    public var packages: [Package]
    /// Entries refused while reading (bad names, sizes, hashes); reported, not fatal.
    public var problems: [String]

    /// Parses manifest JSON. Refuses non-JSON, another format, a newer version and oversized lists; a single bad
    /// package entry is left out and reported in `problems`.
    public static func parse(_ data: Data) throws -> UpdateManifest {
        guard Int64(data.count) <= FieldLimits.maxManifestBytes else { throw FieldError("The office's manifest.json is too large.") }
        let root: Any
        do { root = try JSONSerialization.jsonObject(with: data, options: []) } catch {
            throw FieldError("The office's manifest.json is not valid JSON.")
        }
        guard let obj = root as? [String: Any] else { throw FieldError("The office's manifest.json is not a JSON object.") }
        guard (obj["format"] as? String) == formatID else {
            throw FieldError("That address doesn't serve a CADaddy AR manifest. Run bim.export.ar.manifest on the exports folder and serve that folder.")
        }
        let version = (obj["version"] as? NSNumber)?.intValue ?? 0
        guard version >= 1 else { throw FieldError("The office's manifest.json has no valid version.") }
        guard version <= currentVersion else { throw FieldError("The office's manifest is from a newer CADaddy. Update this app first.") }
        guard let list = obj["packages"] as? [Any] else { throw FieldError("The office's manifest.json has no package list.") }
        guard list.count <= FieldLimits.maxManifestPackages else { throw FieldError("The office's manifest lists too many packages.") }
        var out: [Package] = []
        var problems: [String] = []
        var seen = Set<String>()
        for (i, item) in list.enumerated() {
            do {
                let p = try parsePackage(item)
                guard seen.insert(p.name).inserted else { throw FieldError("\(p.name) is listed twice") }
                out.append(p)
            } catch {
                problems.append("Package \(i + 1): \((error as? FieldError)?.message ?? "unreadable")")
            }
        }
        return UpdateManifest(version: version, generated: (obj["generated"] as? String)?.clipped(), packages: out, problems: Array(problems.prefix(50)))
    }

    static func parsePackage(_ item: Any) throws -> Package {
        guard let p = item as? [String: Any] else { throw FieldError("not an object") }
        guard let rawName = p["name"] as? String, let name = safeRelativeName(rawName) else { throw FieldError("no safe name") }
        guard let files = p["files"] as? [Any], !files.isEmpty else { throw FieldError("\(name) lists no files") }
        guard files.count <= FieldLimits.maxManifestFilesPerPackage else { throw FieldError("\(name) lists too many files") }
        var parsed: [File] = []
        for f in files {
            guard let fo = f as? [String: Any], let raw = fo["name"] as? String else { throw FieldError("\(name): a file has no name") }
            guard let fname = safeRelativeName(raw) else { throw FieldError("\(name): the file name \(raw.clipped()) is not allowed") }
            let ext = (fname as NSString).pathExtension.lowercased()
            let role = (fo["role"] as? String).map { $0.lowercased() } ?? (ext == "json" ? "sidecar" : ext)
            guard let n = fo["bytes"] as? NSNumber, CFGetTypeID(n) != CFBooleanGetTypeID() else { throw FieldError("\(name): \(fname) has no size") }
            let d = n.doubleValue
            guard d.isFinite, d >= 1, d <= Double(Int64.max) else { throw FieldError("\(name): \(fname) has an impossible size") }
            let bytes = n.int64Value
            let cap = role == "sidecar" ? FieldLimits.maxSidecarBytes : FieldLimits.maxModelBytes
            guard bytes <= cap else { throw FieldError("\(name): \(fname) is larger than this app accepts") }
            guard let sha = (fo["sha256"] as? String)?.lowercased(), sha.count == 64, sha.allSatisfy({ $0.isHexDigit && $0.isASCII }) else {
                throw FieldError("\(name): \(fname) has no valid SHA-256")
            }
            parsed.append(File(name: fname, role: role, bytes: bytes, sha256: sha))
        }
        let pkg = Package(name: name, label: ((p["label"] as? String).flatMap { $0.isEmpty ? nil : $0 } ?? name).clipped(),
                          level: (p["level"] as? String)?.clipped(), exported: (p["exported"] as? String)?.clipped(), files: parsed)
        guard pkg.model != nil else { throw FieldError("\(name) has no .glb or .usdz") }
        return pkg
    }

    /// A relative name the app will fetch: `/`-separated parts, none empty, `.` or `..`; no leading `/`, no `\`,
    /// no `:` or control characters, no `%` (so nothing is decoded twice); at most 512 characters and 8 parts.
    public static func safeRelativeName(_ s: String) -> String? {
        guard !s.isEmpty, s.count <= 512, !s.hasPrefix("/"), !s.contains("\\"), !s.contains(":"), !s.contains("%") else { return nil }
        guard !s.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) }) else { return nil }
        let parts = s.split(separator: "/", omittingEmptySubsequences: false)
        guard parts.count <= 8, parts.allSatisfy({ !$0.isEmpty && $0 != "." && $0 != ".." }) else { return nil }
        return s
    }
}

// MARK: - Server address

public enum UpdateServer {
    public static let defaultsKey = "updateServerAddress"

    /// The manifest URL for what the user typed: a full http(s) URL to the exports folder (which may carry a private
    /// path token) or to its manifest.json. Refuses anything that isn't on the local network.
    public static func manifestURL(_ text: String) throws -> URL {
        let t = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !t.isEmpty else { throw FieldError("Enter the office computer's address, for example http://192.168.1.20:8000/exports/") }
        guard t.count <= 2_000, var c = URLComponents(string: t), let scheme = c.scheme?.lowercased(), scheme == "http" || scheme == "https" else {
            throw FieldError("Enter a full address starting with http:// or https://, for example http://192.168.1.20:8000/exports/")
        }
        guard c.user == nil, c.password == nil else { throw FieldError("Leave the user name and password out of the address.") }
        guard let host = c.host, !host.isEmpty else { throw FieldError("The address has no computer name or IP address.") }
        guard isLocal(host) else {
            throw FieldError("CADaddy Field only updates over the local network. Use the office computer's local address (a 10.x, 172.16–31.x or 192.168.x IP, or a .local name).")
        }
        c.fragment = nil
        var path = c.percentEncodedPath
        if !path.lowercased().hasSuffix(".json") {
            if !path.hasSuffix("/") { path += "/" }
            path += "manifest.json"
        }
        c.percentEncodedPath = path
        guard let url = c.url else { throw FieldError("That address can't be used.") }
        return url
    }

    /// A file's URL: the manifest's folder plus the relative name, each part percent-encoded.
    public static func fileURL(manifest: URL, name: String) -> URL? {
        guard UpdateManifest.safeRelativeName(name) != nil else { return nil }
        var allowed = CharacterSet.urlPathAllowed
        allowed.remove(charactersIn: "/;?#%")
        let enc = name.split(separator: "/").compactMap { String($0).addingPercentEncoding(withAllowedCharacters: allowed) }.joined(separator: "/")
        guard var c = URLComponents(url: manifest, resolvingAgainstBaseURL: false) else { return nil }
        let dir = (c.percentEncodedPath as NSString).deletingLastPathComponent
        c.percentEncodedPath = (dir.hasSuffix("/") ? dir : dir + "/") + enc
        c.query = nil
        c.fragment = nil
        return c.url
    }

    /// Private, loopback, link-local or carrier-grade-NAT IPv4; loopback, link-local or unique-local IPv6;
    /// `.local` / `.lan` / `.home.arpa` names; single-label host names.
    public static func isLocal(_ hostIn: String) -> Bool {
        let host = hostIn.lowercased().trimmingCharacters(in: CharacterSet(charactersIn: "[]"))
        let quads = host.split(separator: ".", omittingEmptySubsequences: false)
        if quads.count == 4, quads.allSatisfy({ !$0.isEmpty && $0.count <= 3 && $0.allSatisfy(\.isASCII) && $0.allSatisfy(\.isNumber) }) {
            let o = quads.compactMap { Int($0) }
            guard o.count == 4, o.allSatisfy({ (0...255).contains($0) }) else { return false }
            switch (o[0], o[1]) {
            case (10, _), (127, _), (192, 168), (169, 254): return true
            case (172, 16...31): return true
            case (100, 64...127): return true
            default: return false
            }
        }
        if host.contains(":") {
            return host == "::1" || host.hasPrefix("fe8") || host.hasPrefix("fe9") || host.hasPrefix("fea") || host.hasPrefix("feb")
                || host.hasPrefix("fc") || host.hasPrefix("fd")
        }
        if host.hasSuffix(".local") || host.hasSuffix(".lan") || host.hasSuffix(".home.arpa") { return host.count > 6 }
        return !host.contains(".") && host.allSatisfy { $0.isASCII && ($0.isLetter || $0.isNumber || $0 == "-") } && !host.isEmpty
    }
}

// MARK: - Plan (the diff)

public struct UpdatePlanItem: Equatable, Sendable {
    public enum Action: Equatable, Sendable { case new, changed(UUID), unchanged(UUID) }
    public var package: UpdateManifest.Package
    public var action: Action
}

public enum UpdatePlanner {
    /// Matches each manifest package to a library package by its stored source name (the manifest name it was last
    /// updated from, else the file or folder name it was opened from) and decides new / changed / unchanged by the
    /// model's and the sidecar's SHA-256.
    public static func plan(_ m: UpdateManifest, records: [PackageRecord], sidecarHash: (PackageRecord) -> String?) -> [UpdatePlanItem] {
        var used = Set<UUID>()
        return m.packages.map { p in
            let match = records.first { !used.contains($0.id) && $0.remoteName == p.name }
                ?? records.first { !used.contains($0.id) && $0.remoteName == nil && sourceStem($0.sourceName) == p.baseName.lowercased() }
            guard let r = match else { return UpdatePlanItem(package: p, action: .new) }
            used.insert(r.id)
            let sameModel = p.model?.sha256 == r.sha256.lowercased()
            let theirs = p.sidecar?.sha256
            let ours = r.sidecarSHA256 ?? sidecarHash(r)
            return UpdatePlanItem(package: p, action: sameModel && theirs == ours ? .unchanged(r.id) : .changed(r.id))
        }
    }

    /// "Room 12.glb" / "Room 12.zip" / "Room 12" → "room 12".
    static func sourceStem(_ s: String) -> String {
        let ext = (s as NSString).pathExtension.lowercased()
        let stem = ["glb", "usdz", "zip", "json"].contains(ext) ? (s as NSString).deletingPathExtension : s
        return stem.lowercased()
    }
}

// MARK: - Download (capped, hashed as it arrives)

/// Fetches one URL to a file, refusing more bytes than expected and hashing on the way.
final class CappedDownload: NSObject, URLSessionDataDelegate, @unchecked Sendable {
    private let dest: URL
    private let maxBytes: Int64
    private var handle: FileHandle?
    private var hasher = SHA256()
    private var received: Int64 = 0
    private var failure: FieldError?
    private var continuation: CheckedContinuation<(Int64, String), Error>?
    private let lock = NSLock()

    init(dest: URL, maxBytes: Int64) { self.dest = dest; self.maxBytes = maxBytes }

    /// Size and SHA-256 of what was saved at `dest`.
    static func fetch(_ url: URL, to dest: URL, maxBytes: Int64, configuration: URLSessionConfiguration) async throws -> (Int64, String) {
        let d = CappedDownload(dest: dest, maxBytes: maxBytes)
        let session = URLSession(configuration: configuration, delegate: d, delegateQueue: nil)
        defer { session.finishTasksAndInvalidate() }
        return try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { (c: CheckedContinuation<(Int64, String), Error>) in
                d.lock.lock(); d.continuation = c; d.lock.unlock()
                var req = URLRequest(url: url)
                req.cachePolicy = .reloadIgnoringLocalAndRemoteCacheData
                session.dataTask(with: req).resume()
            }
        } onCancel: { session.invalidateAndCancel() }
    }

    func urlSession(_ session: URLSession, dataTask: URLSessionDataTask, didReceive response: URLResponse,
                    completionHandler: @escaping (URLSession.ResponseDisposition) -> Void) {
        if let h = response as? HTTPURLResponse, !(200...299).contains(h.statusCode) {
            fail(FieldError(h.statusCode == 404 ? "not found on the server (404)" : "the server answered \(h.statusCode)"))
            completionHandler(.cancel); return
        }
        if response.expectedContentLength > maxBytes { fail(FieldError("larger than the manifest says")); completionHandler(.cancel); return }
        FileManager.default.createFile(atPath: dest.path, contents: nil)
        handle = try? FileHandle(forWritingTo: dest)
        if handle == nil { fail(FieldError("could not be saved on this device")); completionHandler(.cancel); return }
        completionHandler(.allow)
    }

    func urlSession(_ session: URLSession, dataTask: URLSessionDataTask, didReceive data: Data) {
        guard failure == nil else { return }
        received += Int64(data.count)
        if received > maxBytes { fail(FieldError("larger than the manifest says")); dataTask.cancel(); return }
        hasher.update(data: data)
        do { try handle?.write(contentsOf: data) } catch { fail(FieldError("could not be saved on this device (is the storage full?)")); dataTask.cancel() }
    }

    func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: Error?) {
        try? handle?.close()
        handle = nil
        lock.lock(); let c = continuation; continuation = nil; lock.unlock()
        if let f = failure { c?.resume(throwing: f); return }
        if let e = error { c?.resume(throwing: Self.message(e)); return }
        let hex = hasher.finalize().map { String(format: "%02x", $0) }.joined()
        c?.resume(returning: (received, hex))
    }

    private func fail(_ e: FieldError) { if failure == nil { failure = e } }

    /// A plain message for a transport error.
    static func message(_ e: Error) -> FieldError {
        let code = (e as? URLError)?.code
        switch code {
        case .notConnectedToInternet?, .networkConnectionLost?, .dataNotAllowed?:
            return FieldError("No network. Join the office Wi-Fi and try again; everything on this device still works offline.")
        case .cannotFindHost?, .cannotConnectToHost?, .dnsLookupFailed?:
            return FieldError("The office computer can't be reached at that address. Check that it is on, serving the exports folder, and on the same Wi-Fi.")
        case .timedOut?: return FieldError("The office computer didn't answer in time.")
        case .cancelled?: return FieldError("The update was cancelled.")
        case .appTransportSecurityRequiresSecureConnection?:
            return FieldError("That address isn't on the local network.")
        case .secureConnectionFailed?, .serverCertificateUntrusted?, .serverCertificateHasBadDate?, .serverCertificateNotYetValid?, .serverCertificateHasUnknownRoot?:
            return FieldError("The office computer's https certificate isn't trusted. Use its http:// address on the local network.")
        default: return FieldError("The download failed (\((e as? URLError)?.code.rawValue ?? -1)).")
        }
    }
}

// MARK: - The update run

public struct UpdateReport: Equatable, Sendable {
    public var updated: [String] = []
    public var added: [String] = []
    public var unchanged: [String] = []
    /// Package name and the reason.
    public var failed: [(name: String, message: String)] = []
    /// Things the user should know (re-align needed, marks whose element is gone, manifest problems).
    public var notes: [String] = []

    public var summary: String {
        "\(updated.count) updated, \(added.count) new, \(unchanged.count) unchanged, \(failed.count) failed"
    }

    public static func == (a: UpdateReport, b: UpdateReport) -> Bool {
        a.updated == b.updated && a.added == b.added && a.unchanged == b.unchanged && a.notes == b.notes
            && a.failed.map(\.name) == b.failed.map(\.name) && a.failed.map(\.message) == b.failed.map(\.message)
    }
}

public struct UpdateProgress: Sendable {
    public var done: Int
    public var total: Int
    public var current: String
    public var fraction: Double { total == 0 ? 0 : Double(done) / Double(total) }
}

public final class PackageUpdater: @unchecked Sendable {
    let library: PackageLibrary
    let configuration: URLSessionConfiguration

    /// `configuration` is for tests (a URLProtocol stub); the app uses `defaultConfiguration()`.
    public init(library: PackageLibrary, configuration: URLSessionConfiguration = PackageUpdater.defaultConfiguration()) {
        self.library = library
        self.configuration = configuration
    }

    /// No cache, no cookies, no credentials stored; short timeouts; never waits for a connection in the background.
    public static func defaultConfiguration() -> URLSessionConfiguration {
        let c = URLSessionConfiguration.ephemeral
        c.timeoutIntervalForRequest = 15
        c.timeoutIntervalForResource = 600
        c.waitsForConnectivity = false
        c.requestCachePolicy = .reloadIgnoringLocalAndRemoteCacheData
        c.urlCache = nil
        c.httpCookieStorage = nil
        c.httpShouldSetCookies = false
        c.urlCredentialStorage = nil
        c.allowsCellularAccess = false
        return c
    }

    /// One update: fetch the manifest at `address`, then each new or changed package. A failure in one package is
    /// reported for it and leaves its old version working; an unreachable server or a bad manifest fails the run
    /// with a plain message and changes nothing.
    public func run(address: String, progress: @escaping @Sendable (UpdateProgress) -> Void = { _ in }) async throws -> UpdateReport {
        let manifestURL = try UpdateServer.manifestURL(address)
        let work = try library.makeIncomingFolder()
        defer { try? FileManager.default.removeItem(at: work) }
        progress(UpdateProgress(done: 0, total: 1, current: "Checking the office for updates…"))
        let mfile = work.appendingPathComponent("manifest.json")
        do { _ = try await CappedDownload.fetch(manifestURL, to: mfile, maxBytes: FieldLimits.maxManifestBytes, configuration: configuration) } catch let e as FieldError {
            if e.message.hasPrefix("not found") { throw FieldError("There is no manifest.json at that address. Run bim.export.ar.manifest on the exports folder and serve that folder.") }
            if e.message.hasPrefix("larger") { throw FieldError("The office's manifest.json is too large.") }
            if e.message.hasPrefix("the server answered") { throw FieldError("The office computer refused the request: \(e.message).") }
            if e.message.hasPrefix("could not be saved") { throw FieldError("The manifest \(e.message).") }
            throw e
        }
        let manifest = try UpdateManifest.parse(try Data(contentsOf: mfile))
        var report = UpdateReport()
        report.notes += manifest.problems
        let plan = UpdatePlanner.plan(manifest, records: library.list(), sidecarHash: { self.library.storedSidecarHash($0) })
        let todo = plan.filter { if case .unchanged = $0.action { return false } else { return true } }
        for item in plan { if case .unchanged = item.action { report.unchanged.append(item.package.label) } }
        for (i, item) in todo.enumerated() {
            try Task.checkCancellation()
            progress(UpdateProgress(done: i, total: todo.count, current: item.package.label))
            do {
                let result = try await apply(item, manifestURL: manifestURL, work: work)
                if case .new = item.action { report.added.append(item.package.label) } else { report.updated.append(item.package.label) }
                report.notes += result.notes.map { "\(item.package.label): \($0)" }
            } catch {
                report.failed.append((item.package.label, (error as? FieldError)?.message ?? "could not be updated"))
            }
        }
        progress(UpdateProgress(done: todo.count, total: max(todo.count, 1), current: "Done"))
        return report
    }

    func apply(_ item: UpdatePlanItem, manifestURL: URL, work: URL) async throws -> PackageLibrary.UpdateResult {
        let p = item.package
        guard let model = p.model else { throw FieldError("the package has no model") }
        let dir = work.appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }
        var got: [String: URL] = [:]
        for f in [model, p.sidecar].compactMap({ $0 }) {
            guard let url = UpdateServer.fileURL(manifest: manifestURL, name: f.name) else { throw FieldError("\(f.name) is not a safe file name") }
            let local = dir.appendingPathComponent(f.role + "." + ((f.name as NSString).pathExtension.isEmpty ? "bin" : (f.name as NSString).pathExtension))
            let (size, sha) = try await CappedDownload.fetch(url, to: local, maxBytes: f.bytes, configuration: configuration)
            guard size == f.bytes else { throw FieldError("\((f.name as NSString).lastPathComponent) is \(size) bytes, not the \(f.bytes) the manifest says") }
            guard sha == f.sha256 else { throw FieldError("\((f.name as NSString).lastPathComponent) doesn't match its SHA-256 (damaged in transfer or changed on the server)") }
            got[f.role] = local
        }
        guard let m = got[model.role] else { throw FieldError("the model was not downloaded") }
        let incoming = PackageLibrary.Incoming(modelURL: m, modelName: (model.name as NSString).lastPathComponent, sidecarURL: got["sidecar"],
                                               remoteName: p.name, label: p.label, exported: p.exported)
        switch item.action {
        case .new: return try library.applyUpdate(incoming, replacing: nil)
        case let .changed(id): return try library.applyUpdate(incoming, replacing: id)
        case .unchanged: return PackageLibrary.UpdateResult(record: nil, notes: [])
        }
    }
}

// MARK: - Anchor change

extension AnchorInfo {
    /// True when two anchors are the same real point: same kind, level and plan units, plan coordinates within 1/8"
    /// (0.0104 ft; the same tolerance in other units' terms). Two missing anchors are the same; a missing and a
    /// present one are not.
    public static func sameSpot(_ a: AnchorInfo?, _ b: AnchorInfo?) -> Bool {
        guard let a, let b else { return a == nil && b == nil }
        guard a.kind == b.kind, a.level == b.level, (a.planUnits ?? "ft") == (b.planUnits ?? "ft") else { return false }
        let tol = 0.003175 / (Self.metersPerUnit(a.planUnits) ?? 0.3048)
        switch (a.planX, a.planY, b.planX, b.planY) {
        case (nil, nil, nil, nil): return true
        case let (ax?, ay?, bx?, by?): return abs(ax - bx) <= tol && abs(ay - by) <= tol
        default: return false
        }
    }

    /// Metres per plan unit (ft by default; m, mm, in).
    static func metersPerUnit(_ u: String?) -> Double? {
        switch (u ?? "ft").lowercased() {
        case "ft", "feet", "foot", "'": return 0.3048
        case "in", "inch", "inches", "\"": return 0.0254
        case "m", "meter", "meters", "metre", "metres": return 1
        case "mm": return 0.001
        default: return nil
        }
    }

    /// Where `previous` sits in the model frame whose origin is this anchor (metres, Y up, plan y along −Z), when both
    /// have plan coordinates in the same known units on the same level.
    public func position(of previous: AnchorInfo) -> SIMD3<Float>? {
        guard let x = planX, let y = planY, let px = previous.planX, let py = previous.planY, level == previous.level,
              (planUnits ?? "ft") == (previous.planUnits ?? "ft"), let k = Self.metersPerUnit(planUnits) else { return nil }
        let v = SIMD3(Float((px - x) * k), 0, Float(-(py - y) * k))
        guard v.x.isFinite, v.z.isFinite, abs(v.x) < 10_000, abs(v.z) < 10_000 else { return nil }
        return v
    }
}
