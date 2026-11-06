import simd
import SwiftUI

/// Top-down plan of the model (model X right, model −Z up — the plan's north). In picking mode a tap picks
/// the nearest snap point (anchor, named points, room corners) within reach, else the tapped floor point.
struct PlanView: View {
    @ObservedObject var session: FieldSession
    let picking: Bool
    let onPick: (ReferencePoint) -> Void
    var picked: [ReferencePoint] = []
    @State private var snaps: [ReferencePoint] = []
    @State private var zoom: CGFloat = 1
    @State private var pan: CGSize = .zero
    @GestureState private var dragPan: CGSize = .zero
    @GestureState private var pinch: CGFloat = 1

    var body: some View {
        GeometryReader { geo in
            let fit = fitTransform(size: geo.size)
            Canvas { ctx, _ in
                draw(ctx: ctx, fit: fit)
            }
            .background(Color(.systemBackground))
            .contentShape(Rectangle())
            .gesture(
                SimultaneousGesture(
                    DragGesture(minimumDistance: 8).updating($dragPan) { v, s, _ in s = v.translation }
                        .onEnded { v in pan.width += v.translation.width; pan.height += v.translation.height },
                    MagnificationGesture().updating($pinch) { v, s, _ in s = v }.onEnded { v in zoom = max(0.2, min(zoom * v, 40)) }
                )
            )
            .onTapGesture { p in
                guard picking else { return }
                let m = fit.toModel(p)
                let best = snaps.min { simd_distance($0.position, m) < simd_distance($1.position, m) }
                if let b = best, CGFloat(simd_distance(b.position, m)) * fit.scale < 28 { onPick(b) } else { onPick(ReferencePoint(name: "Floor point", position: m)) }
            }
        }
        .navigationTitle(picking ? "Pick a model point" : "Plan")
        .task {
            let m = session.model
            let extra = session.sidecar.points
            snaps = await Task.detached { m?.snapPoints(extra: extra) ?? [ReferencePoint(name: "Anchor (origin)", position: .zero)] }.value
        }
    }

    struct Fit {
        var scale: CGFloat; var origin: CGPoint
        func toScreen(_ p: SIMD3<Float>) -> CGPoint { CGPoint(x: origin.x + CGFloat(p.x) * scale, y: origin.y + CGFloat(p.z) * scale) }
        func toModel(_ s: CGPoint) -> SIMD3<Float> { SIMD3(Float((s.x - origin.x) / scale), 0, Float((s.y - origin.y) / scale)) }
    }

    func fitTransform(size: CGSize) -> Fit {
        // The anchor (origin) and, after an update moved it, the old anchor are always in the frame.
        var lo = simd_min(session.model?.boundsMin ?? SIMD3(-1, 0, -1), .zero)
        var hi = simd_max(session.model?.boundsMax ?? SIMD3(1, 0, 1), .zero)
        if let old = session.previousAnchorPosition { lo = simd_min(lo, old); hi = simd_max(hi, old) }
        let w = CGFloat(max(hi.x - lo.x, 0.5)), d = CGFloat(max(hi.z - lo.z, 0.5))
        let base = min((size.width - 140) / w, (size.height - 64) / d) // room for the anchor labels
        let s = max(base, 1) * zoom * pinch
        let cx = CGFloat(lo.x + hi.x) / 2, cz = CGFloat(lo.z + hi.z) / 2
        return Fit(scale: s, origin: CGPoint(x: size.width / 2 - cx * s + pan.width + dragPan.width,
                                             y: size.height / 2 - cz * s + pan.height + dragPan.height))
    }

