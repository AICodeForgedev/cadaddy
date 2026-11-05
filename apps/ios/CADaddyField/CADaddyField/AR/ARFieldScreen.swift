import ARKit
import RealityKit
import SwiftUI

/// The AR screen: place the package in the room, inspect objects, mark them, save the room.
struct ARFieldScreen: View {
    @ObservedObject var session: FieldSession
    @Environment(\.dismiss) private var dismiss
    @State private var showLayers = false
    @State private var showNudge = false
    @State private var showPlanPicker = false
    @State private var showCard = false
    @State private var controller = ARController()

    var body: some View {
        ZStack {
            ARViewContainer(session: session, controller: controller)
                .ignoresSafeArea()
            VStack(spacing: 8) {
                statusBar
                Spacer()
                if showNudge, session.alignment != nil { NudgePanel(session: session, controller: controller) }
                bottomBar
            }
            .padding()
        }
        .inspector(isPresented: $showCard) {
            NavigationStack {
                if let i = session.selected {
                    ElementCardView(session: session, index: i, onMoved: {
                        showCard = false
                        session.mode = .markMoved(i)
                        session.status = "Tap the spot where it is actually installed."
                    })
                    .toolbar { Button("Done") { showCard = false } }
                }
            }
            .presentationDetents([.medium, .large])
        }
        .sheet(isPresented: $showLayers) { LayersView(session: session).presentationDetents([.medium, .large]) }
        .sheet(isPresented: $showPlanPicker) {
            NavigationStack {
                PlanView(session: session, picking: true, onPick: { p in
                    session.twoPointModel.append(p)
                    showPlanPicker = false
                    let i = session.twoPointModel.count - 1
                    session.mode = .twoPointReal(i)
                    session.status = "Now tap the real floor at point \(i == 0 ? "A" : "B") (\(p.name.lowercased()))."
                }, picked: session.twoPointModel)
                .toolbar { Button("Cancel") { showPlanPicker = false; controller.cancelTwoPoint() } }
            }
        }
        .onChange(of: session.selected) { _, v in showCard = v != nil && session.mode == .aligned }
        .onChange(of: session.mode) { _, m in if case .twoPointModel = m { showPlanPicker = true } }
        .onAppear { session.isAR = true }
        .onDisappear { session.isAR = false; controller.stop() }
        .statusBarHidden()
    }

    private var statusBar: some View {
        HStack(alignment: .top) {
            VStack(alignment: .leading, spacing: 4) {
                Text(session.status.isEmpty ? " " : session.status).font(.callout.weight(.semibold))
                if let r = session.residualText { Text(r).font(.caption) }
                if session.buildProgress < 1 { ProgressView(value: session.buildProgress).frame(maxWidth: 220) }
            }
            .padding(10)
            .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: 12))
            Spacer()
            Button { dismiss() } label: { Image(systemName: "xmark").font(.title3.bold()).padding(10) }
                .background(.ultraThinMaterial, in: Circle())
                .accessibilityLabel("Close")
        }
    }

    @ViewBuilder private var bottomBar: some View {
        HStack(spacing: 12) {
            switch session.mode {
            case .twoPointReview:
                Button("Redo") { controller.startTwoPoint() }.buttonStyle(.bordered)
                Button("Accept") { controller.acceptTwoPoint() }.buttonStyle(.borderedProminent)
            case .quickPlace where session.alignment != nil:
                Text("Drag to turn").font(.caption)
                Button("Done") { controller.finishQuick() }.buttonStyle(.borderedProminent)
            case .markMoved:
                Button("Cancel move") { session.mode = .aligned; session.pendingMove = nil; session.status = "" }.buttonStyle(.bordered)
            default:
                Menu {
                    Button { controller.startQuick() } label: { Label("Quick: tap the anchor corner", systemImage: "hand.tap") }
                    Button { controller.startTwoPoint() } label: { Label("Two-point", systemImage: "point.topleft.down.to.point.bottomright.curvepath") }
                } label: { Label("Align", systemImage: "scope") }
                .buttonStyle(.borderedProminent)
                Button { showNudge.toggle() } label: { Label("Nudge", systemImage: "arrow.up.and.down.and.arrow.left.and.right") }
                    .disabled(session.alignment == nil)
                Button { showLayers = true } label: { Label("Layers", systemImage: "square.3.layers.3d") }
                Button { controller.saveRoom() } label: { Label(session.roomSaved ? "Saved" : "Save room", systemImage: "square.and.arrow.down") }
                    .disabled(session.alignment == nil)
            }
        }
        .labelStyle(.titleAndIcon)
        .font(.callout)
        .padding(10)
        .background(.ultraThinMaterial, in: Capsule())
    }
}

