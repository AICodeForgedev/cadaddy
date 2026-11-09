// CADaddy Field — the building experience's palette. Disciplined: one colour per system, a pale ghost for the
// architecture, neutral equipment. Two backgrounds for the owner to choose between, and a high-contrast mode for sun.

import SwiftUI
import UIKit

enum DesignBackground: String, CaseIterable, Identifiable {
    case teal = "Deep Teal", white = "Studio White"
    var id: String { rawValue }
}

struct DesignStyle: Equatable {
    var background: DesignBackground = .teal
    var highContrast = false

    var isDark: Bool { background == .teal }

    // MARK: backdrop (painted by the post-process pass)

    var bgTop: SIMD3<Float> {
        switch (background, highContrast) {
        case (.teal, false): return hex3(0x0C1A19)
        case (.teal, true): return hex3(0x020606)
        case (.white, false): return hex3(0xFFFFFF)
        case (.white, true): return hex3(0xFFFFFF)
        }
    }
    var bgBottom: SIMD3<Float> {
        switch (background, highContrast) {
        case (.teal, false): return hex3(0x17504B)
        case (.teal, true): return hex3(0x0A1F1D)
        case (.white, false): return hex3(0xE4E9EE)
        case (.white, true): return hex3(0xF2F4F6)
        }
    }
    /// The flat colour RealityKit clears to; the post pass swaps it for the gradient.
    var clear: SIMD3<Float> { (bgTop + bgBottom) / 2 }

    // MARK: architecture

    /// The ghost: one colour and one opacity for every shell face, so the blend is the same in any draw order.
    var ghost: UIColor { isDark ? UIColor(hex: 0xBFEDE6) : UIColor(hex: 0x6F8296) }
    var ghostOpacity: Float { highContrast ? 0.12 : (isDark ? 0.07 : 0.07) }
    var edge: UIColor {
        switch (background, highContrast) {
        case (.teal, false): return UIColor(hex: 0x4F8A83)
        case (.teal, true): return UIColor(hex: 0xD6F5F0)
        case (.white, false): return UIColor(hex: 0xB9C4CF)
        case (.white, true): return UIColor(hex: 0x3A4652)
        }
    }
    var floorPlate: UIColor { isDark ? UIColor(hex: 0x1F3D3A) : UIColor(hex: 0xEEF1F4) }
    var roomTile: UIColor { isDark ? UIColor(hex: 0x2B5651) : UIColor(hex: 0xDCE5EC) }
    var roomTileRaised: UIColor { isDark ? UIColor(hex: 0x3F7A73) : UIColor(hex: 0xC5D6E4) }

    // Exterior, for the opening reveal only.
    var facade: UIColor { UIColor(hex: 0xE4E0D8) }
    var facadeDark: UIColor { UIColor(hex: 0xB9B4AA) }
    var glass: UIColor { UIColor(hex: 0x30414C) }

    // MARK: systems

    func color(_ s: BuildingSystem) -> UIColor {
        switch s {
        case .mechanical: return UIColor(hex: highContrast ? 0x1F5BFF : 0x2F6BFF)
        case .electrical: return UIColor(hex: highContrast ? 0xFF7A1F : 0xFF8A3D)
        case .lowVoltage: return UIColor(hex: 0xFFB57F)
        case .plumbing: return UIColor(hex: highContrast ? 0x14C95C : 0x2FD36F)
        case .fire: return UIColor(hex: 0xFF3D7F)
        case .cabinets: return UIColor(hex: 0xC4A27A)
        case .furniture: return UIColor(hex: 0xBDB6AC)
        case .architecture: return ghost
        }
    }

    /// Equipment: neutral light grey with a touch of its system colour (about 14 %).
    func equipment(_ s: BuildingSystem) -> UIColor { UIColor(hex: 0xDCE0E5).mixed(with: color(s), 0.14) }

    /// Branches on the floor view: the system colour, quieter than the mains.
    func branch(_ s: BuildingSystem) -> UIColor {
        color(s).mixed(with: isDark ? UIColor(hex: 0x17504B) : .white, highContrast ? 0.12 : 0.32)
    }

    var lightWarm: UIColor { UIColor(hex: 0xFFD596) }

    // MARK: type and chrome (SwiftUI)

    var ink: Color { isDark ? .white : Color(uiColor: UIColor(hex: 0x0E1A24)) }
    var inkSecondary: Color { isDark ? Color.white.opacity(highContrast ? 0.9 : 0.62) : Color(uiColor: UIColor(hex: 0x0E1A24)).opacity(highContrast ? 0.85 : 0.55) }
    var hairline: Color { isDark ? Color.white.opacity(highContrast ? 0.5 : 0.14) : Color.black.opacity(highContrast ? 0.45 : 0.1) }
    var colorScheme: ColorScheme { isDark ? .dark : .light }

    func swiftUI(_ s: BuildingSystem) -> Color { Color(uiColor: color(s)) }

    private func hex3(_ v: UInt32) -> SIMD3<Float> {
        SIMD3(Float((v >> 16) & 0xFF) / 255, Float((v >> 8) & 0xFF) / 255, Float(v & 0xFF) / 255)
    }
}

extension UIColor {
    convenience init(hex: UInt32, alpha: CGFloat = 1) {
        self.init(red: CGFloat((hex >> 16) & 0xFF) / 255, green: CGFloat((hex >> 8) & 0xFF) / 255, blue: CGFloat(hex & 0xFF) / 255, alpha: alpha)
    }

    func mixed(with other: UIColor, _ t: CGFloat) -> UIColor {
        var r1: CGFloat = 0, g1: CGFloat = 0, b1: CGFloat = 0, a1: CGFloat = 0
        var r2: CGFloat = 0, g2: CGFloat = 0, b2: CGFloat = 0, a2: CGFloat = 0
        getRed(&r1, green: &g1, blue: &b1, alpha: &a1)
        other.getRed(&r2, green: &g2, blue: &b2, alpha: &a2)
        return UIColor(red: r1 + (r2 - r1) * t, green: g1 + (g2 - g1) * t, blue: b1 + (b2 - b1) * t, alpha: a1 + (a2 - a1) * t)
    }
}

extension BuildingSystem {
    var displayName: String {
        switch self {
        case .architecture: return "Architecture"
        case .electrical: return "Electrical"
        case .lowVoltage: return "Low voltage"
        case .mechanical: return "Mechanical"
        case .plumbing: return "Plumbing"
        case .fire: return "Fire protection"
        case .cabinets: return "Cabinets"
        case .furniture: return "Furniture"
        }
    }
    var symbol: String {
        switch self {
        case .architecture: return "building.2"
        case .electrical: return "bolt.fill"
        case .lowVoltage: return "cable.connector"
        case .mechanical: return "fan.fill"
        case .plumbing: return "drop.fill"
        case .fire: return "flame.fill"
        case .cabinets: return "cabinet.fill"
        case .furniture: return "bed.double.fill"
        }
    }
}
