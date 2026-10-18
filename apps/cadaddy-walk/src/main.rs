//! Headless walk-mode renderer: derive a drawing's 3D (as CADaddy's View ▸ Walk does) and render
//! one frame to a PNG, for checks and documentation. Walk mode itself lives in CADaddy.
//!
//! `cadaddy-walk FILE --shot OUT.png [--cam x,y,z,yaw°,pitch°] [--size WxH] [--level N] [--cut]`
//! FILE is a drawing (its `.walk.json` beside it, or a building file listing it) or a building
//! file (`.walk.json`) itself.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

use cadaddy_ui_egui::walk::{Building, frame_for, gpu};
use cadaddy_walk3d::{V3, Walker};
use egui_wgpu::wgpu;
use std::path::PathBuf;

struct Args {
    file: PathBuf,
    shot: PathBuf,
    cam: Option<[f64; 5]>,
    size: (u32, u32),
    level: usize,
    cut: bool,
}

fn args() -> Result<Args, String> {
    let (mut file, mut shot, mut cam, mut size, mut level, mut cut) = (None, None, None, (1600, 900), 0, false);
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut val = || it.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--shot" => shot = Some(PathBuf::from(val()?)),
            "--level" => level = val()?.parse().map_err(|_| "--level N")?,
            "--cut" => cut = true,
            "--size" => {
                let v = val()?;
                let (w, h) = v.split_once('x').ok_or("--size WxH")?;
                size = (w.parse().map_err(|_| "--size WxH")?, h.parse().map_err(|_| "--size WxH")?);
            }
            "--cam" => {
                let v: Vec<f64> = val()?.split(',').filter_map(|s| s.trim().parse().ok()).collect();
                let [x, y, z, yaw, pitch] = v.as_slice() else { return Err("--cam x,y,z,yaw,pitch (feet, degrees)".into()) };
                cam = Some([*x, *y, *z, *yaw, *pitch]);
            }
            _ if !arg.starts_with("--") => file = Some(PathBuf::from(arg)),
            _ => return Err(format!("unknown option {arg}")),
        }
    }
    let usage = "usage: cadaddy-walk FILE --shot OUT.png [--cam x,y,z,yaw,pitch] [--size WxH] [--level N] [--cut]";
    Ok(Args { file: file.ok_or(usage)?, shot: shot.ok_or(usage)?, cam, size, level, cut })
}

/// The drawing readers (what CADaddy installs at startup), so floor sheets can be read.
fn install_io() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, name| cadaddy_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadaddy_io::write(d, name).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
    });
}

fn run(a: &Args) -> Result<(), String> {
    install_io();
    let b = Building::load_file(&a.file)?;
    for p in &b.problems {
        eprintln!("cadaddy-walk: empty floor: {p}");
    }
    let mut w = Walker::spawn(&b.scene, a.level);
    if let Some([x, y, z, yaw, pitch]) = a.cam {
        w.eye = V3 { x, y, z };
        w.yaw = yaw.to_radians();
        w.pitch = pitch.to_radians();
    }
    let instance = wgpu::Instance::default();
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).map_err(|e| format!("no GPU adapter: {e}"))?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).map_err(|e| format!("GPU device: {e}"))?;
    let mut g = gpu::Gpu::new(&device);
    g.upload(&device, &b.scene);
    g.render(&device, &queue, a.size, &frame_for(&b.scene, &w, a.size, a.cut));
    let (width, height, rgba) = g.read_rgba(&device, &queue)?;
    let img = image::RgbaImage::from_raw(width, height, rgba).ok_or("bad image size")?;
    img.save(&a.shot).map_err(|e| format!("{}: {e}", a.shot.display()))?;
    let s = &b.scene;
    println!(
        "{}: rules {}; {} levels, {} wall faces ({} closed into solid walls), {} doors, {} windows, {} cased openings, {} devices, {} unplaced -> {}",
        a.file.display(),
        b.rules_from,
        s.levels.len(),
        s.walls.len(),
        s.solid_walls,
        s.doors,
        s.windows,
        s.cased,
        s.devices.len(),
        s.unplaced,
        a.shot.display()
    );
    Ok(())
}

fn main() {
    if let Err(e) = args().and_then(|a| run(&a)) {
        eprintln!("cadaddy-walk: {e}");
        std::process::exit(1);
    }
}
