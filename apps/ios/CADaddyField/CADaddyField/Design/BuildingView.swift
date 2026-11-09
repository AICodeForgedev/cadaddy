// CADaddy Field — the building / floor / room screen: one 3D scene with quiet glass chrome over it.
// Title top left, floor rail on the right edge, everything you press in the thumb zone at the bottom.

import RealityKit
import SwiftUI

@available(iOS 18.0, *)
struct BuildingView: View {
    @StateObject private var exp = BuildingExperience()
    var onClose: () -> Void
    var onStandInRoom: () -> Void

    /// Measured heights of the title block and the bottom stack at the current text size.
    @State private var headerHeight: CGFloat = 70
    @State private var bottomHeight: CGFloat = 44

    private var s: DesignStyle { exp.style }

    var body: some View {
        GeometryReader { geo in
            let inset = geo.safeAreaInsets
            let layout = ChromeLayout.make(.init(
                screen: CGSize(width: geo.size.width + inset.leading + inset.trailing, height: geo.size.height + inset.top + inset.bottom),
                safeTop: inset.top, safeBottom: inset.bottom, safeLeading: inset.leading, safeTrailing: inset.trailing,
                headerHeight: headerHeight, bottomHeight: bottomHeight, railItems: exp.currentRoom == nil ? exp.building.railLevels.count : 0))
            // Local (safe-area) coordinates of a full-screen rectangle.
            let local = { (r: CGRect) in r.offsetBy(dx: -inset.leading, dy: -inset.top) }
            ZStack(alignment: .topLeading) {
                Backdrop(style: s).ignoresSafeArea()
                BuildingSceneContainer(exp: exp).ignoresSafeArea()
                if let line = exp.revealLine { RevealHandle(top: line.0, bottom: line.1, style: s).ignoresSafeArea() }
                scrim(height: layout.header.maxY).ignoresSafeArea().allowsHitTesting(false)

                header
                    .frame(width: local(layout.header).width, alignment: .leading)
                    .fixedSize(horizontal: false, vertical: true)
                    .background(GeometryReader { g in Color.clear.preference(key: HeaderHeightKey.self, value: g.size.height) })
                    .offset(x: local(layout.header).minX, y: local(layout.header).minY)

                closeButton.offset(x: local(layout.close).minX, y: local(layout.close).minY)

                if let rail = layout.rail {
                    Group {
                        switch layout.railMode {
                        case let .rail(pitch): FloorRail(exp: exp, pitch: pitch)
                        case .menu: FloorMenu(exp: exp)
                        case .hidden: EmptyView()
                        }
                    }
                    .offset(x: local(rail).minX, y: local(rail).minY)
                    .transition(.opacity)
                }

                LayersButton(exp: exp).offset(x: local(layout.layers).minX, y: local(layout.layers).minY)

                VStack(spacing: 0) {
                    Spacer(minLength: 0)
                    bottom
                        .background(GeometryReader { g in Color.clear.preference(key: BottomHeightKey.self, value: g.size.height) })
                }
                .frame(width: geo.size.width, height: geo.size.height)

                if exp.showLayers {
                    Color.black.opacity(0.001).ignoresSafeArea().onTapGesture { withAnimation(.spring(response: 0.4, dampingFraction: 0.85)) { exp.showLayers = false } }
                    LayersPanel(exp: exp, maxHeight: max(local(layout.layers).minY - local(layout.header).minY - 8, 160))
                        .background(GeometryReader { g in Color.clear.onAppear { exp.overlay = g.frame(in: .global) } })
                        .frame(width: geo.size.width, height: geo.size.height, alignment: .bottomTrailing)
                        .padding(.trailing, geo.size.width - local(layout.layers).maxX)
                        .padding(.bottom, geo.size.height - local(layout.layers).minY + ChromeLayout.gap)
                        .frame(width: geo.size.width, height: geo.size.height)
                        .transition(.scale(scale: 0.92, anchor: .bottomTrailing).combined(with: .opacity))
                }
            }
            .onPreferenceChange(HeaderHeightKey.self) { h in if abs(h - headerHeight) > 0.5 { headerHeight = h } }
            .onPreferenceChange(BottomHeightKey.self) { h in if abs(h - bottomHeight) > 0.5 { bottomHeight = h } }
            .onAppear { exp.setChrome(layout) }
            .onChange(of: layout) { _, l in exp.setChrome(l) }
        }
        .dynamicTypeSize(...FieldType.largest)
        .preferredColorScheme(s.colorScheme)
        .statusBarHidden(false)
        .onDisappear { exp.detach() }
        .animation(.spring(response: 0.55, dampingFraction: 0.85), value: exp.scope)
        .animation(.spring(response: 0.45, dampingFraction: 0.85), value: exp.selected)
        .animation(.spring(response: 0.45, dampingFraction: 0.85), value: exp.focus)
        .animation(.spring(response: 0.45, dampingFraction: 0.85), value: exp.sheetCollapsed)
    }

