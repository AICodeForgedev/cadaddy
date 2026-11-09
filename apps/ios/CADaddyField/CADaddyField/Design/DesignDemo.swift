// CADaddy Field — debug-only screen automation for the design screenshots and the screen recording.
//   -design building|reveal|lifted|floor|room|card|trace|panel|layers|tour
//   -designbg white|teal   -designhc   (-fpslog prints FIELD-PERF lines)

#if DEBUG
import Foundation
import UIKit

@available(iOS 18.0, *)
@MainActor
enum DesignDemo {
    static var shot: String? {
        let a = ProcessInfo.processInfo.arguments
        guard let i = a.firstIndex(of: "-design"), a.indices.contains(i + 1) else { return nil }
        return a[i + 1]
    }

    static func run(_ exp: BuildingExperience) {
        let a = ProcessInfo.processInfo.arguments
        guard let shot else { return }
        if let i = a.firstIndex(of: "-designbg"), a.indices.contains(i + 1) {
            exp.setBackground(a[i + 1] == "white" ? .white : .teal)
        }
        exp.setHighContrast(a.contains("-designhc"))
        let b = exp.building
        let room = "207"
        let outlet = b.elements.first { $0.room == room && $0.tag == "R1" }?.id ?? ""
        let panel = b.elements.first { $0.kind == "Panelboard" && $0.tag == "2LA" }?.id ?? ""
        func after(_ t: Double, _ f: @escaping () -> Void) { DispatchQueue.main.asyncAfter(deadline: .now() + t, execute: f) }
        switch shot {
        case "reveal":
            after(0.1) { exp.setRevealForScreenshot(0.46) }
        case "building":
            exp.finishReveal()
        case "lifted":
            exp.finishReveal()
            after(0.2) { exp.setLiftForScreenshot(2) }
        case "floor":
            after(0.1) { exp.go(.floor(2)) }
        case "room":
            after(0.1) { exp.go(.room(room)) }
        case "card":
            after(0.1) { exp.go(.room(room)) }
            after(1.2) { exp.tapElement(outlet) }
        case "trace":
            after(0.1) { exp.go(.floor(2)) }
            after(1.2) { exp.tapElement(outlet) }
        case "panel":
            after(0.1) { exp.go(.floor(2)) }
            after(1.2) { exp.tapElement(panel) }
        case "layers":
            after(0.1) { exp.go(.floor(2)) }
            after(1.0) { exp.showLayers = true }
        case "tour":
            tour(exp, room: room, outlet: outlet)
        default:
            break
        }
    }

    /// The whole flow for the recording (~36 s).
    static func tour(_ exp: BuildingExperience, room: String, outlet: String) {
        func at(_ t: Double, _ f: @escaping () -> Void) { DispatchQueue.main.asyncAfter(deadline: .now() + t, execute: f) }
        // 0-4 s: the solid building, then the reveal sweeps it to X-ray.
        // 4-10 s: a slow orbit (the ghost holds still: no flicker, no layer swaps).
        at(4.2) { exp.renderer.autoOrbitSpeed = 0.32 }
        at(10.0) { exp.renderer.autoOrbitSpeed = 0 }
        // 10-13 s: the rail, thumb going down, floors lifting apart with a detent per floor.
        for (k, f) in stride(from: Float(0.02), through: 0.62, by: 0.02).enumerated() {
            at(10.4 + Double(k) * 0.06) { exp.railChanged(f) }
        }
        at(12.5) { exp.railEnded() }
        // 13.5: into level 2.
        at(13.6) { exp.railTap(2) }
        // 16: a room rises, the camera goes in.
        at(16.2) { exp.tapRoom(room) }
        // 18.5: tap an outlet: card + upstream trace.
        at(18.8) { exp.tapElement(outlet) }
        // 21.5: follow the path to the gear (crosses floors), then the transformer.
        at(21.6) { if let s = exp.focus?.steps.first(where: { $0.link == "Gear" }) { exp.flyTo(s) } }
        at(24.6) { if let s = exp.focus?.steps.first(where: { $0.link == "Panel" }) { exp.flyTo(s) } }
        // 27: layers: mechanical off and back on.
        at(27.2) { exp.showLayers = true }
        at(28.2) { exp.setLayer(.mechanical, on: false) }
        at(29.6) { exp.setLayer(.mechanical, on: true) }
        at(30.6) { exp.showLayers = false }
        // 31: back up to the building.
        at(31.2) { exp.clearFocus(); exp.goUp() }
        at(33.0) { exp.goUp() }
    }
}
#endif
