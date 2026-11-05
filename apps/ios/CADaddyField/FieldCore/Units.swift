// CADaddy Field — unit conversions between metres (the package unit) and feet-inches (US field unit).

import Foundation

public enum Units {
    public static let metersPerInch = 0.0254
    public static let metersPerFoot = 0.3048

    public static func inches(fromMeters m: Double) -> Double { m / metersPerInch }
    public static func meters(fromInches inch: Double) -> Double { inch * metersPerInch }
    public static func meters(fromFeet ft: Double) -> Double { ft * metersPerFoot }

    /// Formats metres as feet-inches rounded to 1/`denominator` inch (default 1/8"), e.g. `8' 6 1/2"`,
    /// `0' 3/8"`, `-1' 2"`. Non-finite input gives "—".
    public static func feetInches(_ meters: Double, denominator: Int = 8) -> String {
        guard meters.isFinite, denominator > 0, denominator <= 64 else { return "—" }
        let negative = meters < 0
        let totalUnits = (abs(meters) / metersPerInch * Double(denominator)).rounded()
        guard totalUnits < 1e12 else { return "—" }
        let units = Int(totalUnits)
        let perFoot = 12 * denominator
        let feet = units / perFoot
        let rem = units % perFoot
        let whole = rem / denominator
        var num = rem % denominator
        var den = denominator
        while num > 0 && num % 2 == 0 && den % 2 == 0 { num /= 2; den /= 2 }
        let sign = (negative && units > 0) ? "-" : ""
        if num == 0 { return "\(sign)\(feet)' \(whole)\"" }
        if whole == 0 { return "\(sign)\(feet)' \(num)/\(den)\"" }
        return "\(sign)\(feet)' \(whole) \(num)/\(den)\""
    }

    /// Formats metres as decimal inches, e.g. `0.42 in`.
    public static func inchesText(_ meters: Double, digits: Int = 2) -> String {
        guard meters.isFinite else { return "—" }
        return String(format: "%.\(max(0, min(digits, 4)))f in", inches(fromMeters: meters))
    }

    /// Parses a length typed or exported as text into metres. Accepts feet-inches
    /// (`8'6"`, `8' 6 1/2"`, `8'-6 1/2"`, `102"`, `6 1/2"`, `8.5'`, `3/8"`) and metric
    /// (`2.59 m`, `259 cm`, `2590 mm`). A bare number is metres. Returns nil when unreadable.
    public static func parseLength(_ text: String) -> Double? {
        let s = text.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
            .replacingOccurrences(of: "″", with: "\"").replacingOccurrences(of: "′", with: "'")
            .replacingOccurrences(of: "”", with: "\"").replacingOccurrences(of: "’", with: "'")
        guard !s.isEmpty, s.count <= 64 else { return nil }
        for (suffix, factor) in [("mm", 0.001), ("cm", 0.01), ("m", 1.0)] where s.hasSuffix(suffix) {
            let body = s.dropLast(suffix.count).trimmingCharacters(in: .whitespaces)
            guard let v = Double(body), v.isFinite else { return nil }
            return v * factor
        }
        if !s.contains("'") && !s.contains("\"") {
            guard let v = Double(s), v.isFinite else { return nil }
            return v
        }
        var negative = false
        var rest = Substring(s)
        if rest.hasPrefix("-") { negative = true; rest = rest.dropFirst() }
        var feet = 0.0
        if let q = rest.firstIndex(of: "'") {
            guard let f = Double(rest[rest.startIndex..<q].trimmingCharacters(in: .whitespaces)), f.isFinite else { return nil }
            feet = f
            rest = rest[rest.index(after: q)...]
            rest = Substring(rest.trimmingCharacters(in: .whitespaces))
            if rest.hasPrefix("-") { rest = Substring(rest.dropFirst().trimmingCharacters(in: .whitespaces)) }
        }
        var inches = 0.0
        let inchText = rest.replacingOccurrences(of: "\"", with: "").trimmingCharacters(in: .whitespaces)
        if !inchText.isEmpty {
            guard let v = parseInchNumber(inchText) else { return nil }
            inches = v
        }
        let total = meters(fromFeet: feet) + meters(fromInches: inches)
        guard total.isFinite else { return nil }
        return negative ? -total : total
    }

    /// `6`, `6.5`, `6 1/2`, `1/2`.
    private static func parseInchNumber(_ s: String) -> Double? {
        let parts = s.split(separator: " ", omittingEmptySubsequences: true)
        guard !parts.isEmpty, parts.count <= 2 else { return nil }
        var total = 0.0
        for p in parts {
            if p.contains("/") {
                let nd = p.split(separator: "/")
                guard nd.count == 2, let n = Double(nd[0]), let d = Double(nd[1]), d != 0, n.isFinite, d.isFinite else { return nil }
                total += n / d
            } else {
                guard let v = Double(p), v.isFinite else { return nil }
                total += v
            }
        }
        return total
    }
}