    /// Text over the scene stays readable in every look: the backdrop's own top colour, solid behind the title
    /// block and fading out just below it, sized to the measured header (however many lines it has).
    private func scrim(height: CGFloat) -> some View {
        let top = Color(uiColor: BuildingRendererColors.ui(s.bgTop))
        let h = max(height, 1) + 28
        let solid = max(height - 6, 0) / h
        return LinearGradient(stops: [
            .init(color: top.opacity(s.highContrast ? 0.97 : 0.93), location: 0),
            .init(color: top.opacity(s.highContrast ? 0.95 : 0.86), location: solid),
            .init(color: top.opacity(0), location: 1),
        ], startPoint: .top, endPoint: .bottom)
        .frame(height: h)
    }

    // MARK: header

    private var header: some View {
        VStack(alignment: .leading, spacing: 2) {
            if exp.scope != .building { breadcrumb.padding(.bottom, 2) }
            Text(exp.title)
                .fieldFont(exp.scope == .building ? 34 : 32, .bold)
                .tracking(-0.9)
                .foregroundStyle(s.ink)
                .contentTransition(.opacity)
                .lineLimit(1)
                .minimumScaleFactor(0.7)
            // One quiet line: it never wraps over the scene; it shrinks a little, then truncates in the middle.
            Text(exp.statLine)
                .fieldFont(13, .medium)
                .foregroundStyle(s.inkSecondary)
                .monospacedDigit()
                .lineLimit(1)
                .minimumScaleFactor(0.85)
                .truncationMode(.middle)
                .contentTransition(.opacity)
        }
        .padding(s.highContrast ? 10 : 0)
        .background { if s.highContrast { RoundedRectangle(cornerRadius: 14, style: .continuous).fill(s.isDark ? Color.black.opacity(0.85) : Color.white.opacity(0.92)) } }
    }

    private var closeButton: some View {
        Button(action: onClose) {
            Image(systemName: "xmark").fieldFont(15, .semibold).foregroundStyle(s.ink)
                .frame(width: 40, height: 40)
                .background(.ultraThinMaterial, in: Circle())
                .overlay(Circle().strokeBorder(s.hairline, lineWidth: 0.5))
                .frame(width: ChromeLayout.target, height: ChromeLayout.target)
                .contentShape(Rectangle())
        }
        .accessibilityLabel("Close")
    }

    private var breadcrumb: some View {
        HStack(spacing: 4) {
            crumb(exp.building.name) { exp.go(.building) }
            if let l = exp.currentLevel, let lv = exp.building.level(l) {
                Image(systemName: "chevron.right").fieldFont(9, .bold).foregroundStyle(s.inkSecondary)
                crumb(lv.name) { if exp.currentRoom != nil { exp.go(.floor(l)) } }
            }
            if let r = exp.currentRoom {
                Image(systemName: "chevron.right").fieldFont(9, .bold).foregroundStyle(s.inkSecondary)
                Text(r).fieldFont(13, .semibold, .monospaced).foregroundStyle(s.inkSecondary).lineLimit(1)
            }
        }
        .lineLimit(1)
    }

    private func crumb(_ t: String, _ a: @escaping () -> Void) -> some View {
        Button(action: a) { Text(t).fieldFont(13, .semibold).foregroundStyle(s.inkSecondary).lineLimit(1).frame(minHeight: 56).padding(.horizontal, 4).contentShape(Rectangle()) }
            .buttonStyle(.plain)
    }

    // MARK: bottom

