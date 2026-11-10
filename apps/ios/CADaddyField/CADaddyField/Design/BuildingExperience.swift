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
    let building: SampleBuilding
    /// The four views' rules over this building (tiers computed once).
    let filter: ViewFilter
    let renderer = BuildingRenderer()

    @Published private(set) var scope: DesignScope = .building
    /// At whole-building scope: Architectural or MEP. A model opens in MEP, mains and big gear.
    @Published private(set) var wholeView: FieldView = .mep
    @Published private(set) var density: MEPDensity = .mains
    /// Where Floor and Room go from the view control: the last floor and room visited.
    private(set) var lastLevel: Int = 2
    private(set) var lastRoom: String?
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
    @Published var showLayers = false { didSet { if !showLayers { overlay = nil } } }
    /// Where the chrome is on this screen (measured by the view, laid out by ChromeLayout).
    @Published private(set) var chrome: ChromeLayout?
    /// A floating panel over the scene (the layers panel), kept clear of labels.
    var overlay: CGRect?
    /// The room sheet pulled down to its handle and primary button.
    @Published var sheetCollapsed = false

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
        let t0 = CFAbsoluteTimeGetCurrent()
        let b = SampleBuilding.sample()
        building = b
        let t1 = CFAbsoluteTimeGetCurrent()
        filter = ViewFilter(b)
        #if DEBUG
        if ProcessInfo.processInfo.arguments.contains("-fpslog") {
            print(String(format: "FIELD-INIT sample %.1f ms filter %.1f ms", (t1 - t0) * 1000, (CFAbsoluteTimeGetCurrent() - t1) * 1000))
        }
        #endif
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

    /// The active view (one of the four) with what it looks at.
    var viewState: ViewState {
        switch scope {
        case .building: return ViewState(view: wholeView, density: density)
        case let .floor(i): return ViewState(view: .floor, level: i)
        case let .room(n): return ViewState(view: .room, level: building.room(n)?.level, room: n)
        }
    }

    var activeView: FieldView { viewState.view }

    /// How many objects the active view draws. Worked out off the main thread (memoised per view and switch
    /// state); until it arrives the header keeps the last number, so a view change never waits on it.
    var drawnCount: Int {
        let st = viewState, prefs = layers
        let key = "\(st.view)|\(st.density)|\(st.level ?? -1)|\(st.room ?? "")|\(prefs.off.map(\.rawValue).sorted())"
        if let n = countCache[key] { lastCount = n; return n }
        if !counting.contains(key) {
            counting.insert(key)
            let f = filter
            Task.detached(priority: .utility) {
                let n = f.drawn(st, prefs: prefs).count
                await MainActor.run { [weak self] in
                    guard let self else { return }
                    self.countCache[key] = n
                    self.counting.remove(key)
                    self.objectWillChange.send()
                }
            }
        }
        return lastCount
    }
    private var countCache: [String: Int] = [:]
    private var counting: Set<String> = []
    private var lastCount = 0

    /// Pipes MEP leaves out because the model gives no size (the visible note in Full MEP).
    var unknownSizePipes: Int { filter.classifier.unknownSizeCount(building) }

    /// The view control: Architectural and MEP show the whole building; Floor and Room open the last ones visited.
    func setView(_ v: FieldView) {
        guard v != activeView else { return }
        selection.selectionChanged()
        switch v {
        case .architectural, .mep:
            wholeView = v
            if scope == .building { clearFocus(); renderer.apply(scope: .building, liftFocus: liftFocus) } else { go(.building) }
        case .floor:
            go(.floor(currentLevel ?? lastLevel))
        case .room:
            let level = currentLevel ?? lastLevel
            let room = lastRoom.flatMap { n in building.room(n).map { $0.level == level ? n : nil } ?? nil }
                ?? building.rooms(on: level).first { $0.name == "Guest Room" }?.number
                ?? lastRoom
            if let room { go(.room(room)) }
        }
    }

    func setDensity(_ d: MEPDensity) {
        guard d != density else { return }
        density = d
        selection.selectionChanged()
        if scope == .building { renderer.apply(scope: .building, liftFocus: liftFocus) }
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

    /// The view measured its chrome: frame the scene into what is left.
    func setChrome(_ c: ChromeLayout) {
        guard c != chrome else { return }
        chrome = c
        renderer.chromeChanged()
    }

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
        renderer.prewarmFloors()
    }

    func replayReveal() {
        // The opening plays into MEP, mains and big gear.
        wholeView = .mep
        density = .mains
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
        if case let .floor(i) = s { lastLevel = i }
        if case let .room(n) = s { lastRoom = n; lastLevel = building.room(n)?.level ?? lastLevel }
        if case .room = s {} else { sheetCollapsed = false }
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
            let n = drawnCount
            switch wholeView {
            case .architectural:
                return "Architectural · \(floors) floors · \(building.occupiedRoomCount) rooms · \(n.formatted()) objects"
            default:
                return "\(density == .full ? "Full MEP" : "Mains and big gear") · \(n.formatted()) of \(building.elements.count.formatted()) objects"
            }
        case let .floor(i):
            let rooms = building.rooms(on: i).filter { $0.name != "Corridor" }
            let n = drawnCount
            return rooms.isEmpty ? "\(n) objects" : "\(rooms.count) rooms · \(n.formatted()) objects"
        case let .room(num):
            guard let r = building.room(num) else { return "" }
            // Whole inches: the stat line is one quiet line, the card carries the exact sizes.
            let w = Units.feetInches(Double(r.max.x - r.min.x), denominator: 1), d = Units.feetInches(Double(r.max.y - r.min.y), denominator: 1)
            return "\(r.name) · \(w) × \(d) · \(building.elements(in: num).count) objects"
        }
    }
}
