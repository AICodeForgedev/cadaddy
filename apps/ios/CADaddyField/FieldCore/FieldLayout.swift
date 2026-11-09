// CADaddy Field — screen layout rules for the building / floor / room experience. Pure maths on bounds and
// rectangles (Foundation, CoreGraphics, simd), so the tests can check every rule on any building proportions and
// phone size:
//   - ChromeLayout: where the header, close button, floor rail, layers button and bottom sheet go, and the free
//     area left for the 3D scene. Nothing touches, in any scope, with the sheet up or down, at any measured size.
//   - SceneFraming: the orbit camera distance and target that fit a set of 3D bounds inside that free area,
//     centred, for any proportions (long and thin, square, tall).
//   - LabelLayout: labels laid out in screen space: kept on screen, off the chrome, never overlapping; moved
//     aside with a short leader or dropped by priority.
//   - DepthFog: how much a distant object fades toward the backdrop (an opaque colour mix, no blending).

import CoreGraphics
import Foundation
import simd

// MARK: - Chrome layout

/// Where every piece of chrome goes on one screen. All rectangles are in points, in the full-screen space
/// (origin top left, safe areas included).
public struct ChromeLayout: Equatable, Sendable {
    public enum RailMode: Equatable, Sendable {
        /// The rail with every floor at `pitch` points per item.
        case rail(pitch: CGFloat)
        /// Not enough height for a usable rail: a single "Floors" button takes its place.
        case menu
        /// The scope has no rail (a room).
        case hidden
    }

    public var header: CGRect
    public var close: CGRect
    public var rail: CGRect?
    public var railMode: RailMode
    public var layers: CGRect
    public var bottom: CGRect
    /// The area the 3D scene's subject is framed into: clear of every piece of chrome.
    public var free: CGRect

    /// The rule's constants (points).
    public static let target: CGFloat = 56          // the glove touch target
    public static let minPitch: CGFloat = 44        // a rail item is never shorter than this
    public static let gap: CGFloat = 8              // between any two pieces of chrome
    public static let edge: CGFloat = 10            // from the screen edge (inside the safe area)
    public static let railPadding: CGFloat = 6      // the rail's glass above the first and below the last item
    public static let minScene: CGFloat = 140       // the scene never gets less height than this (or 20 % of a tiny screen)

    public struct Input: Sendable {
        public var screen: CGSize
        public var safeTop: CGFloat
        public var safeBottom: CGFloat
        public var safeLeading: CGFloat
        public var safeTrailing: CGFloat
        /// Measured height of the title block (breadcrumb, title, stat line) at the current text size.
        public var headerHeight: CGFloat
        /// Measured height of the bottom stack (hint, path strip, sheet), from its top to the safe-area bottom.
        public var bottomHeight: CGFloat
        /// Floors on the rail; 0 when the scope shows no rail.
        public var railItems: Int
        public init(screen: CGSize, safeTop: CGFloat, safeBottom: CGFloat, safeLeading: CGFloat = 0, safeTrailing: CGFloat = 0,
                    headerHeight: CGFloat, bottomHeight: CGFloat, railItems: Int) {
            self.screen = screen; self.safeTop = safeTop; self.safeBottom = safeBottom
            self.safeLeading = safeLeading; self.safeTrailing = safeTrailing
            self.headerHeight = headerHeight; self.bottomHeight = bottomHeight; self.railItems = railItems
        }
    }