    func draw(ctx: GraphicsContext, fit: Fit) {
        guard let model = session.model else { return }
        var budget = 60_000
        for e in model.elements where session.isVisible(e) {
            let color = elementColor(model, e)
            if e.info.isContext {
                var path = Path()
                for part in e.parts where model.meshes.indices.contains(part.mesh) {
                    let m = e.transform * part.transform
                    for prim in model.meshes[part.mesh].primitives {
                        var i = 0
                        while i + 2 < prim.indices.count && budget > 0 {
                            let pts = (0..<3).compactMap { k -> CGPoint? in
                                let ix = Int(prim.indices[i + k])
                                guard prim.positions.indices.contains(ix) else { return nil }
                                let w = m * SIMD4(prim.positions[ix], 1)
                                return fit.toScreen(SIMD3(w.x, w.y, w.z))
                            }
                            if pts.count == 3 {
                                // Opposite faces of a solid project with opposite winding and would cancel
                                // under the non-zero fill rule: give every triangle the same winding and
                                // skip the edge-on (zero-area) ones.
                                let area = (pts[1].x - pts[0].x) * (pts[2].y - pts[0].y) - (pts[2].x - pts[0].x) * (pts[1].y - pts[0].y)
                                if abs(area) > 0.01 {
                                    path.addLines(area > 0 ? pts : [pts[0], pts[2], pts[1]])
                                    path.closeSubpath()
                                }
                            }
                            i += 3; budget -= 1
                        }
                    }
                }
                // Context is drawn in a fixed plan tone (exported wall colours are near-white, invisible on paper).
                let kind = e.info.kind?.lowercased() ?? ""
                if kind.hasPrefix("wall") { ctx.fill(path, with: .color(Color.primary.opacity(0.6))) }
                else if kind.hasPrefix("floor") || kind.hasPrefix("slab") { ctx.fill(path, with: .color(Color.secondary.opacity(0.10))) }
                else if !kind.hasPrefix("ceiling") && !kind.hasPrefix("roof") { ctx.fill(path, with: .color(Color.secondary.opacity(0.25))) }
            } else {
                let p = fit.toScreen(e.origin)
                let r: CGFloat = session.selected == e.index ? 7 : 4
                ctx.fill(Path(ellipseIn: CGRect(x: p.x - r, y: p.y - r, width: 2 * r, height: 2 * r)), with: .color(color))
                if fit.scale > 60, let tag = e.info.tag {
                    ctx.draw(Text(tag).font(.system(size: 9)).foregroundColor(.secondary), at: CGPoint(x: p.x, y: p.y - 10))
                }
            }
        }
        if picking {
            for s in snaps {
                let p = fit.toScreen(s.position)
                ctx.stroke(Path(ellipseIn: CGRect(x: p.x - 5, y: p.y - 5, width: 10, height: 10)), with: .color(.accentColor), lineWidth: 1.5)
            }
        }
        // The anchor marker.
        let a = fit.toScreen(.zero)
        var cross = Path()
        cross.move(to: CGPoint(x: a.x - 10, y: a.y)); cross.addLine(to: CGPoint(x: a.x + 10, y: a.y))
        cross.move(to: CGPoint(x: a.x, y: a.y - 10)); cross.addLine(to: CGPoint(x: a.x, y: a.y + 10))
        ctx.stroke(cross, with: .color(.red), lineWidth: 2)
        ctx.draw(Text("anchor").font(.caption2).foregroundColor(.red), at: CGPoint(x: a.x + 24, y: a.y + 10))
        // After an update moved the anchor: the one the room was last aligned to, in grey.
        if let old = session.previousAnchorPosition {
            let o = fit.toScreen(old)
            var c = Path()
            c.move(to: CGPoint(x: o.x - 10, y: o.y)); c.addLine(to: CGPoint(x: o.x + 10, y: o.y))
            c.move(to: CGPoint(x: o.x, y: o.y - 10)); c.addLine(to: CGPoint(x: o.x, y: o.y + 10))
            ctx.stroke(c, with: .color(.gray), lineWidth: 2)
            ctx.draw(Text("old anchor").font(.caption2).foregroundColor(.gray), at: CGPoint(x: o.x + 30, y: o.y + 10))
        }
        for (i, pk) in picked.enumerated() {
            let p = fit.toScreen(pk.position)
            ctx.fill(Path(ellipseIn: CGRect(x: p.x - 8, y: p.y - 8, width: 16, height: 16)), with: .color(.orange))
            ctx.draw(Text(i == 0 ? "A" : "B").font(.caption.bold()).foregroundColor(.white), at: p)
        }
    }

    func elementColor(_ model: FieldModel, _ e: FieldElement) -> Color {
        guard let part = e.parts.first, model.meshes.indices.contains(part.mesh), let c = model.meshes[part.mesh].primitives.first?.color else { return .gray }
        return Color(red: Double(c.x), green: Double(c.y), blue: Double(c.z))
    }
}
