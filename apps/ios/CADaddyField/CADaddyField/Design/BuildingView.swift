// CADaddy Field — the building / floor / room screen: one 3D scene with quiet glass chrome over it.
// Title top left, floor rail on the right edge, everything you press in the thumb zone at the bottom.

import RealityKit
import SwiftUI

@available(iOS 18.0, *)
struct BuildingView: View {
    @StateObject private var exp = BuildingExperience()
    var onClose: () -> Void
    var onStandInRoom: () -> Void

    private var s: DesignStyle { exp.style }

    var body: some View {
        GeometryReader { geo in
            ZStack(alignment: .topLeading) {
                Backdrop(style: s).ignoresSafeArea()
                BuildingSceneContainer(exp: exp).ignoresSafeArea()
                if let line = exp.revealLine { RevealHandle(top: line.0, bottom: line.1, style: s).ignoresSafeArea() }
                // A soft scrim so the title stays legible over a busy model.
                LinearGradient(colors: [Color(uiColor: BuildingRendererColors.ui(s.bgTop)).opacity(s.highContrast ? 0 : 0.85), .clear], startPoint: .top, endPoint: .bottom)
                    .frame(height: 210).ignoresSafeArea().allowsHitTesting(false)
                header.padding(.horizontal, 20).padding(.top, 6)
                if exp.currentRoom == nil {
                FloorRail(exp: exp)
                    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .trailing)
                    .padding(.trailing, 10)
                    .padding(.bottom, bottomReserve(geo.size.height) + 70)
                    .padding(.top, 120)
                    .transition(.opacity)
                }
                VStack(spacing: 10) {
                    Spacer(minLength: 0)
                    HStack(alignment: .bottom) {
                        Spacer()
                        LayersButton(exp: exp)
                    }
                    .padding(.horizontal, 16)
                    bottom
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                if exp.showLayers {
                    Color.black.opacity(0.001).ignoresSafeArea().onTapGesture { withAnimation(.spring(response: 0.4, dampingFraction: 0.85)) { exp.showLayers = false } }
                    LayersPanel(exp: exp)
                        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .bottomTrailing)
                        .padding(.trailing, 16)
                        .padding(.bottom, bottomReserve(geo.size.height) + 76)
                        .transition(.scale(scale: 0.92, anchor: .bottomTrailing).combined(with: .opacity))
                }
            }
        }
        .preferredColorScheme(s.colorScheme)
        .statusBarHidden(false)
        .onDisappear { exp.detach() }
        .animation(.spring(response: 0.55, dampingFraction: 0.85), value: exp.scope)
        .animation(.spring(response: 0.45, dampingFraction: 0.85), value: exp.selected)
        .animation(.spring(response: 0.45, dampingFraction: 0.85), value: exp.focus)
    }

    private func bottomReserve(_ h: CGFloat) -> CGFloat {
        switch exp.scope {
        case .building: return exp.focus == nil ? 0 : 70
        case .floor: return exp.focus == nil ? 150 : 250
        case .room: return min(h * 0.44, 400)
        }
    }

    // MARK: header

    private var header: some View {
        HStack(alignment: .top) {
            VStack(alignment: .leading, spacing: 2) {
                if exp.scope != .building { breadcrumb.padding(.bottom, 2) }
                Text(exp.title)
                    .font(.system(size: exp.scope == .building ? 34 : 32, weight: .bold))
                    .tracking(-0.9)
                    .foregroundStyle(s.ink)
                    .contentTransition(.opacity)
                    .lineLimit(1)
                    .minimumScaleFactor(0.7)
                Text(exp.statLine)
                    .font(.system(size: 13, weight: .medium))
                    .foregroundStyle(s.inkSecondary)
                    .monospacedDigit()
                    .contentTransition(.opacity)
            }
            .padding(s.highContrast ? 10 : 0)
            .background { if s.highContrast { RoundedRectangle(cornerRadius: 14, style: .continuous).fill(s.isDark ? Color.black.opacity(0.85) : Color.white.opacity(0.92)) } }
            Spacer()
            Button(action: onClose) {
                Image(systemName: "xmark").font(.system(size: 15, weight: .semibold)).foregroundStyle(s.ink)
                    .frame(width: 40, height: 40)
                    .background(.ultraThinMaterial, in: Circle())
                    .overlay(Circle().strokeBorder(s.hairline, lineWidth: 0.5))
                    .frame(width: 56, height: 56)
                    .contentShape(Rectangle())
            }
            .accessibilityLabel("Close")
            .offset(x: 10, y: -6)
        }
    }

    private var breadcrumb: some View {
        HStack(spacing: 4) {
            crumb(exp.building.name) { exp.go(.building) }
            if let l = exp.currentLevel, let lv = exp.building.level(l) {
                Image(systemName: "chevron.right").font(.system(size: 9, weight: .bold)).foregroundStyle(s.inkSecondary)
                crumb(lv.name) { if exp.currentRoom != nil { exp.go(.floor(l)) } }
            }
            if let r = exp.currentRoom {
                Image(systemName: "chevron.right").font(.system(size: 9, weight: .bold)).foregroundStyle(s.inkSecondary)
                Text(r).font(.system(size: 13, weight: .semibold, design: .monospaced)).foregroundStyle(s.inkSecondary)
            }
        }
    }

    private func crumb(_ t: String, _ a: @escaping () -> Void) -> some View {
        Button(action: a) { Text(t).font(.system(size: 13, weight: .semibold)).foregroundStyle(s.inkSecondary).padding(.vertical, 8).contentShape(Rectangle()) }
            .buttonStyle(.plain)
    }

    // MARK: bottom

    @ViewBuilder private var bottom: some View {
        switch exp.scope {
        case .building:
            if exp.focus != nil { TraceStrip(exp: exp).padding(.horizontal, 12).padding(.bottom, 8).transition(.move(edge: .bottom).combined(with: .opacity)) } else {
                Text(exp.revealDone ? "Tap a floor, or drag the rail to lift floors apart" : "Drag across the building to reveal its systems")
                    .font(.system(size: 13, weight: .medium)).foregroundStyle(s.inkSecondary)
                    .padding(.horizontal, 20).padding(.bottom, 14).frame(maxWidth: .infinity, alignment: .leading)
            }
        case let .floor(i):
            FloorSheet(exp: exp, level: i).transition(.move(edge: .bottom).combined(with: .opacity))
        case let .room(n):
            RoomSheet(exp: exp, number: n, onStandInRoom: onStandInRoom).transition(.move(edge: .bottom).combined(with: .opacity))
        }
    }
}

