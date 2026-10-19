//! Tour export: a camera tour rendered frame by frame to numbered PNG files, and those turned into
//! an MP4 with ffmpeg when it is installed (no permissive pure-Rust H.264 encoder exists yet; the
//! command is printed so it can be run by hand). The renderer is passed in: the headless walk
//! renderer (`cadaddy-walk --tour`) uses the GPU, tests a stand-in.

use super::{Building, put_pose};
use cadaddy_walk3d::Walker;
use std::path::{Path, PathBuf};

/// Largest frame side, pixels.
pub const MAX_SIDE: u32 = 8192;

/// What an export made.
#[derive(Clone, Debug, PartialEq)]
pub struct Export {
    pub frames: usize,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub duration: f64,
    pub dir: PathBuf,
}

/// The file name of frame `i` (from 0).
pub fn frame_name(i: usize) -> String {
    format!("frame_{i:05}.png")
}

/// The ffmpeg arguments that make `out` (an H.264 MP4 any player opens) from an export's frames.
pub fn ffmpeg_args(e: &Export, out: &Path) -> Vec<String> {
    vec![
        "-y".into(),
        "-loglevel".into(),
        "error".into(),
        "-framerate".into(),
        format!("{}", e.fps),
        "-i".into(),
        e.dir.join("frame_%05d.png").display().to_string(),
        "-c:v".into(),
        "libx264".into(),
        "-pix_fmt".into(),
        "yuv420p".into(),
        // Even sides (yuv420p needs them).
        "-vf".into(),
        "pad=ceil(iw/2)*2:ceil(ih/2)*2".into(),
        out.display().to_string(),
    ]
}

/// The command line to run by hand, for the message when ffmpeg isn't installed.
pub fn ffmpeg_command(e: &Export, out: &Path) -> String {
    let quote = |a: &str| if a.contains([' ', '(', ')', '*']) { format!("\"{a}\"") } else { a.to_string() };
    std::iter::once("ffmpeg".to_string()).chain(ffmpeg_args(e, out).iter().map(|a| quote(a))).collect::<Vec<_>>().join(" ")
}

/// Render tour `name` of `b` at `fps` into `dir` (made if missing): one PNG per frame from
/// [`frame_name`]. `render` draws the scene from a camera, returning (width, height, RGBA).
pub fn export_frames(
    b: &Building,
    name: &str,
    fps: f64,
    dir: &Path,
    mut render: impl FnMut(&Walker) -> Result<(u32, u32, Vec<u8>), String>,
) -> Result<Export, String> {
    if !(0.1..=240.0).contains(&fps) {
        return Err("frames per second must be between 0.1 and 240".into());
    }
    let tours = cadaddy_walk3d::Tours::from_json(b.drawings.first().and_then(|d| d.tours.as_deref()))?;
    let tour = tours.tour(name).ok_or_else(|| format!("no tour named {name}"))?;
    let path = tours.path(tour)?;
    let frames = path.frame_count(fps);
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut w = Walker::spawn(&b.scene, 0);
    w.fly = true;
    let (mut width, mut height) = (0, 0);
    for i in 0..frames {
        put_pose(&mut w, &b.scene, path.pose(i as f64 / fps));
        let (fw, fh, rgba) = render(&w)?;
        let img = image::RgbaImage::from_raw(fw, fh, rgba).ok_or("the renderer gave a frame of the wrong size")?;
        let file = dir.join(frame_name(i));
        img.save(&file).map_err(|e| format!("{}: {e}", file.display()))?;
        (width, height) = (fw, fh);
    }
    Ok(Export { frames, width, height, fps, duration: path.duration(), dir: dir.to_path_buf() })
}

/// Make `out` from an export's frames with ffmpeg. `Ok(false)` when ffmpeg isn't installed.
#[cfg(not(target_arch = "wasm32"))]
pub fn encode_mp4(e: &Export, out: &Path) -> Result<bool, String> {
    match std::process::Command::new("ffmpeg").args(ffmpeg_args(e, out)).output() {
        Ok(o) if o.status.success() => Ok(true),
        Ok(o) => Err(format!("ffmpeg failed: {}", String::from_utf8_lossy(&o.stderr).trim())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(format!("ffmpeg: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_doc::Drawing;
    use std::sync::Arc;

    #[test]
    fn a_tour_exports_one_frame_per_tick_at_the_renderers_size() {
        let mut d = Drawing::new_imperial();
        d.tours = Some(
            r#"{"version":1,"views":[{"name":"A","eye":[0,0,5.5]},{"name":"B","eye":[9,0,5.5],"yaw":90}],
                "tours":[{"name":"T","stops":["A","B"],"speed":3,"look":"views"}]}"#
                .into(),
        );
        let b = Building::load(Arc::new(d), None).unwrap();
        let dir = std::env::temp_dir().join(format!("cadaddy_tour_export_{}", std::process::id()));
        let mut eyes = Vec::new();
        let e = export_frames(&b, "t", 4.0, &dir, |w| {
            eyes.push((w.eye.x, w.yaw.to_degrees()));
            Ok((32, 18, vec![200; 32 * 18 * 4]))
        })
        .unwrap();
        // 9 ft at 3 ft/s = 3 s; at 4 fps, frames at 0, 0.25 … 3 s.
        assert_eq!((e.frames, e.width, e.height), (13, 32, 18));
        assert!((e.duration - 3.0).abs() < 1e-9);
        assert!(eyes.first().is_some_and(|f| f.0.abs() < 1e-9));
        assert!(eyes.last().is_some_and(|l| (l.0 - 9.0).abs() < 1e-9 && (l.1 - 90.0).abs() < 1e-6), "{eyes:?}");
        let files = std::fs::read_dir(&dir).unwrap().filter_map(|f| f.ok()).filter(|f| f.file_name().to_string_lossy().starts_with("frame_")).count();
        assert_eq!(files, 13);
        let img = image::open(dir.join(frame_name(12))).unwrap();
        assert_eq!((img.width(), img.height()), (32, 18));
        let cmd = ffmpeg_command(&e, Path::new("out.mp4"));
        assert!(cmd.starts_with("ffmpeg -y") && cmd.contains("-framerate 4 ") && cmd.contains("frame_%05d.png") && cmd.ends_with("out.mp4"), "{cmd}");
        // Bad input.
        assert!(export_frames(&b, "nope", 4.0, &dir, |_| Ok((1, 1, vec![0; 4]))).is_err());
        assert!(export_frames(&b, "T", 0.0, &dir, |_| Ok((1, 1, vec![0; 4]))).is_err());
        assert!(export_frames(&b, "T", 4.0, &dir, |_| Ok((2, 2, vec![0; 4]))).is_err(), "a short frame");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
