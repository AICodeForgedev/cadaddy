//! Where the 2D canvas spends its time: per regeneration (the display list built for the view,
//! by kind of object, then turned into GPU vertex data and uploaded) and per frame (what is sent
//! to be drawn, what of it is in view, how long the canvas and the whole frame took), with the
//! GPU and system memory and the adapter. Shown on the `--debug` overlay and returned by the
//! `ui.draw2d` control method; counting is on only while one of them asks (the build profile and
//! the in-view count cost time of their own).
//!
//! A large print set sits side by side in one model space: the counts here make the cost of what
//! is off screen visible. A large drawing's GPU data is cut into area tiles and a frame draws only
//! the tiles in view (Q6472); a zoom band builds again only the store chunks the view reaches.

use cadaddy_render::DisplayList;
use cadaddy_render::profile::{BuildProfile, Part, ProfileReport};
use serde_json::{Value, json};

use cadaddy_geom::Bounds2;

/// The last regeneration of the canvas's display list.
#[derive(Clone, Debug, Default)]
pub struct Regen {
    /// Built on the worker thread (a view change) rather than at once on the UI thread.
    pub background: bool,
    /// Objects in the space the list was built for.
    pub objects: usize,
    /// What the build looked at and drew, and its time by kind of object (CPU, summed over
    /// threads). A list built from cached parts looks only at the parts that changed.
    pub profile: ProfileReport,
    /// Wall-clock milliseconds of the build.
    pub build_ms: f64,
    pub prims: usize,
    pub verts: usize,
    pub tris: usize,
    /// Bytes of the list's primitives and points.
    pub list_bytes: usize,
    /// Converting the list to vertex data (GPU canvas).
    pub mesh_ms: f64,
    pub line_vertices: usize,
    pub tri_vertices: usize,
    pub quad_instances: usize,
    pub mesh_bytes: usize,
    /// What the vertex data draws simpler than the list (objects under a pixel as dots, small
    /// ones as their box: [`crate::gpu::build_screen_mesh`]).
    pub simplified: crate::gpu::Simplified,
    /// Creating its GPU buffers, when done on the worker (else see [`Frame::upload_ms`]).
    pub upload_ms: f64,
    /// The area tiles the vertex data is cut into (0: not cut, drawn whole).
    pub tiles: usize,
    /// Model-space store chunks built again, and kept from an earlier build at another zoom
    /// band (out of the view).
    pub chunks_rebuilt: usize,
    pub chunks_kept: usize,
}

/// The last frame.
#[derive(Clone, Debug, Default)]
pub struct Frame {
    /// The app's update (everything but egui's tessellation and the GPU), and the canvas's part.
    pub app_ms: f64,
    pub canvas_ms: f64,
    /// Since the frame before: with the overlay repainting all the time, the frame rate's
    /// inverse, which includes presenting and waiting on the GPU.
    pub interval_ms: f64,
    /// Primitives sent to be drawn: on the GPU canvas those of the tiles in view (all of the
    /// list when it isn't cut into tiles), on the CPU canvas the ones in view.
    pub prims_sent: usize,
    /// Vertices the GPU runs (six an instance), and the tiles drawn.
    pub gpu_vertices: usize,
    pub tiles_drawn: usize,
    /// Primitives at least partly in view, and the time to find them (the CPU canvas does this
    /// every frame; the GPU canvas does not cull, it is counted for the report only).
    pub prims_in_view: usize,
    pub cull_ms: f64,
    /// The slowest GPU preparation since the last frame (uploading a changed list there).
    pub upload_ms: f64,
}

#[derive(Default)]
pub struct DrawStats {
    /// Count from now on (see [`DrawStats::wanted`]).
    on: bool,
    pub regen: Regen,
    pub frame: Frame,
    /// The profile of the build in flight, when one is counted.
    pending: Option<std::sync::Arc<BuildProfile>>,
    last_frame_at: Option<f64>,
    /// The list generation and view the in-view count is for.
    in_view_key: Option<(u64, [u64; 4])>,
    /// Vertex data made on the UI thread is uploaded while the frame is prepared for the GPU,
    /// after the frame: its upload is the regeneration's, reported at the end of the next frame
    /// (2: made this frame, 1: the next frame reports it).
    upload_pending: u8,
}

impl DrawStats {
    /// Count while `on` (the overlay is shown or the control channel asked).
    pub fn set_on(&mut self, on: bool) {
        self.on = on;
        if !on {
            self.pending = None;
            self.in_view_key = None;
        }
    }