// MARK: - Scene

@available(iOS 18.0, *)
struct BuildingSceneContainer: UIViewRepresentable {
    let exp: BuildingExperience
    func makeUIView(context: Context) -> ARView {
        let v = ARView(frame: .zero, cameraMode: .nonAR, automaticallyConfigureSession: false)
        DispatchQueue.main.async { exp.attach(v) }
        return v
    }
    func updateUIView(_ v: ARView, context: Context) {}
}

/// The studio backdrop behind the (transparent) 3D view: a vertical sweep with a soft lift toward the lower right.
struct Backdrop: View {
    let style: DesignStyle
    var body: some View {
        let top = Color(uiColor: BuildingRendererColors.ui(style.bgTop)), bottom = Color(uiColor: BuildingRendererColors.ui(style.bgBottom))
        ZStack {
            LinearGradient(colors: [top, bottom], startPoint: .top, endPoint: .bottom)
            if style.isDark && !style.highContrast {
                RadialGradient(colors: [Color(uiColor: UIColor(hex: 0x2A8077)).opacity(0.55), .clear], center: UnitPoint(x: 0.8, y: 1.0), startRadius: 10, endRadius: 620)
            } else if !style.isDark {
                RadialGradient(colors: [.white, .clear], center: UnitPoint(x: 0.5, y: 0.42), startRadius: 10, endRadius: 520)
            }
            // A faint vignette keeps the eye on the building.
            RadialGradient(colors: [.clear, .black.opacity(style.isDark ? 0.35 : 0.05)], center: .center, startRadius: 260, endRadius: 760)
        }
    }
}

enum BuildingRendererColors {
    static func ui(_ c: SIMD3<Float>) -> UIColor { UIColor(red: CGFloat(c.x), green: CGFloat(c.y), blue: CGFloat(c.z), alpha: 1) }
}

