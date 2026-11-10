// CADaddy Field — state for the building / floor / room experience. The renderer draws it; SwiftUI shows the chrome.

import Combine
import RealityKit
import SwiftUI
import UIKit

/// The M / E / P / Cabinets / Furniture switches. One store for every screen, AR included, kept on the device.
@MainActor
final class LayerStore: ObservableObject {
    static let shared = LayerStore()
    @Published var prefs = LayerPrefs.load() { didSet { if prefs != oldValue { prefs.save() } } }
}

@available(iOS 18.0, *)
@MainActor
final class BuildingExperience: ObservableObject {
    let building = SampleBuilding.sample()
    let renderer = BuildingRenderer()

    @Published private(set) var scope: DesignScope = .building
    @Published var style: DesignStyle
    @Published private(set) var layers: LayerPrefs
    @Published private(set) var focus: SystemFocus?
    @Published var selected: String?
    @Published private(set) var liftFocus: Float?
    @Published private(set) var railDetent: Int?
    @Published private(set) var revealProgress: Float = 0
    @Published private(set) var revealDone = false
    @Published var revealLine: (CGPoint, CGPoint)?
    @Published var fps: Double = 0
    @Published var search = ""
    @Published var hiddenGroups: Set<BuildingSystem> = []
    @Published var showLayers = false

    private var store = LayerStore.shared
    private var subs: Set<AnyCancellable> = []
    private var revealClock: Double?
    private var revealDragBase: Float = 0
    private var revealIdle: Double = 0
    private var attached = false
    private let selection = UISelectionFeedbackGenerator()
    private let impact = UIImpactFeedbackGenerator(style: .soft)
    private let light = UIImpactFeedbackGenerator(style: .light)

    static let backgroundKey = "field.design.background"
    static let contrastKey = "field.design.highContrast"

    init() {
        var s = DesignStyle()
        if let raw = UserDefaults.standard.string(forKey: Self.backgroundKey), let bg = DesignBackground(rawValue: raw) { s.background = bg }
        s.highContrast = UserDefaults.standard.bool(forKey: Self.contrastKey) || UIAccessibility.isDarkerSystemColorsEnabled
        style = s
        layers = LayerStore.shared.prefs
        store.$prefs.dropFirst().sink { [weak self] p in
            guard let self else { return }
            self.layers = p
            self.renderer.applyLayers()
            self.renderer.refreshLabels(self.scope)
        }.store(in: &subs)
    }

    var currentRoom: String? { if case let .room(n) = scope { return n } else { return nil } }
    var currentLevel: Int? {
        switch scope {
        case .building: return nil
        case let .floor(i): return i
        case let .room(n): return building.room(n)?.level
        }
    }

    // MARK: lifecycle

    func attach(_ v: ARView) {
        guard !attached else { return }
        attached = true
        renderer.attach(v, experience: self)
        renderer.apply(scope: .building, liftFocus: nil)
        renderer.setReveal(0)
        if UIAccessibility.isReduceMotionEnabled {
            finishReveal()
        } else {
            revealClock = -0.9   // a beat on the solid building, then the sweep
        }
        #if DEBUG
        DesignDemo.run(self)
        #endif
    }

    func detach() { renderer.stop() }

    func tick(_ dt: Double) {
        if let c = revealClock {
            let t = c + dt
            revealClock = t
            if t > 0 {
                let u = Float(min(t / 2.8, 1))
                let eased = u * u * (3 - 2 * u)
                setRevealProgress(max(revealDragBase, revealDragBase + (1 - revealDragBase) * eased))
                if u >= 1 { finishReveal() }
            }
        } else if !revealDone, revealIdle > 0 {
            revealIdle -= dt
            if revealIdle <= 0 { revealDragBase = revealProgress; revealClock = 0 }
        }
    }

    // MARK: reveal

    private func setRevealProgress(_ p: Float) {
        revealProgress = min(max(p, 0), 1)
        renderer.setReveal(revealProgress)
    }

    func beginRevealDrag() { revealClock = nil; revealIdle = 0; revealDragBase = revealProgress }
    func dragReveal(_ delta: Float) { setRevealProgress(revealDragBase + delta) }
    func endRevealDrag() {
        if revealProgress >= 0.999 { finishReveal(); return }
        revealDragBase = revealProgress
        revealIdle = 1.4   // resume on its own if the finger leaves it part way
    }

    func finishReveal() {
        revealClock = nil
        revealIdle = 0
        revealDragBase = 0
        setRevealProgress(1)
        revealDone = true
        revealLine = nil
    }

    func replayReveal() {
        go(.building)
        revealDone = false
        revealDragBase = 0
        setRevealProgress(0)
        revealClock = UIAccessibility.isReduceMotionEnabled ? nil : -0.4
        if UIAccessibility.isReduceMotionEnabled { finishReveal() }
    }

    func setRevealForScreenshot(_ p: Float) { revealClock = nil; revealIdle = 0; revealDone = false; setRevealProgress(p) }