    pub fn is_on(&self) -> bool {
        self.on
    }

    /// A profile for a build about to start (`None` when not counting).
    pub(super) fn profile_for_build(&mut self) -> Option<std::sync::Arc<BuildProfile>> {
        let p = self.on.then(BuildProfile::new);
        self.pending.clone_from(&p);
        p
    }

    /// A build finished: `list` took `build_ms`, profiled into `report` (or the pending profile).
    pub(super) fn built(&mut self, list: &DisplayList, objects: usize, build_ms: f64, background: bool, report: Option<ProfileReport>) {
        if !self.on {
            return;
        }
        let profile = report.or_else(|| self.pending.take().map(|p| p.report())).unwrap_or_default();
        self.regen = Regen {
            background,
            objects,
            profile,
            build_ms,
            prims: list.prims.len(),
            verts: list.verts.len(),
            tris: list.tris.len(),
            list_bytes: list_bytes(list),
            ..Regen::default()
        };
        self.in_view_key = None;
    }

    /// The list was converted to vertex data in `ms` (`upload_ms`: its buffers made too).
    pub(super) fn meshed(&mut self, mesh: &crate::gpu::MeshData, ms: f64, upload_ms: f64) {
        if !self.on {
            return;
        }
        let r = &mut self.regen;
        (r.mesh_ms, r.upload_ms) = (ms, upload_ms);
        (r.line_vertices, r.tri_vertices, r.quad_instances) = (mesh.line_vertices(), mesh.tri_vertices(), mesh.quad_instances());
        r.mesh_bytes = mesh.bytes();
        r.simplified = mesh.simplified;
        self.upload_pending = 2;
    }

    /// The vertex data was cut into `tiles` (`None`: drawn whole).
    pub(super) fn tiled(&mut self, tiles: Option<&crate::gpu::Tiles>) {
        if self.on {
            self.regen.tiles = tiles.map_or(0, |t| t.bounds.len());
        }
    }

    /// The model-space build made `rebuilt` chunks again and kept `kept` at another zoom band.
    pub(super) fn chunks(&mut self, rebuilt: usize, kept: usize) {
        if self.on {
            (self.regen.chunks_rebuilt, self.regen.chunks_kept) = (rebuilt, kept);
        }
    }

    /// A tiled mesh drew `d` this frame (after [`DrawStats::drew`]).
    pub(super) fn drew_tiles(&mut self, d: crate::gpu::TilesDrawn) {
        if self.on {
            let f = &mut self.frame;
            (f.prims_sent, f.gpu_vertices, f.tiles_drawn) = (d.prims, d.vertices, d.tiles);
        }
    }

    /// The vertex counts of a list already uploaded (the buffers no longer hold a CPU copy).
    pub(super) fn meshed_counts(&mut self, counts: [usize; 4], simplified: crate::gpu::Simplified, ms: f64, upload_ms: f64) {
        if !self.on {
            return;
        }
        let r = &mut self.regen;
        (r.mesh_ms, r.upload_ms) = (ms, upload_ms);
        [r.line_vertices, r.tri_vertices, r.quad_instances, r.mesh_bytes] = counts;
        r.simplified = simplified;
    }

    /// The canvas drew `list` this frame at `visible`: `sent` primitives (`None`: all of them,
    /// the GPU canvas).
    pub(super) fn drew(&mut self, list: &DisplayList, list_gen: u64, visible: &Bounds2, sent: Option<usize>) {
        if !self.on {
            return;
        }
        let f = &mut self.frame;
        f.prims_sent = sent.unwrap_or(list.prims.len());
        f.gpu_vertices = if sent.is_none() { self.regen.line_vertices + self.regen.tri_vertices + 6 * self.regen.quad_instances } else { 0 };
        f.tiles_drawn = 0;
        let key = (list_gen, [visible.min.x.to_bits(), visible.min.y.to_bits(), visible.max.x.to_bits(), visible.max.y.to_bits()]);
        if self.in_view_key != Some(key) {
            let t0 = crate::now_ms();
            f.prims_in_view = in_view(list, visible);
            f.cull_ms = crate::now_ms() - t0;
            self.in_view_key = Some(key);
        }
    }