struct RevealHandle: View {
    let top: CGPoint, bottom: CGPoint
    let style: DesignStyle
    var body: some View {
        ZStack {
            Path { p in p.move(to: top); p.addLine(to: bottom) }
                .stroke(LinearGradient(colors: [.white.opacity(0), .white.opacity(0.9), .white.opacity(0)], startPoint: .top, endPoint: .bottom), lineWidth: 1.5)
                .shadow(color: .white.opacity(0.6), radius: 6)
            Circle().fill(.ultraThinMaterial).frame(width: 44, height: 44)
                .overlay(Image(systemName: "arrow.left.and.right").font(.system(size: 15, weight: .semibold)).foregroundStyle(.white))
                .overlay(Circle().strokeBorder(.white.opacity(0.5), lineWidth: 0.75))
                .position(x: (top.x + bottom.x) / 2, y: (top.y + bottom.y) / 2)
        }
        .allowsHitTesting(false)
    }
}

// MARK: - Glass

struct Glass: ViewModifier {
    let style: DesignStyle
    var radius: CGFloat = 24
    func body(content: Content) -> some View {
        content
            .background {
                if style.highContrast {
                    RoundedRectangle(cornerRadius: radius, style: .continuous).fill(style.isDark ? Color.black.opacity(0.92) : Color.white.opacity(0.96))
                } else {
                    RoundedRectangle(cornerRadius: radius, style: .continuous).fill(.ultraThinMaterial)
                }
            }
            .overlay(RoundedRectangle(cornerRadius: radius, style: .continuous).strokeBorder(style.hairline, lineWidth: style.highContrast ? 1 : 0.5))
            .shadow(color: .black.opacity(style.isDark ? 0.35 : 0.08), radius: 18, y: 8)
    }
}

extension View {
    func glass(_ s: DesignStyle, radius: CGFloat = 24) -> some View { modifier(Glass(style: s, radius: radius)) }
}

// MARK: - Floor rail

@available(iOS 18.0, *)
struct FloorRail: View {
    @ObservedObject var exp: BuildingExperience
    private let item: CGFloat = 52

    var body: some View {
        let levels = exp.building.railLevels
        let s = exp.style
        VStack(spacing: 0) {
            ForEach(Array(levels.enumerated()), id: \.offset) { _, lv in
                let active = isActive(lv.index)
                Text(lv.short)
                    .font(.system(size: lv.short.count > 1 ? 11 : 15, weight: active ? .bold : .semibold, design: lv.short.count > 1 ? .default : .rounded))
                    .foregroundStyle(active ? (s.isDark ? Color.black : Color.white) : s.ink.opacity(0.8))
                    .frame(width: 44, height: 40)
                    .background { if active { Capsule().fill(s.isDark ? Color.white : Color(uiColor: UIColor(hex: 0x0E1A24))).padding(.horizontal, 3) } }
                    .frame(width: 56, height: item)
                    .contentShape(Rectangle())
                    .accessibilityLabel(lv.name)
                    .accessibilityAddTraits(.isButton)
            }
        }
        .padding(.vertical, 6)
        .glass(s, radius: 28)
        .gesture(
            DragGesture(minimumDistance: 0)
                .onChanged { g in
                    if g.translation.height.magnitude > 6 || g.translation.width.magnitude > 6 {
                        // Item k (0 = Roof) is centred at 6 + item·(k + ½); map that span to 0…1.
                        exp.railChanged(Float((g.location.y - 6 - item / 2) / (item * CGFloat(max(levels.count - 1, 1)))))
                    }
                }
                .onEnded { g in
                    if g.translation.height.magnitude <= 6 && g.translation.width.magnitude <= 6 {
                        let k = Int((g.location.y - 6) / item)
                        if levels.indices.contains(k) { exp.railTap(levels[k].index) }
                    } else {
                        exp.railEnded()
                    }
                }
        )
        .animation(.spring(response: 0.35, dampingFraction: 0.85), value: exp.railDetent)
        .animation(.spring(response: 0.35, dampingFraction: 0.85), value: exp.scope)
    }

    private func isActive(_ i: Int) -> Bool {
        if case .building = exp.scope { return exp.railDetent == i && exp.liftFocus != nil }
        return exp.currentLevel == i
    }
}

// MARK: - Layers