    /// The rule:
    /// - The header runs across the top inside the safe area; the close button is its top-right corner and the
    ///   title block is the width left of it.
    /// - The bottom stack sits on the safe-area bottom. When it would leave less than the touch target above it
    ///   for the layers button, it is capped (its content scrolls).
    /// - The layers button sits in the right column directly above the bottom stack.
    /// - The rail takes the right column between the close button and the layers button, centred in it. Items are
    ///   56 pt; when the column is short they shrink down to 44 pt; below that the rail becomes one "Floors"
    ///   button so nothing ever overlaps.
    /// - The scene's free area is what is left: below the header, above the bottom stack (and the layers button),
    ///   left of the rail column when there is a rail.
    public static func make(_ i: Input) -> ChromeLayout {
        let w = max(i.screen.width, 1), h = max(i.screen.height, 1)
        let left = i.safeLeading + edge, right = w - i.safeTrailing - edge
        let top = i.safeTop
        let floorY = h - i.safeBottom
        let t = target, g = gap

        let close = CGRect(x: right - t, y: top, width: t, height: t)
        let headerH = max(i.headerHeight, t)
        // The vertical budget between the header and the bottom stack: the scene keeps at least `sceneMin`, and the
        // right column keeps the layers button (plus at least the one-button rail when there is a rail). With a
        // rail the scene sits beside the column; without one it sits above the layers button.
        let sceneMin = min(minScene, max((floorY - top) * 0.2, 0))
        let middle = i.railItems > 0 ? max(sceneMin, 2 * t + g) + 2 * g : sceneMin + t + 3 * g
        // The header and then the bottom stack give way (their content truncates or scrolls) before the middle does.
        let maxHeader = max(t, floorY - top - middle)
        let header = CGRect(x: left, y: top, width: max(close.minX - g - left, 0), height: min(headerH, maxHeader))
        let headerBottom = max(header.maxY, close.maxY)

        let maxBottom = max(floorY - headerBottom - middle, 0)
        let bottomH = min(max(i.bottomHeight, 0), maxBottom)
        let bottom = CGRect(x: i.safeLeading, y: floorY - bottomH, width: w - i.safeLeading - i.safeTrailing, height: bottomH)
        let layers = CGRect(x: right - t, y: bottom.minY - g - t, width: t, height: t)

        var rail: CGRect?
        var mode = RailMode.hidden
        if i.railItems > 0 {
            // The column under the close button: the title block never reaches into it (its width stops left of
            // the close button), so the rail starts right below the close button, not below the whole header.
            let spanTop = close.maxY + g, spanBottom = layers.minY - g
            let span = max(spanBottom - spanTop, 0)
            let n = CGFloat(i.railItems)
            let natural = n * t + 2 * railPadding
            if span >= natural {
                mode = .rail(pitch: t)
                rail = CGRect(x: right - t, y: spanTop + (span - natural) / 2, width: t, height: natural)
            } else {
                let pitch = floor((span - 2 * railPadding) / n)
                if pitch >= minPitch {
                    mode = .rail(pitch: pitch)
                    let hh = n * pitch + 2 * railPadding
                    rail = CGRect(x: right - t, y: spanTop + (span - hh) / 2, width: t, height: hh)
                } else {
                    mode = .menu
                    rail = CGRect(x: right - t, y: spanTop, width: t, height: t)
                }
            }
        }

        // Free area for the scene. With a rail: left of the right column, down to the bottom stack. Without one:
        // whichever is larger of the full width above the layers button, or left of its column down to the stack.
        let freeTop = headerBottom + g
        let beside = CGRect(x: left, y: freeTop, width: max(right - t - g - left, 1), height: max(bottom.minY - g - freeTop, 1))
        let above = CGRect(x: left, y: freeTop, width: max(right - left, 1), height: max(layers.minY - g - freeTop, 1))
        let free = rail != nil || beside.width * beside.height >= above.width * above.height ? beside : above
        return ChromeLayout(header: header, close: close, rail: rail, railMode: mode, layers: layers, bottom: bottom, free: free)
    }

    /// Every pair of chrome rectangles that touch (closer than `gap` minus a hair). Empty when the rule holds.
    public var collisions: [(String, String)] {
        var parts: [(String, CGRect)] = [("header", header), ("close", close), ("layers", layers), ("bottom", bottom)]
        if let rail { parts.append(("rail", rail)) }
        var out: [(String, String)] = []
        for a in 0..<parts.count {
            for b in (a + 1)..<parts.count {
                let ra = parts[a].1, rb = parts[b].1
                guard ra.width > 0, ra.height > 0, rb.width > 0, rb.height > 0 else { continue }
                // Header and close share a row by design: they may sit side by side, not overlap.
                let need: CGFloat = (parts[a].0 == "header" && parts[b].0 == "close") ? 0 : ChromeLayout.gap - 0.5
                if ra.insetBy(dx: -need / 2, dy: -need / 2).intersects(rb.insetBy(dx: -need / 2, dy: -need / 2)) {
                    out.append((parts[a].0, parts[b].0))
                }
            }
        }
        return out
    }
}