    /// A frame ended: the app took `app_ms` of it, the canvas `canvas_ms`, the GPU preparation
    /// `upload_ms` (the slowest since the last frame).
    pub fn frame_done(&mut self, start_ms: f64, app_ms: f64, canvas_ms: f64, upload_ms: f64) {
        if !self.on {
            self.last_frame_at = None;
            return;
        }
        let f = &mut self.frame;
        f.interval_ms = self.last_frame_at.map_or(0.0, |t| start_ms - t);
        (f.app_ms, f.canvas_ms, f.upload_ms) = (app_ms, canvas_ms, upload_ms);
        self.last_frame_at = Some(start_ms);
        if self.upload_pending == 1 && upload_ms > 0.0 {
            self.regen.upload_ms = upload_ms;
        }
        self.upload_pending = self.upload_pending.saturating_sub(1);
    }

    /// The report: what the overlay shows and `ui.draw2d` returns. `adapter`: the GPU's name
    /// ("CPU canvas" without one); `rss_mb`: the process's memory.
    pub fn json(&self, adapter: &str, gpu: bool, gpu_bytes: u64, rss_mb: Option<f64>) -> Value {
        let (r, f) = (&self.regen, &self.frame);
        let round = |v: f64| (v * 100.0).round() / 100.0;
        let parts: serde_json::Map<String, Value> =
            r.profile.parts.iter().map(|p| (p.part.name().to_string(), json!({ "ms": round(p.ms), "objects": p.objects }))).collect();
        json!({
            "on": self.on,
            "adapter": adapter,
            "renderer": if gpu { "gpu" } else { "cpu" },
            "regen": {
                "background": r.background,
                "objects": r.objects,
                "considered": r.profile.considered,
                "drawn": r.profile.drawn,
                "buildMs": round(r.build_ms),
                "cpuMs": round(r.profile.total_ms()),
                "parts": parts,
                "prims": r.prims, "verts": r.verts, "tris": r.tris,
                "listBytes": r.list_bytes,
                "meshMs": round(r.mesh_ms),
                "uploadMs": round(r.upload_ms),
                "lineVertices": r.line_vertices, "triVertices": r.tri_vertices, "quadInstances": r.quad_instances,
                "meshBytes": r.mesh_bytes,
                "tiles": r.tiles,
                "chunksRebuilt": r.chunks_rebuilt,
                "chunksKept": r.chunks_kept,
                "simplified": {
                    "dotObjects": r.simplified.dot_objects,
                    "dotPrims": r.simplified.dot_prims,
                    "dots": r.simplified.dots,
                    "boxes": r.simplified.boxes,
                },
            },
            "frame": {
                "appMs": round(f.app_ms),
                "canvasMs": round(f.canvas_ms),
                "intervalMs": round(f.interval_ms),
                "primsSent": f.prims_sent,
                "gpuVertices": f.gpu_vertices,
                "tilesDrawn": f.tiles_drawn,
                "primsInView": f.prims_in_view,
                "cullMs": round(f.cull_ms),
                "uploadMs": round(f.upload_ms),
            },
            "memory": { "gpuBytes": gpu_bytes, "rssMb": rss_mb.map(|m| m.round()) },
        })
    }

    /// The overlay's lines.
    pub fn lines(&self, adapter: &str) -> Vec<String> {
        let (r, f) = (&self.regen, &self.frame);
        let p = &r.profile;
        let mut out = vec![
            format!("2d: {adapter}"),
            format!("objects {}  considered {}  drew {}", r.objects, p.considered, p.drawn),
            format!("regen {:.1} ms{} (cpu {:.1})", r.build_ms, if r.background { " bg" } else { "" }, p.total_ms()),
        ];
        out.extend(Part::ALL.iter().map(|&k| format!("  {:<10} {:>8.1} ms {:>9}", k.name(), p.ms(k), p.objects(k))));
        out.push(format!("mesh {:.1} ms  upload {:.1} ms", r.mesh_ms, r.upload_ms.max(f.upload_ms)));
        out.push(format!("list {} prims  {} MB", r.prims, r.list_bytes / 1_000_000));
        out.push(format!("gpu {} verts  {} MB", r.line_vertices + r.tri_vertices + 6 * r.quad_instances, r.mesh_bytes / 1_000_000));
        let sp = &r.simplified;
        out.push(format!("sub-pixel {} objects + {} prims as {} dots  {} as boxes", sp.dot_objects, sp.dot_prims, sp.dots, sp.boxes));
        out.push(format!("frame {:.1} ms (canvas {:.1})  every {:.1} ms", f.app_ms, f.canvas_ms, f.interval_ms));
        out.push(format!("sent {}  in view {}  cull {:.1} ms", f.prims_sent, f.prims_in_view, f.cull_ms));
        if r.tiles > 0 {
            out.push(format!("tiles {} of {}  {} verts drawn", f.tiles_drawn, r.tiles, f.gpu_vertices));
        }
        out.push(format!("chunks rebuilt {}  kept {}", r.chunks_rebuilt, r.chunks_kept));
        out
    }
}