struct NudgePanel: View {
    @ObservedObject var session: FieldSession
    let controller: ARController

    var body: some View {
        VStack(spacing: 6) {
            Text("Nudge 1/8\" · turn 0.1°").font(.caption.bold())
            HStack(spacing: 14) {
                pair("X", { controller.nudge(x: -1) }, { controller.nudge(x: 1) })
                pair("Y", { controller.nudge(y: -1) }, { controller.nudge(y: 1) })
                pair("Z", { controller.nudge(z: -1) }, { controller.nudge(z: 1) })
                pair("⟲", { controller.rotate(1) }, { controller.rotate(-1) })
            }
        }
        .padding(10)
        .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: 12))
    }

    private func pair(_ label: String, _ minus: @escaping () -> Void, _ plus: @escaping () -> Void) -> some View {
        VStack(spacing: 4) {
            Text(label).font(.caption2)
            HStack(spacing: 4) {
                RepeatButton(systemImage: "minus", action: minus)
                RepeatButton(systemImage: "plus", action: plus)
            }
        }
    }
}

/// A button that repeats while held (one step per tap, ~12 steps/s held).
struct RepeatButton: View {
    let systemImage: String
    let action: () -> Void
    @State private var timer: Timer?

    var body: some View {
        Image(systemName: systemImage)
            .frame(width: 34, height: 30)
            .background(Color.accentColor.opacity(0.2), in: RoundedRectangle(cornerRadius: 6))
            .onTapGesture { action() }
            .onLongPressGesture(minimumDuration: 0.3, pressing: { down in
                timer?.invalidate()
                timer = down ? Timer.scheduledTimer(withTimeInterval: 0.08, repeats: true) { _ in Task { @MainActor in action() } } : nil
            }, perform: {})
    }
}

struct LayersView: View {
    @ObservedObject var session: FieldSession

    var body: some View {
        NavigationStack {
            Form {
                Section("Walls, floors, ceilings") {
                    Picker("Shown", selection: $session.wallMode) { ForEach(WallMode.allCases) { Text($0.rawValue).tag($0) } }.pickerStyle(.segmented)
                    Toggle("Hide model behind real surfaces (LiDAR)", isOn: $session.occlusion)
                        .disabled(!ARWorldTrackingConfiguration.supportsSceneReconstruction(.mesh))
                    if !ARWorldTrackingConfiguration.supportsSceneReconstruction(.mesh) {
                        Text("This device has no LiDAR scanner, so real walls can't hide the model.").font(.caption).foregroundStyle(.secondary)
                    }
                    Toggle("Mounting-height labels", isOn: $session.showHeights)
                }
                Section("Trades") {
                    ForEach(session.model?.trades ?? [], id: \.self) { t in
                        Toggle(isOn: Binding(get: { !session.hiddenTrades.contains(t) }, set: { on in if on { session.hiddenTrades.remove(t) } else { session.hiddenTrades.insert(t) } })) {
                            HStack { Circle().fill(session.tradeColor(t)).frame(width: 10, height: 10); Text(t.capitalized) }
                        }
                    }
                }
                Section("Kinds") {
                    ForEach(session.model?.kinds ?? [], id: \.self) { k in
                        Toggle(k, isOn: Binding(get: { !session.hiddenKinds.contains(k) }, set: { on in if on { session.hiddenKinds.remove(k) } else { session.hiddenKinds.insert(k) } }))
                    }
                }
                Section("Performance") { LabeledContent("Frame rate", value: String(format: "%.0f fps", session.fps)) }
            }
            .navigationTitle("Layers")
        }
    }
}