@available(iOS 18.0, *)
struct LayersButton: View {
    @ObservedObject var exp: BuildingExperience
    var body: some View {
        Button {
            withAnimation(.spring(response: 0.4, dampingFraction: 0.85)) { exp.showLayers.toggle() }
        } label: {
            Image(systemName: exp.showLayers ? "square.3.layers.3d.top.filled" : "square.3.layers.3d")
                .font(.system(size: 21, weight: .medium))
                .foregroundStyle(exp.style.ink)
                .frame(width: 56, height: 56)
                .glass(exp.style, radius: 28)
        }
        .accessibilityLabel("Layers")
    }
}

/// The compact layer switches, shared by the building screens and AR.
struct LayerSwitches: View {
    @ObservedObject var store: LayerStore
    let style: DesignStyle
    let scope: DetailScope

    var body: some View {
        VStack(spacing: 0) {
            ForEach(FieldLayer.allCases) { l in
                let dim = l.isDimmed(at: scope)
                HStack(spacing: 12) {
                    badge(l)
                    VStack(alignment: .leading, spacing: 1) {
                        Text(l.title).font(.system(size: 16, weight: .semibold)).foregroundStyle(style.ink)
                        if dim { Text("Shown on floors and rooms").font(.system(size: 11)).foregroundStyle(style.inkSecondary) }
                    }
                    Spacer()
                    Toggle(l.title, isOn: Binding(get: { store.prefs.isOn(l) }, set: { on in withAnimation { store.prefs.set(l, on: on) } }))
                        .labelsHidden()
                        .tint(color(l))
                }
                .frame(minHeight: 56)
                .opacity(dim ? 0.45 : 1)
                .contentShape(Rectangle())
                if l != FieldLayer.allCases.last { Rectangle().fill(style.hairline).frame(height: 0.5) }
            }
        }
    }

    private func color(_ l: FieldLayer) -> Color {
        switch l {
        case .mechanical: return style.swiftUI(.mechanical)
        case .electrical: return style.swiftUI(.electrical)
        case .plumbing: return style.swiftUI(.plumbing)
        case .cabinets: return style.swiftUI(.cabinets)
        case .furniture: return style.swiftUI(.furniture)
        }
    }

    private func badge(_ l: FieldLayer) -> some View {
        let short = l.rawValue.count == 1 ? l.rawValue : (l == .cabinets ? "C" : "F")
        return Text(short)
            .font(.system(size: 14, weight: .bold, design: .rounded))
            .foregroundStyle(l.rawValue.count == 1 ? Color.white : style.ink)
            .frame(width: 30, height: 30)
            .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(l.rawValue.count == 1 ? color(l) : color(l).opacity(0.35)))
    }
}

@available(iOS 18.0, *)
struct LayersPanel: View {
    @ObservedObject var exp: BuildingExperience
    var body: some View {
        let s = exp.style
        VStack(alignment: .leading, spacing: 10) {
            Text("SHOW").font(.system(size: 11, weight: .semibold)).tracking(1.2).foregroundStyle(s.inkSecondary)
            LayerSwitches(store: LayerStore.shared, style: s, scope: exp.scope.detail)
            Rectangle().fill(s.hairline).frame(height: 0.5).padding(.vertical, 4)
            Text("LOOK").font(.system(size: 11, weight: .semibold)).tracking(1.2).foregroundStyle(s.inkSecondary)
            Picker("Background", selection: Binding(get: { exp.style.background }, set: { exp.setBackground($0) })) {
                ForEach(DesignBackground.allCases) { Text($0.rawValue).tag($0) }
            }
            .pickerStyle(.segmented)
            HStack {
                Text("High contrast").font(.system(size: 16, weight: .semibold)).foregroundStyle(s.ink)
                Spacer()
                Toggle("High contrast", isOn: Binding(get: { exp.style.highContrast }, set: { exp.setHighContrast($0) })).labelsHidden()
            }
            .frame(minHeight: 48)
            Button { withAnimation { exp.showLayers = false }; exp.replayReveal() } label: {
                Label("Replay the opening", systemImage: "play.fill").font(.system(size: 15, weight: .semibold)).foregroundStyle(s.ink)
                    .frame(maxWidth: .infinity, minHeight: 48, alignment: .leading).contentShape(Rectangle())
            }
            .buttonStyle(.plain)
        }
        .padding(18)
        .frame(width: 304)
        .glass(s, radius: 26)
    }
}