// MARK: - Scene framing

/// An orbit camera pose: the camera sits `distance` from `target` at (`yaw`, `pitch`), looking at the target.
/// Same convention as the renderer: position = target + distance · (cos p · sin y, sin p, cos p · cos y).
public struct OrbitPose: Equatable, Sendable {
    public var target: SIMD3<Float>
    public var yaw: Float
    public var pitch: Float
    public var distance: Float
    public init(target: SIMD3<Float>, yaw: Float, pitch: Float, distance: Float) {
        self.target = target; self.yaw = yaw; self.pitch = pitch; self.distance = distance
    }

    public var direction: SIMD3<Float> { SIMD3(cos(pitch) * sin(yaw), sin(pitch), cos(pitch) * cos(yaw)) }
    public var eye: SIMD3<Float> { target + distance * direction }

    /// Camera basis: right, up, forward (into the screen).
    var basis: (r: SIMD3<Float>, u: SIMD3<Float>, f: SIMD3<Float>) {
        let f = -direction
        var r = simd_cross(f, SIMD3(0, 1, 0))
        if simd_length(r) < 1e-5 { r = SIMD3(1, 0, 0) }
        r = simd_normalize(r)
        return (r, simd_cross(r, f), f)
    }
}

public enum SceneFraming {
    /// Projects a world point to screen points for a camera with a vertical field of view. nil behind the camera.
    public static func project(_ p: SIMD3<Float>, pose: OrbitPose, fovYDegrees: Float, viewport: CGSize, near: Float = 0.2) -> CGPoint? {
        let (r, u, f) = pose.basis
        let v = p - pose.eye
        let z = simd_dot(v, f)
        guard z > near else { return nil }
        let ty = tan(fovYDegrees * .pi / 360)
        let aspect = Float(max(viewport.width, 1) / max(viewport.height, 1))
        let nx = simd_dot(v, r) / (z * ty * aspect), ny = simd_dot(v, u) / (z * ty)
        return CGPoint(x: CGFloat((nx + 1) / 2) * viewport.width, y: CGFloat((1 - ny) / 2) * viewport.height)
    }

    /// The screen rectangle the points cover, nil if any is behind the camera.
    public static func screenBounds(_ pts: [SIMD3<Float>], pose: OrbitPose, fovYDegrees: Float, viewport: CGSize) -> CGRect? {
        var lo = CGPoint(x: CGFloat.greatestFiniteMagnitude, y: .greatestFiniteMagnitude), hi = CGPoint(x: -CGFloat.greatestFiniteMagnitude, y: -.greatestFiniteMagnitude)
        for p in pts {
            guard let s = project(p, pose: pose, fovYDegrees: fovYDegrees, viewport: viewport) else { return nil }
            lo.x = min(lo.x, s.x); lo.y = min(lo.y, s.y); hi.x = max(hi.x, s.x); hi.y = max(hi.y, s.y)
        }
        guard lo.x <= hi.x else { return nil }
        return CGRect(x: lo.x, y: lo.y, width: hi.x - lo.x, height: hi.y - lo.y)
    }

    /// The eight corners of an axis-aligned box.
    public static func corners(min a: SIMD3<Float>, max b: SIMD3<Float>) -> [SIMD3<Float>] {
        var out: [SIMD3<Float>] = []
        for x in [a.x, b.x] { for y in [a.y, b.y] { for z in [a.z, b.z] { out.append(SIMD3(x, y, z)) } } }
        return out
    }

    /// Like `fit`, also turning the camera up to `spread` radians either way from `yaw` (in `steps` steps) and
    /// keeping the turn that shows the subject largest. A mild cost per radian keeps the design angle unless
    /// another angle is clearly better (a long thin building seen more end-on fills a portrait screen).
    public static func fitTurning(_ pts: [SIMD3<Float>], yaw: Float, spread: Float, steps: Int = 4, pitch: Float, fovYDegrees: Float,
                                  viewport: CGSize, free: CGRect, margin: CGFloat = 12, costPerRadian: Float = 0.25) -> OrbitPose {
        var best = fit(pts, yaw: yaw, pitch: pitch, fovYDegrees: fovYDegrees, viewport: viewport, free: free, margin: margin)
        var bestScore = best.distance
        let n = max(steps, 0)
        guard n > 0, spread > 0 else { return best }
        for k in 1...n {
            for sign in [Float(-1), 1] {
                let dy = sign * spread * Float(k) / Float(n)
                let p = fit(pts, yaw: yaw + dy, pitch: pitch, fovYDegrees: fovYDegrees, viewport: viewport, free: free, margin: margin)
                let score = p.distance * (1 + costPerRadian * abs(dy))
                if score < bestScore { best = p; bestScore = score }
            }
        }
        return best
    }