/// Bytes of `list`'s primitives and points.
fn list_bytes(list: &DisplayList) -> usize {
    list.prims.len() * std::mem::size_of::<cadaddy_render::DPrim>() + (list.verts.len() + list.tris.len()) * std::mem::size_of::<cadaddy_geom::Vec2>()
}

/// Primitives of `list` at least partly inside `visible` (infinite lines always are).
pub fn in_view(list: &DisplayList, visible: &Bounds2) -> usize {
    list.prims
        .iter()
        .filter(|p| {
            matches!(p.kind, cadaddy_render::Kind::Infinite { .. }) || Bounds2::from_points(list.points(p).iter().copied()).intersects(visible)
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_doc::{Common, Drawing, EntityKind, Line, Space};
    use cadaddy_geom::{Vec2, Vec3};

    fn lines(n: usize, step: f64) -> Drawing {
        let mut d = Drawing::new_imperial();
        for i in 0..n {
            let x = i as f64 * step;
            let l = Line { a: Vec3::new(x, 0.0, 0.0), b: Vec3::new(x + 1.0, 1.0, 0.0) };
            let Ok(_) = d.add(&Space::Model, Common::default(), EntityKind::Line(l)) else { panic!("add") };
        }
        d
    }

    #[test]
    fn a_counted_build_reports_what_it_looked_at_and_its_parts() {
        let d = lines(40, 10.0);
        let mut s = DrawStats::default();
        // Off: no profile, nothing recorded.
        assert!(s.profile_for_build().is_none());
        s.set_on(true);
        let profile = s.profile_for_build();
        let opts = cadaddy_render::Options { profile, ..Default::default() };
        let list = cadaddy_render::build(&d, &Space::Model, &opts);
        s.built(&list, 40, 1.0, false, None);
        assert_eq!(s.regen.profile.considered, 40);
        assert_eq!(s.regen.profile.drawn, 40);
        assert_eq!(s.regen.profile.objects(Part::Curves), 40);
        assert_eq!(s.regen.prims, list.prims.len());
        // A view of the first two lines: everything is sent on the GPU canvas, two are in view.
        let view = Bounds2::new(Vec2::new(-1.0, -1.0), Vec2::new(12.0, 2.0));
        s.drew(&list, 1, &view, None);
        assert_eq!(s.frame.prims_sent, 40);
        assert_eq!(s.frame.prims_in_view, 2);
        let v = s.json("Test GPU", true, 0, None);
        assert_eq!(v["regen"]["considered"], 40);
        assert_eq!(v["regen"]["parts"]["curves"]["objects"], 40);
        assert_eq!(v["frame"]["primsInView"], 2);
        assert!(s.lines("Test GPU").iter().any(|l| l.contains("in view 2")));
    }

    #[test]
    fn frames_report_their_interval_once_counting() {
        let mut s = DrawStats::default();
        s.frame_done(100.0, 5.0, 2.0, 0.0);
        s.set_on(true);
        s.frame_done(200.0, 5.0, 2.0, 0.0);
        assert_eq!(s.frame.interval_ms, 0.0);
        s.frame_done(216.0, 6.0, 3.0, 1.5);
        assert_eq!(s.frame.interval_ms, 16.0);
        assert_eq!((s.frame.app_ms, s.frame.canvas_ms, s.frame.upload_ms), (6.0, 3.0, 1.5));
    }

    #[test]
    fn vertex_data_made_in_a_frame_gets_the_upload_the_next_frame_reports() {
        let mut s = DrawStats::default();
        s.set_on(true);
        s.meshed(&crate::gpu::MeshData::default(), 4.0, 0.0);
        // The frame that made it reports an older preparation's upload: not this one's.
        s.frame_done(0.0, 1.0, 1.0, 0.7);
        assert_eq!(s.regen.upload_ms, 0.0);
        s.frame_done(16.0, 1.0, 1.0, 9.0);
        assert_eq!((s.regen.mesh_ms, s.regen.upload_ms), (4.0, 9.0));
        // Later uploads (a highlight, a drag) are the frame's only.
        s.frame_done(32.0, 1.0, 1.0, 3.0);
        assert_eq!(s.regen.upload_ms, 9.0);
    }
}
