//! Video from numbered PNG frames: an H.264 MP4 made with ffmpeg when it is installed (no
//! permissive pure-Rust H.264 encoder exists yet; without ffmpeg the command is given so it can
//! be run by hand). Camera tours (`cadaddy-walk --tour`) and 4D exports (`fourd.export`) share it.

use std::path::Path;

/// The file name of frame `i` (from 0).
pub fn frame_name(i: usize) -> String {
    format!("frame_{i:05}.png")
}

/// The ffmpeg arguments that make `out` (an H.264 MP4 any player opens) from the frames in `dir`
/// shown at `fps`.
pub fn ffmpeg_args(dir: &Path, fps: f64, out: &Path) -> Vec<String> {
    vec![
        "-y".into(),
        "-loglevel".into(),
        "error".into(),
        "-framerate".into(),
        format!("{fps}"),
        "-i".into(),
        dir.join("frame_%05d.png").display().to_string(),
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
pub fn ffmpeg_command(dir: &Path, fps: f64, out: &Path) -> String {
    let quote = |a: &str| if a.contains([' ', '(', ')', '*']) { format!("\"{a}\"") } else { a.to_string() };
    std::iter::once("ffmpeg".to_string()).chain(ffmpeg_args(dir, fps, out).iter().map(|a| quote(a))).collect::<Vec<_>>().join(" ")
}

/// Make `out` from the frames in `dir` with ffmpeg. `Ok(false)` when ffmpeg isn't installed.
#[cfg(not(target_arch = "wasm32"))]
pub fn encode_mp4(dir: &Path, fps: f64, out: &Path) -> Result<bool, String> {
    match std::process::Command::new("ffmpeg").args(ffmpeg_args(dir, fps, out)).output() {
        Ok(o) if o.status.success() => Ok(true),
        Ok(o) => Err(format!("ffmpeg failed: {}", String::from_utf8_lossy(&o.stderr).trim())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(format!("ffmpeg: {e}")),
    }
}