// MARK: - Path strip (follow the system)

@available(iOS 18.0, *)
struct TraceStrip: View {
    @ObservedObject var exp: BuildingExperience
    var compact = false

    var body: some View {
        let s = exp.style
        if let f = exp.focus, let tapped = exp.building.element(f.tapped) {
            VStack(alignment: .leading, spacing: 8) {
                HStack(spacing: 8) {
                    Circle().fill(s.swiftUI(tapped.system)).frame(width: 8, height: 8)
                    Text(f.showsDownstream ? "\(tapped.tag) feeds \(exp.building.downstream(of: f.tapped).count) objects" : "Follow the system")
                        .font(.system(size: 12, weight: .semibold)).foregroundStyle(s.inkSecondary)
                    Spacer()
                    Button { exp.clearFocus() } label: {
                        Image(systemName: "xmark").font(.system(size: 12, weight: .bold)).foregroundStyle(s.inkSecondary).frame(width: 44, height: 32).contentShape(Rectangle())
                    }
                    .accessibilityLabel("Stop following")
                }
                switch f.trace {
                case let .noKnownConnection(msg):
                    Label(msg, systemImage: "questionmark.circle").font(.system(size: 14, weight: .medium)).foregroundStyle(s.ink)
                        .padding(.vertical, 6)
                case let .chain(steps):
                    ScrollView(.horizontal, showsIndicators: false) {
                        HStack(spacing: 4) {
                            ForEach(Array(steps.enumerated()), id: \.offset) { i, st in
                                if i > 0 { Image(systemName: "chevron.right").font(.system(size: 10, weight: .bold)).foregroundStyle(s.inkSecondary) }
                                Button { exp.flyTo(st) } label: {
                                    VStack(alignment: .leading, spacing: 1) {
                                        Text(st.link.uppercased()).font(.system(size: 9, weight: .semibold)).tracking(0.8).foregroundStyle(s.inkSecondary)
                                        Text(st.tag).font(.system(size: 14, weight: .semibold, design: .monospaced)).foregroundStyle(s.ink)
                                    }
                                    .padding(.horizontal, 10)
                                    .frame(minWidth: 56, minHeight: 50)
                                    .background(RoundedRectangle(cornerRadius: 12, style: .continuous)
                                        .fill(exp.selected == st.id ? s.swiftUI(tapped.system).opacity(0.28) : s.hairline.opacity(0.5)))
                                    .contentShape(Rectangle())
                                }
                                .buttonStyle(.plain)
                                .accessibilityLabel("\(st.link) \(st.tag)")
                            }
                        }
                    }
                }
            }
            .padding(.horizontal, 14).padding(.vertical, 10)
            .glass(s, radius: 22)
        }
    }
}

// MARK: - Floor sheet

@available(iOS 18.0, *)
struct FloorSheet: View {
    @ObservedObject var exp: BuildingExperience
    let level: Int
    @FocusState private var searching: Bool