    /// The pose at (`yaw`, `pitch`) whose view puts every point inside `free` (inset by `margin`), as large as
    /// possible and centred in it. The target slides sideways and up/down in the camera plane to centre the
    /// projection; the distance is the smallest that fits.
    public static func fit(_ pts: [SIMD3<Float>], yaw: Float, pitch: Float, fovYDegrees: Float, viewport: CGSize, free: CGRect,
                           margin: CGFloat = 12) -> OrbitPose {
        guard !pts.isEmpty else { return OrbitPose(target: .zero, yaw: yaw, pitch: pitch, distance: 10) }
        var lo = SIMD3<Float>(repeating: .greatestFiniteMagnitude), hi = -lo
        for p in pts where p.x.isFinite && p.y.isFinite && p.z.isFinite { lo = simd_min(lo, p); hi = simd_max(hi, p) }
        guard lo.x <= hi.x else { return OrbitPose(target: .zero, yaw: yaw, pitch: pitch, distance: 10) }
        let centre = (lo + hi) / 2
        let radius = max(simd_length(hi - lo) / 2, 0.1)
        let box = free.insetBy(dx: min(margin, free.width / 4), dy: min(margin, free.height / 4))
        let ty = tan(fovYDegrees * .pi / 360)
        let aspect = Float(max(viewport.width, 1) / max(viewport.height, 1))

        /// Centre the projection in the box at distance d; returns the pose and whether it fits.
        func solve(_ d: Float) -> (OrbitPose, Bool) {
            var pose = OrbitPose(target: centre, yaw: yaw, pitch: pitch, distance: d)
            let (r, u, _) = pose.basis
            for _ in 0..<12 {
                guard let sb = screenBounds(pts, pose: pose, fovYDegrees: fovYDegrees, viewport: viewport) else { return (pose, false) }
                let ex = Float(sb.midX - box.midX), ey = Float(sb.midY - box.midY)
                if abs(ex) < 0.25 && abs(ey) < 0.25 { break }
                // World metres per screen point at the target's depth.
                let mx = 2 * d * ty * aspect / Float(max(viewport.width, 1)), my = 2 * d * ty / Float(max(viewport.height, 1))
                pose.target += r * (ex * mx) - u * (ey * my)
            }
            guard let sb = screenBounds(pts, pose: pose, fovYDegrees: fovYDegrees, viewport: viewport) else { return (pose, false) }
            return (pose, box.insetBy(dx: -0.5, dy: -0.5).contains(sb))
        }

        var nearD = radius * 0.5, farD = radius * 4
        var tries = 0
        while !solve(farD).1 && tries < 40 { farD *= 1.6; tries += 1 }
        for _ in 0..<40 {
            let mid = (nearD + farD) / 2
            if solve(mid).1 { farD = mid } else { nearD = mid }
        }
        return solve(farD).0
    }
}

// MARK: - Label layout

public struct LabelCandidate: Equatable, Sendable {
    public var id: String
    /// The screen point the label belongs to (projected from its object).
    public var anchor: CGPoint
    public var size: CGSize
    /// Higher keeps its place; lower moves aside or is dropped first.
    public var priority: Int
    public init(id: String, anchor: CGPoint, size: CGSize, priority: Int) {
        self.id = id; self.anchor = anchor; self.size = size; self.priority = priority
    }
}

public struct PlacedLabel: Equatable, Sendable {
    public var id: String
    public var frame: CGRect
    /// The slot used (0 = on its anchor; others are offset and draw a leader back to the anchor).
    public var slot: Int
    public var anchor: CGPoint
    public var hasLeader: Bool { slot != 0 }
}