    // MARK: navigation

    func go(_ s: DesignScope, keepFocus: Bool = false) {
        if !revealDone { finishReveal() }
        if !keepFocus { clearFocus() }
        if case .building = s {} else { liftFocus = nil; railDetent = nil }
        scope = s
        selected = keepFocus ? selected : nil
        search = ""
        renderer.apply(scope: s, liftFocus: liftFocus)
        if keepFocus { renderer.apply(focus: focus) }
        light.impactOccurred(intensity: 0.6)
    }

    func goUp() {
        switch scope {
        case .building:
            if focus != nil { clearFocus() } else if liftFocus != nil { liftFocus = nil; railDetent = nil; renderer.apply(scope: .building, liftFocus: nil) }
        case let .floor(i): go(.building); _ = i
        case let .room(n): go(.floor(building.room(n)?.level ?? 1))
        }
    }

    func tapRoom(_ number: String) {
        guard building.room(number) != nil else { return }
        if currentRoom == number { return }
        impact.impactOccurred()
        if case .floor = scope, !UIAccessibility.isReduceMotionEnabled {
            renderer.raiseRoom(number)
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.22) { [weak self] in self?.go(.room(number)) }
        } else {
            go(.room(number))
        }
    }

    // MARK: follow the system

    func tapElement(_ id: String) {
        if focus?.tapped == id { clearFocus(); return }
        let f = building.focus(id)
        focus = f
        selected = id
        renderer.clearFocusVisuals()
        renderer.apply(focus: f)
        light.impactOccurred()
    }

    func clearFocus() {
        guard focus != nil || selected != nil else { return }
        focus = nil
        selected = nil
        renderer.clearFocusVisuals()
        renderer.apply(scope: scope, liftFocus: liftFocus)
    }

    /// A step in the path strip: fly there, crossing floors in the same camera.
    func flyTo(_ step: TraceStep) {
        guard let e = building.element(step.id) else { return }
        let target: DesignScope
        switch e.level {
        case 1...4:
            if let r = e.room, e.detail == .room, building.room(r) != nil { target = .room(r) } else { target = .floor(e.level) }
        case 5: target = .floor(5)
        default: target = .building
        }
        if target != scope { go(target, keepFocus: true) }
        selected = step.id
        renderer.frame(element: step.id)
        selection.selectionChanged()
    }

    // MARK: floor rail

    func railChanged(_ fraction: Float) {
        if scope != .building { go(.building) }
        if !revealDone { finishReveal() }
        let n = building.levels.count
        let f = FloorLift.focus(railFraction: fraction, levels: n)
        let d = FloorLift.detent(f, levels: n)
        if d != railDetent { selection.selectionChanged(); railDetent = d }
        liftFocus = f
        renderer.apply(scope: .building, liftFocus: f)
    }

    func railEnded() {
        guard let d = railDetent else { return }
        liftFocus = Float(d)
        renderer.apply(scope: .building, liftFocus: liftFocus)
    }

    func railTap(_ level: Int) {
        selection.selectionChanged()
        go(.floor(level))
    }

    func setLiftForScreenshot(_ f: Float) { liftFocus = f; railDetent = Int(f.rounded()); renderer.apply(scope: .building, liftFocus: f) }

    // MARK: layers and look

    func setLayer(_ l: FieldLayer, on: Bool) { store.prefs.set(l, on: on) }

    func setBackground(_ bg: DesignBackground) {
        style.background = bg
        UserDefaults.standard.set(bg.rawValue, forKey: Self.backgroundKey)
        renderer.restyle(style)
    }

    func setHighContrast(_ on: Bool) {
        style.highContrast = on
        UserDefaults.standard.set(on, forKey: Self.contrastKey)
        renderer.restyle(style)
    }

    // MARK: text

    var title: String {
        switch scope {
        case .building: return building.name
        case let .floor(i): return building.level(i)?.name ?? "Level"
        case let .room(n): return building.room(n).map { $0.name == "Guest Room" ? "Room \($0.number)" : $0.name } ?? n
        }
    }

    var statLine: String {
        switch scope {
        case .building:
            let floors = building.levels.filter { $0.height > 0 }.count
            return "\(floors) floors · \(building.occupiedRoomCount) rooms · \(building.elements.count.formatted()) objects"
        case let .floor(i):
            let rooms = building.rooms(on: i).filter { $0.name != "Corridor" }
            let n = building.elements.filter { $0.level == i }.count
            return rooms.isEmpty ? "\(n) objects" : "\(rooms.count) rooms · \(n.formatted()) objects"
        case let .room(num):
            guard let r = building.room(num) else { return "" }
            let w = Units.feetInches(Double(r.max.x - r.min.x)), d = Units.feetInches(Double(r.max.y - r.min.y))
            return "\(r.name) · \(w) × \(d) · \(building.elements(in: num).count) objects"
        }
    }
}
