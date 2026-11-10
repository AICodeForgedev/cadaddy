// CADaddy Field — debug-only screen automation for the design screenshots and the screen recording.
//   -design building|reveal|lifted|floor|room|card|trace|panel|layers|arch|archlifted|mepfull|tour
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
        if exp.session != nil { runSet(exp, shot: shot); return }
        let b = exp.building
        let room = "207"
        let outlet = b.elements.first { $0.room == room && $0.tag == "R1" }?.id ?? ""
        let panel = b.elements.first { $0.kind == "Panelboard" && $0.tag == "2LA" }?.id ?? ""
        func after(_ t: Double, _ f: @escaping @MainActor @Sendable () -> Void) { DispatchQueue.main.asyncAfter(deadline: .now() + t, execute: f) }
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
        case "gear":
            after(0.1) { exp.go(.room(room)) }
            after(1.0) { exp.tapElement(outlet) }
            after(1.6) { if let s = exp.focus?.steps.first(where: { $0.link == "Gear" }) { exp.flyTo(s) } }
        case "layers":
            after(0.1) { exp.go(.floor(2)) }
            after(1.0) { exp.showLayers = true }
        case "arch":
            exp.finishReveal()
            after(0.1) { exp.setView(.architectural) }
        case "archlifted":
            exp.finishReveal()
            after(0.1) { exp.setView(.architectural) }
            after(0.3) { exp.setLiftForScreenshot(2) }
        case "mepfull":
            exp.finishReveal()
            after(0.1) { exp.setDensity(.full) }
        case "tour":
            tour(exp, room: room, outlet: outlet)
        default:
            break
        }
    }

    /// The same screens on a real building's package set (`-openset FOLDER -design SHOT`). Picks its objects from the
    /// index by what they are (a light on the second level, its room, a panel, a rule-given link, a pipe nobody feeds).
    static func runSet(_ exp: BuildingExperience, shot: String) {
        guard let i = exp.session?.index else { return }
        func after(_ t: Double, _ f: @escaping @MainActor @Sendable () -> Void) { DispatchQueue.main.asyncAfter(deadline: .now() + t, execute: f) }
        let upper = min(1, max(i.levels.count - 1, 0))
        let light = i.graph.first { $0.kind == "light_fixture" && i.levelIndex($0.level) == upper && !$0.fedBy.isEmpty }
        let lightRoom = light.flatMap { l in i.rooms.first { r in r.package.map { p in l.packages.contains { i.allPackages[Int($0)].name == p } } ?? false } }
        let room = lightRoom?.id ?? i.rooms.first { i.levelIndex($0.level) == upper }?.id ?? i.rooms.first?.id ?? ""
        let panel = i.graph.first { $0.role == "panel" && !$0.fedBy.isEmpty }
        let inferred = i.graph.first { $0.fedBy.contains { $0.state == .inferred } }
        let inferredRoom = inferred.flatMap { e in i.rooms.first { r in r.package.map { p in e.packages.contains { i.allPackages[Int($0)].name == p } } ?? false } }
        let orphan = i.graph.first { $0.kind == "pipe" && $0.fedBy.isEmpty && $0.packages.contains { i.allPackages[Int($0)].kind == .mepMains } }
        switch shot {
        case "reveal": after(0.1) { exp.setRevealForScreenshot(0.46) }
        case "building": exp.finishReveal()
        case "lifted": exp.finishReveal(); after(0.2) { exp.setLiftForScreenshot(Float(upper)) }
        case "mepfull": exp.finishReveal(); after(0.1) { exp.setDensity(.full) }
        case "arch": exp.finishReveal(); after(0.1) { exp.setView(.architectural) }
        case "archlifted": exp.finishReveal(); after(0.1) { exp.setView(.architectural) }; after(0.6) { exp.setLiftForScreenshot(Float(upper)) }
        case "floor": after(0.1) { exp.go(.floor(upper)) }
        case "floor1": after(0.1) { exp.go(.floor(0)) }
        case "room": after(0.1) { exp.go(.room(room)) }
        case "card": after(0.1) { exp.go(.room(room)) }; after(1.6) { if let l = light { exp.tapElement(l.id) } }
        case "trace": after(0.1) { exp.go(.floor(upper)) }; after(1.6) { if let l = light { exp.tapElement(l.id) } }
        case "gear":
            after(0.1) { exp.go(.floor(upper)) }
            after(1.6) { if let l = light { exp.tapElement(l.id) } }
            after(2.4) { if let st = exp.focus?.steps.last(where: { !$0.id.isEmpty }) { exp.flyTo(st) } }
        case "panel": after(0.1) { exp.go(.floor(0)) }; after(1.6) { if let p = panel { exp.tapElement(p.id) } }
        case "inferred":
            after(0.1) { if let r = inferredRoom { exp.go(.room(r.id)) } }
            after(1.6) { if let e = inferred { exp.tapElement(e.id) } }
        case "unknown": exp.finishReveal(); after(0.4) { if let o = orphan { exp.tapElement(o.id) } }
        case "layers": after(0.1) { exp.go(.floor(upper)) }; after(1.4) { exp.showLayers = true }
        case "roomfurniture": after(0.1) { exp.go(.room(room)) }; after(1.0) { exp.setLayer(.furniture, on: true) }
        case "tour": tourSet(exp, upper: upper, room: room, light: light?.id)
        default: break
        }
    }

    static func tourSet(_ exp: BuildingExperience, upper: Int, room: String, light: String?) {
        func at(_ t: Double, _ f: @escaping @MainActor @Sendable () -> Void) { DispatchQueue.main.asyncAfter(deadline: .now() + t, execute: f) }
        at(4.2) { exp.renderer.autoOrbitSpeed = 0.32 }
        at(9.0) { exp.renderer.autoOrbitSpeed = 0 }
        for (k, f) in stride(from: Float(0.02), through: 0.9, by: 0.03).enumerated() { at(9.4 + Double(k) * 0.06) { exp.railChanged(f) } }
        at(11.6) { exp.railEnded() }
        at(12.6) { exp.railTap(upper) }
        at(15.6) { exp.tapRoom(room) }
        at(18.6) { if let light { exp.tapElement(light) } }
        at(21.6) { if let s = exp.focus?.steps.last(where: { !$0.id.isEmpty }) { exp.flyTo(s) } }
        at(25.0) { exp.showLayers = true }
        at(26.0) { exp.setLayer(.mechanical, on: false) }
        at(27.4) { exp.setLayer(.mechanical, on: true) }
        at(28.4) { exp.showLayers = false }
        at(29.0) { exp.clearFocus(); exp.go(.building) }
        at(31.0) { exp.setDensity(.full) }
        at(34.0) { exp.setView(.architectural) }
        at(37.0) { exp.setView(.mep) }
        at(38.0) { exp.setDensity(.mains) }
    }

    /// The whole flow for the recording (~36 s).
    static func tour(_ exp: BuildingExperience, room: String, outlet: String) {
        func at(_ t: Double, _ f: @escaping @MainActor @Sendable () -> Void) { DispatchQueue.main.asyncAfter(deadline: .now() + t, execute: f) }
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
        // 35: the view control: Full MEP, Architectural, back to MEP.
        at(35.0) { exp.setDensity(.full) }
        at(38.0) { exp.renderer.autoOrbitSpeed = 0.25 }
        at(40.5) { exp.renderer.autoOrbitSpeed = 0 }
        at(41.0) { exp.setView(.architectural) }
        for (k, f) in stride(from: Float(0.02), through: 0.62, by: 0.02).enumerated() {
            at(43.5 + Double(k) * 0.06) { exp.railChanged(f) }
        }
        at(45.6) { exp.railEnded() }
        at(47.0) { exp.setView(.mep) }
        at(48.0) { exp.setDensity(.mains) }
    }
}
#endif