public enum LabelLayout {
    /// Gap kept between two labels, and between a label and the chrome.
    public static let spacing: CGFloat = 3

    /// Offsets tried in order: on the anchor, then above, below, right, left, the diagonals, then farther out.
    /// Each is in label-size units plus a leader length, so a big label moves as far as it needs to.
    static func offset(slot: Int, size s: CGSize) -> CGPoint? {
        let lead: CGFloat = 10
        let dx = s.width / 2 + lead, dy = s.height + lead / 2
        let ring: [CGPoint] = [
            .zero,
            CGPoint(x: 0, y: -dy), CGPoint(x: 0, y: dy), CGPoint(x: dx, y: 0), CGPoint(x: -dx, y: 0),
            CGPoint(x: dx, y: -dy), CGPoint(x: -dx, y: -dy), CGPoint(x: dx, y: dy), CGPoint(x: -dx, y: dy),
            CGPoint(x: 0, y: -2 * dy), CGPoint(x: 0, y: 2 * dy), CGPoint(x: 2 * dx, y: 0), CGPoint(x: -2 * dx, y: 0),
        ]
        return ring.indices.contains(slot) ? ring[slot] : nil
    }
    public static let slotCount = 13

    /// Lays the labels out. A label is placed only if it lies wholly inside `bounds`, clear of every rectangle
    /// in `avoid` (the chrome) and of every label already placed. Labels go in priority order (then by id, so
    /// the result is stable), each trying its previous slot first (no jumping between frames), then the slots
    /// in order. A label with no free slot is dropped: a lower-priority label never pushes out a higher one.
    public static func place(_ labels: [LabelCandidate], bounds: CGRect, avoid: [CGRect], previous: [String: Int] = [:]) -> [PlacedLabel] {
        let order = labels.sorted { $0.priority != $1.priority ? $0.priority > $1.priority : $0.id < $1.id }
        var placed: [PlacedLabel] = []
        let inner = bounds.insetBy(dx: spacing, dy: spacing)
        let blocked = avoid.map { $0.insetBy(dx: -spacing, dy: -spacing) }
        for c in order {
            guard c.anchor.x.isFinite, c.anchor.y.isFinite, c.size.width > 0, c.size.height > 0 else { continue }
            var slots = Array(0..<slotCount)
            if let p = previous[c.id], let i = slots.firstIndex(of: p) { slots.remove(at: i); slots.insert(p, at: 0) }
            for slot in slots {
                guard let o = offset(slot: slot, size: c.size) else { continue }
                let f = CGRect(x: c.anchor.x + o.x - c.size.width / 2, y: c.anchor.y + o.y - c.size.height / 2, width: c.size.width, height: c.size.height)
                guard inner.contains(f) else { continue }
                if blocked.contains(where: { $0.intersects(f) }) { continue }
                let grown = f.insetBy(dx: -spacing, dy: -spacing)
                if placed.contains(where: { $0.frame.intersects(grown) }) { continue }
                placed.append(PlacedLabel(id: c.id, frame: f, slot: slot, anchor: c.anchor))
                break
            }
        }
        return placed
    }
}

// MARK: - Depth fog

/// Distant things fade toward the backdrop. Done as an opaque colour mix chosen per object (no blending, no
/// render-pipeline hooks), quantised into a few steps so materials are shared and only change when an object
/// crosses a step.
public enum DepthFog {
    public static let steps = 6

    /// The fog step (0 = none … `steps`) for an object at `distance`, between the scene's nearest and farthest
    /// depth. Nothing fades in the nearest 35 % of the depth range.
    public static func step(distance: Float, near: Float, far: Float) -> Int {
        guard distance.isFinite, near.isFinite, far.isFinite, far > near + 0.01 else { return 0 }
        let t = (distance - near) / (far - near)
        let start: Float = 0.35
        let u = min(max((t - start) / (1 - start), 0), 1)
        let s = u * u * (3 - 2 * u)
        return min(max(Int((s * Float(steps)).rounded()), 0), steps)
    }

    /// The colour-mix amount for a step, up to `maxMix` at the far end.
    public static func mix(step: Int, maxMix: Float) -> Float {
        Float(min(max(step, 0), steps)) / Float(steps) * min(max(maxMix, 0), 1)
    }
}