    @ViewBuilder private var bottom: some View {
        switch exp.scope {
        case .building:
            if exp.focus != nil { TraceStrip(exp: exp).padding(.horizontal, 12).padding(.bottom, 8).transition(.move(edge: .bottom).combined(with: .opacity)) } else {
                Text(exp.revealDone ? "Tap a floor, or drag the rail to lift floors apart" : "Drag across the building to reveal its systems")
                    .fieldFont(13, .medium).foregroundStyle(s.inkSecondary)
                    .lineLimit(2)
                    .padding(.horizontal, 20).padding(.top, 6).padding(.bottom, 14).frame(maxWidth: .infinity, alignment: .leading)
            }
        case let .floor(i):
            FloorSheet(exp: exp, level: i).transition(.move(edge: .bottom).combined(with: .opacity))
        case let .room(n):
            RoomSheet(exp: exp, number: n, onStandInRoom: onStandInRoom).transition(.move(edge: .bottom).combined(with: .opacity))
        }
    }
}

private struct HeaderHeightKey: PreferenceKey {
    static let defaultValue: CGFloat = 0
    static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) { value = max(value, nextValue()) }
}

private struct BottomHeightKey: PreferenceKey {
    static let defaultValue: CGFloat = 0
    static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) { value = max(value, nextValue()) }
}

/// Chrome text follows Dynamic Type up to `largest` (×1.3); the accessibility sizes stop there on these
/// screens so the scene keeps its room. The layout rule measures the result, so nothing overlaps at any size.
enum FieldType {
    static let largest = DynamicTypeSize.xxxLarge
    static func scale(_ d: DynamicTypeSize) -> CGFloat {
        switch d {
        case .xSmall: return 0.85
        case .small: return 0.9
        case .medium: return 0.95
        case .large: return 1
        case .xLarge: return 1.1
        case .xxLarge: return 1.2
        default: return 1.3
        }
    }
}

struct FieldFont: ViewModifier {
    @Environment(\.dynamicTypeSize) private var size
    let points: CGFloat
    let weight: Font.Weight
    let design: Font.Design
    func body(content: Content) -> some View { content.font(.system(size: points * FieldType.scale(size), weight: weight, design: design)) }
}