    var body: some View {
        let s = exp.style
        VStack(spacing: 8) {
            if exp.focus != nil { TraceStrip(exp: exp) }
            VStack(alignment: .leading, spacing: 12) {
                HStack(alignment: .firstTextBaseline) {
                    Text(exp.building.level(level)?.name ?? "").font(.system(size: 22, weight: .bold)).tracking(-0.5).foregroundStyle(s.ink)
                    Spacer()
                    Text("\(exp.building.rooms(on: level).filter { $0.name != "Corridor" }.count) rooms")
                        .font(.system(size: 14, weight: .medium)).foregroundStyle(s.inkSecondary).monospacedDigit()
                }
                HStack(spacing: 8) {
                    Image(systemName: "magnifyingglass").foregroundStyle(s.inkSecondary)
                    TextField("Room, panel or mark", text: $exp.search)
                        .font(.system(size: 16, weight: .medium))
                        .focused($searching)
                        .submitLabel(.go)
                        .onSubmit(search)
                        .autocorrectionDisabled()
                        .textInputAutocapitalization(.characters)
                    if !matches.isEmpty && !exp.search.isEmpty {
                        Text(matches.prefix(3).joined(separator: "  ")).font(.system(size: 13, weight: .semibold, design: .monospaced)).foregroundStyle(s.inkSecondary).lineLimit(1)
                    }
                }
                .padding(.horizontal, 14)
                .frame(height: 50)
                .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(s.hairline.opacity(0.55)))
            }
            .padding(16)
            .glass(s, radius: 26)
        }
        .padding(.horizontal, 10)
        .padding(.bottom, 6)
    }

    private var matches: [String] {
        let q = exp.search.trimmingCharacters(in: .whitespaces).uppercased()
        guard !q.isEmpty else { return [] }
        let rooms = exp.building.rooms(on: level).map(\.number).filter { $0.uppercased().hasPrefix(q) }
        let tags = exp.building.elements.filter { $0.level == level && $0.role == .equipment && $0.tag.uppercased().hasPrefix(q) }.map(\.tag)
        return rooms + tags
    }

    private func search() {
        let q = exp.search.trimmingCharacters(in: .whitespaces).uppercased()
        if let r = exp.building.rooms(on: level).first(where: { $0.number.uppercased() == q }) ?? exp.building.room(q) { exp.tapRoom(r.number); return }
        if let e = exp.building.elements.first(where: { $0.level == level && $0.role == .equipment && $0.tag.uppercased() == q }) { exp.tapElement(e.id); exp.flyTo(TraceStep(id: e.id, link: e.link, tag: e.tag, level: e.level)) }
    }
}

// MARK: - Room sheet and card

@available(iOS 18.0, *)
struct RoomSheet: View {
    @ObservedObject var exp: BuildingExperience
    let number: String
    let onStandInRoom: () -> Void

    private let order: [BuildingSystem] = [.electrical, .lowVoltage, .mechanical, .plumbing, .fire, .cabinets, .furniture]

    var body: some View {
        let s = exp.style
        VStack(spacing: 0) {
            Capsule().fill(s.hairline).frame(width: 36, height: 4).padding(.top, 8).padding(.bottom, 6)
            if let id = exp.selected, let e = exp.building.element(id) {
                ElementCard(exp: exp, element: e)
            } else {
                list
            }
            Button(action: onStandInRoom) {
                HStack(spacing: 10) {
                    Image(systemName: "arkit").font(.system(size: 18, weight: .semibold))
                    Text("Stand in this room").font(.system(size: 17, weight: .semibold))
                }
                .foregroundStyle(s.isDark ? Color.black : Color.white)
                .frame(maxWidth: .infinity, minHeight: 56)
                .background(RoundedRectangle(cornerRadius: 18, style: .continuous).fill(s.isDark ? Color.white : Color(uiColor: UIColor(hex: 0x0E1A24))))
            }
            .padding(.horizontal, 16).padding(.top, 8).padding(.bottom, 14)
        }
        .glass(s, radius: 28)
        .padding(.horizontal, 8)
        .padding(.bottom, 4)
        .frame(maxHeight: 420)
    }

    private var groups: [(BuildingSystem, [BuildingElement])] {
        let items = exp.building.elements(in: number)
        return order.compactMap { sys in
            let list = items.filter { $0.system == sys }
            return list.isEmpty ? nil : (sys, list)
        }
    }

    private var list: some View {
        let s = exp.style
        return VStack(alignment: .leading, spacing: 10) {
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 6) {
                    ForEach(groups, id: \.0) { sys, items in
                        let on = !exp.hiddenGroups.contains(sys)
                        Button {
                            withAnimation(.spring(response: 0.35, dampingFraction: 0.85)) { if on { exp.hiddenGroups.insert(sys) } else { exp.hiddenGroups.remove(sys) } }
                        } label: {
                            HStack(spacing: 6) {
                                Circle().fill(s.swiftUI(sys)).frame(width: 7, height: 7)
                                Text(sys.displayName).font(.system(size: 13, weight: .semibold))
                                Text("\(items.count)").font(.system(size: 12, weight: .medium, design: .monospaced)).foregroundStyle(s.inkSecondary)
                            }
                            .foregroundStyle(on ? s.ink : s.inkSecondary)
                            .padding(.horizontal, 12)
                            .frame(minHeight: 36)
                            .background(Capsule().fill(on ? s.hairline.opacity(0.7) : Color.clear))
                            .overlay(Capsule().strokeBorder(s.hairline, lineWidth: 0.75))
                            .frame(minHeight: 48)
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                    }
                }
                .padding(.horizontal, 16)
            }
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0, pinnedViews: []) {
                    ForEach(groups.filter { !exp.hiddenGroups.contains($0.0) }, id: \.0) { sys, items in
                        Text(sys.displayName.uppercased()).font(.system(size: 11, weight: .semibold)).tracking(1.1).foregroundStyle(s.inkSecondary)
                            .padding(.top, 10).padding(.bottom, 2)
                        ForEach(items) { e in row(e) }
                    }
                }
                .padding(.horizontal, 16)
            }
            .frame(maxHeight: 230)
        }
    }

    private func row(_ e: BuildingElement) -> some View {
        let s = exp.style
        return Button { exp.tapElement(e.id) } label: {
            HStack(spacing: 12) {
                RoundedRectangle(cornerRadius: 3).fill(s.swiftUI(e.system)).frame(width: 4, height: 26)
                Text(e.tag).font(.system(size: 15, weight: .semibold, design: .monospaced)).foregroundStyle(s.ink).frame(width: 64, alignment: .leading)
                Text(e.kind).font(.system(size: 15, weight: .medium)).foregroundStyle(s.ink)
                Spacer()
                if let h = e.mountingHeight {
                    Text(Units.feetInches(Double(h))).font(.system(size: 13, weight: .medium, design: .monospaced)).foregroundStyle(s.inkSecondary)
                }
            }
            .frame(minHeight: 50)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .overlay(alignment: .bottom) { Rectangle().fill(s.hairline).frame(height: 0.5) }
    }
}

@available(iOS 18.0, *)
struct ElementCard: View {
    @ObservedObject var exp: BuildingExperience
    let element: BuildingElement

    var body: some View {
        let s = exp.style
        let e = element
        VStack(alignment: .leading, spacing: 12) {
            HStack(alignment: .top) {
                Button { exp.clearFocus() } label: {
                    Image(systemName: "chevron.left").font(.system(size: 15, weight: .bold)).foregroundStyle(s.ink)
                        .frame(width: 44, height: 44).background(Circle().fill(s.hairline.opacity(0.6)))
                }
                .accessibilityLabel("Back to the room list")
                VStack(alignment: .leading, spacing: 2) {
                    HStack(spacing: 6) {
                        Image(systemName: e.system.symbol).font(.system(size: 11, weight: .bold)).foregroundStyle(s.swiftUI(e.system))
                        Text(e.system.displayName.uppercased()).font(.system(size: 11, weight: .semibold)).tracking(1).foregroundStyle(s.inkSecondary)
                    }
                    Text(e.kind).font(.system(size: 24, weight: .bold)).tracking(-0.5).foregroundStyle(s.ink)
                }
                Spacer()
                Text(e.tag).font(.system(size: 22, weight: .semibold, design: .monospaced)).foregroundStyle(s.swiftUI(e.system))
            }
            Grid(alignment: .leading, horizontalSpacing: 18, verticalSpacing: 8) {
                GridRow {
                    cell("Circuit", e.circuit ?? "—", mono: true)
                    cell("Height", e.mountingHeight.map { Units.feetInches(Double($0)) } ?? "—", mono: true)
                }
                GridRow {
                    cell("Panel", e.panel ?? "—", mono: true)
                    cell("Size", e.size ?? "—", mono: false)
                }
                GridRow {
                    cell("Catalog", e.catalog ?? "—", mono: true).gridCellColumns(2)
                }
            }
            TraceStrip(exp: exp)
        }
        .padding(.horizontal, 16)
        .padding(.bottom, 4)
    }

    private func cell(_ k: String, _ v: String, mono: Bool) -> some View {
        let s = exp.style
        return VStack(alignment: .leading, spacing: 1) {
            Text(k.uppercased()).font(.system(size: 10, weight: .semibold)).tracking(0.9).foregroundStyle(s.inkSecondary)
            Text(v).font(.system(size: 15, weight: .semibold, design: mono ? .monospaced : .default)).foregroundStyle(s.ink).lineLimit(1).minimumScaleFactor(0.75)
        }
    }
}