extension View {
    func fieldFont(_ points: CGFloat, _ weight: Font.Weight, _ design: Font.Design = .default) -> some View {
        modifier(FieldFont(points: points, weight: weight, design: design))
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
                .overlay(Image(systemName: "arrow.left.and.right").fieldFont(15, .semibold).foregroundStyle(.white))
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
    /// Points per floor, from ChromeLayout (56, or down to 44 on a short column).
    var pitch: CGFloat = ChromeLayout.target
    private var item: CGFloat { pitch }
    private var pad: CGFloat { ChromeLayout.railPadding }

    var body: some View {
        let levels = exp.building.railLevels
        let s = exp.style
        VStack(spacing: 0) {
            ForEach(Array(levels.enumerated()), id: \.offset) { _, lv in
                let active = isActive(lv.index)
                Text(lv.short)
                    .font(.system(size: lv.short.count > 1 ? 11 : 15, weight: active ? .bold : .semibold, design: lv.short.count > 1 ? .default : .rounded))
                    .foregroundStyle(active ? (s.isDark ? Color.black : Color.white) : s.ink.opacity(0.8))
                    .frame(width: 44, height: min(40, item - 4))
                    .background { if active { Capsule().fill(s.isDark ? Color.white : Color(uiColor: UIColor(hex: 0x0E1A24))).padding(.horizontal, 3) } }
                    .frame(width: ChromeLayout.target, height: item)
                    .contentShape(Rectangle())
                    .accessibilityLabel(lv.name)
                    .accessibilityAddTraits(.isButton)
            }
        }
        .padding(.vertical, pad)
        .glass(s, radius: 28)
        .gesture(
            DragGesture(minimumDistance: 0)
                .onChanged { g in
                    if g.translation.height.magnitude > 6 || g.translation.width.magnitude > 6 {
                        // Item k (0 = Roof) is centred at pad + item·(k + ½); map that span to 0…1.
                        exp.railChanged(Float((g.location.y - pad - item / 2) / (item * CGFloat(max(levels.count - 1, 1)))))
                    }
                }
                .onEnded { g in
                    if g.translation.height.magnitude <= 6 && g.translation.width.magnitude <= 6 {
                        let k = Int(((g.location.y - pad) / item).rounded(.down))
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

/// When the column is too short for a usable rail: one button with the floors as a menu.
@available(iOS 18.0, *)
struct FloorMenu: View {
    @ObservedObject var exp: BuildingExperience
    var body: some View {
        Menu {
            ForEach(exp.building.railLevels, id: \.index) { lv in
                Button(lv.name) { exp.railTap(lv.index) }
            }
        } label: {
            Image(systemName: "square.stack.3d.up")
                .fieldFont(20, .medium)
                .foregroundStyle(exp.style.ink)
                .frame(width: ChromeLayout.target, height: ChromeLayout.target)
                .glass(exp.style, radius: 28)
        }
        .accessibilityLabel("Floors")
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
                .fieldFont(21, .medium)
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
                        Text(l.title).fieldFont(16, .semibold).foregroundStyle(style.ink)
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
            .fieldFont(14, .bold, .rounded)
            .foregroundStyle(l.rawValue.count == 1 ? Color.white : style.ink)
            .frame(width: 30, height: 30)
            .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(l.rawValue.count == 1 ? color(l) : color(l).opacity(0.35)))
    }
}

@available(iOS 18.0, *)
struct LayersPanel: View {
    @ObservedObject var exp: BuildingExperience
    /// The height above the layers button; the panel scrolls when it is taller (big text, room sheet up).
    var maxHeight: CGFloat = .infinity
    var body: some View {
        ScrollView(.vertical, showsIndicators: false) { content }
            .scrollBounceBehavior(.basedOnSize)
            .frame(width: 304)
            .frame(maxHeight: maxHeight)
            .fixedSize(horizontal: false, vertical: true)
            .glass(exp.style, radius: 26)
    }

    private var content: some View {
        let s = exp.style
        return VStack(alignment: .leading, spacing: 10) {
            Text("SHOW").fieldFont(11, .semibold).tracking(1.2).foregroundStyle(s.inkSecondary)
            LayerSwitches(store: LayerStore.shared, style: s, scope: exp.scope.detail)
            Rectangle().fill(s.hairline).frame(height: 0.5).padding(.vertical, 4)
            Text("LOOK").fieldFont(11, .semibold).tracking(1.2).foregroundStyle(s.inkSecondary)
            Picker("Background", selection: Binding(get: { exp.style.background }, set: { exp.setBackground($0) })) {
                ForEach(DesignBackground.allCases) { Text($0.rawValue).tag($0) }
            }
            .pickerStyle(.segmented)
            HStack {
                Text("High contrast").fieldFont(16, .semibold).foregroundStyle(s.ink)
                Spacer()
                Toggle("High contrast", isOn: Binding(get: { exp.style.highContrast }, set: { exp.setHighContrast($0) })).labelsHidden()
            }
            .frame(minHeight: 48)
            Button { withAnimation { exp.showLayers = false }; exp.replayReveal() } label: {
                Label("Replay the opening", systemImage: "play.fill").fieldFont(15, .semibold).foregroundStyle(s.ink)
                    .frame(maxWidth: .infinity, minHeight: 48, alignment: .leading).contentShape(Rectangle())
            }
            .buttonStyle(.plain)
        }
        .padding(18)
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
                        .fieldFont(12, .semibold).foregroundStyle(s.inkSecondary)
                    Spacer()
                    Button { exp.clearFocus() } label: {
                        Image(systemName: "xmark").fieldFont(12, .bold).foregroundStyle(s.inkSecondary).frame(width: 56, height: 40).contentShape(Rectangle())
                    }
                    .accessibilityLabel("Stop following")
                }
                switch f.trace {
                case let .noKnownConnection(msg):
                    Label(msg, systemImage: "questionmark.circle").fieldFont(14, .medium).foregroundStyle(s.ink)
                        .padding(.vertical, 6)
                case let .chain(steps):
                    ScrollView(.horizontal, showsIndicators: false) {
                        HStack(spacing: 4) {
                            ForEach(Array(steps.enumerated()), id: \.offset) { i, st in
                                if i > 0 { Image(systemName: "chevron.right").fieldFont(10, .bold).foregroundStyle(s.inkSecondary) }
                                Button { exp.flyTo(st) } label: {
                                    VStack(alignment: .leading, spacing: 1) {
                                        Text(st.link.uppercased()).fieldFont(9, .semibold).tracking(0.8).foregroundStyle(s.inkSecondary)
                                        Text(st.tag).fieldFont(14, .semibold, .monospaced).foregroundStyle(s.ink)
                                    }
                                    .padding(.horizontal, 10)
                                    .frame(minWidth: 56, minHeight: 56)
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
                    Text(exp.building.level(level)?.name ?? "").fieldFont(22, .bold).tracking(-0.5).foregroundStyle(s.ink)
                    Spacer()
                    Text("\(exp.building.rooms(on: level).filter { $0.name != "Corridor" }.count) rooms")
                        .fieldFont(14, .medium).foregroundStyle(s.inkSecondary).monospacedDigit()
                }
                HStack(spacing: 8) {
                    Image(systemName: "magnifyingglass").foregroundStyle(s.inkSecondary)
                    TextField("Room, panel or mark", text: $exp.search)
                        .fieldFont(16, .medium)
                        .focused($searching)
                        .submitLabel(.go)
                        .onSubmit(search)
                        .autocorrectionDisabled()
                        .textInputAutocapitalization(.characters)
                    if !matches.isEmpty && !exp.search.isEmpty {
                        Text(matches.prefix(3).joined(separator: "  ")).fieldFont(13, .semibold, .monospaced).foregroundStyle(s.inkSecondary).lineLimit(1)
                    }
                }
                .padding(.horizontal, 14)
                .frame(height: 56)
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
            // The handle: tap or drag to pull the sheet down to its button, and back up.
            Capsule().fill(s.hairline).frame(width: 36, height: 4)
                .frame(maxWidth: .infinity, minHeight: 28)
                .contentShape(Rectangle())
                .onTapGesture { exp.sheetCollapsed.toggle() }
                .gesture(DragGesture(minimumDistance: 8).onEnded { g in
                    if g.translation.height > 24 { exp.sheetCollapsed = true } else if g.translation.height < -24 { exp.sheetCollapsed = false }
                })
                .accessibilityLabel(exp.sheetCollapsed ? "Show the room list" : "Hide the room list")
                .accessibilityAddTraits(.isButton)
            if !exp.sheetCollapsed {
                if let id = exp.selected, let e = exp.building.element(id) {
                    ElementCard(exp: exp, element: e)
                } else {
                    list
                }
            }
            Button(action: onStandInRoom) {
                HStack(spacing: 10) {
                    Image(systemName: "arkit").fieldFont(18, .semibold)
                    Text("Stand in this room").fieldFont(17, .semibold)
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
        .frame(maxHeight: min(420, UIScreen.main.bounds.height * 0.5))
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
                                Text(sys.displayName).fieldFont(13, .semibold)
                                Text("\(items.count)").fieldFont(12, .medium, .monospaced).foregroundStyle(s.inkSecondary)
                            }
                            .foregroundStyle(on ? s.ink : s.inkSecondary)
                            .padding(.horizontal, 12)
                            .frame(minHeight: 40)
                            .background(Capsule().fill(on ? s.hairline.opacity(0.7) : Color.clear))
                            .overlay(Capsule().strokeBorder(s.hairline, lineWidth: 0.75))
                            .frame(minHeight: 56)
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
                        Text(sys.displayName.uppercased()).fieldFont(11, .semibold).tracking(1.1).foregroundStyle(s.inkSecondary)
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
                Text(e.tag).fieldFont(15, .semibold, .monospaced).foregroundStyle(s.ink).frame(width: 64, alignment: .leading)
                Text(e.kind).fieldFont(15, .medium).foregroundStyle(s.ink)
                Spacer()
                if let h = e.mountingHeight {
                    Text(Units.feetInches(Double(h))).fieldFont(13, .medium, .monospaced).foregroundStyle(s.inkSecondary)
                }
            }
            .frame(minHeight: 56)
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
                    Image(systemName: "chevron.left").fieldFont(15, .bold).foregroundStyle(s.ink)
                        .frame(width: 44, height: 44).background(Circle().fill(s.hairline.opacity(0.6)))
                        .frame(width: 56, height: 56).contentShape(Rectangle())
                }
                .accessibilityLabel("Back to the room list")
                VStack(alignment: .leading, spacing: 2) {
                    HStack(spacing: 6) {
                        Image(systemName: e.system.symbol).fieldFont(11, .bold).foregroundStyle(s.swiftUI(e.system))
                        Text(e.system.displayName.uppercased()).fieldFont(11, .semibold).tracking(1).foregroundStyle(s.inkSecondary)
                    }
                    Text(e.kind).fieldFont(24, .bold).tracking(-0.5).foregroundStyle(s.ink)
                }
                Spacer()
                Text(e.tag).fieldFont(22, .semibold, .monospaced).foregroundStyle(s.swiftUI(e.system))
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
            Text(k.uppercased()).fieldFont(10, .semibold).tracking(0.9).foregroundStyle(s.inkSecondary)
            Text(v).font(.system(size: 15, weight: .semibold, design: mono ? .monospaced : .default)).foregroundStyle(s.ink).lineLimit(1).minimumScaleFactor(0.75)
        }
    }
}
