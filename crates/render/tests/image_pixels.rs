//! IMAGE entities draw their pixels: placed by insertion point and pixel vectors, clipped to
//! their clip boundary, through block references, and as a crossed frame when the file is missing.

use cadaddy_color::Rgb;
use cadaddy_doc::{Block, Common, Drawing, Entity, EntityKind, Handle, Image, Insert, Space};
use cadaddy_geom::{Vec2, Vec3};
use cadaddy_render::images::{Raster, insert};
use cadaddy_render::raster::{RasterOptions, View, render};
use cadaddy_render::{DisplayList, Kind, Options, build};

/// A 2 × 2 image: red, green on the top row; blue, white below.
fn quad_image(path: &str) {
    let px = [[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255], [255, 255, 255, 255]];
    insert(path, Raster::new(2, 2, px.concat()).unwrap());
}

/// The image entity: 2 × 2 pixels, each 5 × 5 units, lower-left corner at `at`.
fn image(path: &str, at: Vec2) -> Image {
    Image {
        insert: at.to3(0.0),
        u: Vec3::new(5.0, 0.0, 0.0),
        v: Vec3::new(0.0, 5.0, 0.0),
        size: Vec2::new(2.0, 2.0),
        path: path.into(),
        ..Image::default()
    }
}

/// The colour at world point `p` with the view framing 0..20 × 0..20 at 10 px a unit.
fn color_at(list: &DisplayList, p: Vec2) -> [u8; 3] {
    let view = View { center: Vec2::new(10.0, 10.0), scale: 10.0, width: 200, height: 200 };
    let o = RasterOptions { background: Rgb(0, 0, 0), antialias: false, ..Default::default() };
    let pm = render(list, &view, &o).unwrap();
    let (x, y) = view.to_screen(p);
    let c = pm.pixel(x as u32, y as u32).unwrap();
    // bilinear filtering blends a little of the neighbouring pixel in: snap to 0 or 255
    let snap = |v: u8| {
        if v >= 200 {
            255
        } else if v <= 55 {
            0
        } else {
            v
        }
    };
    [snap(c.red()), snap(c.green()), snap(c.blue())]
}

fn opts() -> Options {
    Options { tolerance: 0.01, ..Options::default() }
}

#[test]
fn pixels_land_where_the_image_is_placed() {
    quad_image("mem://quad-placed");
    let mut d = Drawing::new_imperial();
    d.header.set_i64("IMAGEFRAME", 0);
    d.add(&Space::Model, Common::default(), EntityKind::Image(image("mem://quad-placed", Vec2::new(5.0, 5.0)))).unwrap();
    let list = build(&d, &Space::Model, &opts());
    assert_eq!(list.prims.iter().filter(|p| matches!(p.kind, Kind::Image(_))).count(), 1);
    assert!(list.prims.iter().all(|p| p.kind != Kind::Polyline), "no frame with IMAGEFRAME 0");
    // the file's top row is at the top
    assert_eq!(color_at(&list, Vec2::new(7.5, 12.5)), [255, 0, 0]);
    assert_eq!(color_at(&list, Vec2::new(12.5, 12.5)), [0, 255, 0]);
    assert_eq!(color_at(&list, Vec2::new(7.5, 7.5)), [0, 0, 255]);
    assert_eq!(color_at(&list, Vec2::new(12.5, 7.5)), [255, 255, 255]);
    assert_eq!(color_at(&list, Vec2::new(2.0, 2.0)), [0, 0, 0], "nothing outside it");
}

#[test]
fn clip_boundary_and_block_references() {
    quad_image("mem://quad-clipped");
    let mut d = Drawing::new_imperial();
    d.header.set_i64("IMAGEFRAME", 0);
    // clipped to the left column (pixel x -0.5..0.5)
    let mut img = image("mem://quad-clipped", Vec2::new(0.0, 0.0));
    img.clip = vec![Vec2::new(-0.5, -0.5), Vec2::new(0.5, 1.5)];
    img.clipping = true;
    let mut blk = Block::new("LOGO");
    blk.entities.push(Entity::new(Handle(1), EntityKind::Image(img)));
    d.blocks.insert("LOGO".into(), std::sync::Arc::new(blk));
    let ins = Insert {
        block: "LOGO".into(),
        insert: Vec3::new(5.0, 5.0, 0.0),
        scale: Vec3::new(1.0, 1.0, 1.0),
        rotation: 0.0,
        attribs: vec![],
        cols: 1,
        rows: 1,
        col_spacing: 0.0,
        row_spacing: 0.0,
        clip: None,
    };
    d.add(&Space::Model, Common::default(), EntityKind::Insert(ins)).unwrap();
    let list = build(&d, &Space::Model, &opts());
    assert_eq!(color_at(&list, Vec2::new(7.5, 12.5)), [255, 0, 0], "moved with the block");
    assert_eq!(color_at(&list, Vec2::new(7.5, 7.5)), [0, 0, 255]);
    assert_eq!(color_at(&list, Vec2::new(12.5, 12.5)), [0, 0, 0], "clipped away");
}

#[test]
fn a_missing_file_draws_a_crossed_frame() {
    let mut d = Drawing::new_imperial();
    d.add(&Space::Model, Common::default(), EntityKind::Image(image("/no/such/image.png", Vec2::new(5.0, 5.0)))).unwrap();
    let list = build(&d, &Space::Model, &opts());
    assert!(list.images.is_empty());
    // two diagonals and the frame
    assert_eq!(list.prims.iter().filter(|p| p.kind == Kind::Polyline).count(), 3);
}

#[test]
fn a_black_stencil_shows_white_on_a_dark_screen() {
    // a black mark on a clear background (a logo from a stencil mask)
    insert("mem://stencil", Raster::new(2, 1, vec![0, 0, 0, 255, 0, 0, 0, 0]).unwrap());
    let mut d = Drawing::new_imperial();
    d.header.set_i64("IMAGEFRAME", 0);
    let mut img = image("mem://stencil", Vec2::new(5.0, 5.0));
    img.size = Vec2::new(2.0, 1.0);
    d.add(&Space::Model, Common::default(), EntityKind::Image(img)).unwrap();
    let list = build(&d, &Space::Model, &opts());
    assert_eq!(color_at(&list, Vec2::new(7.5, 7.5)), [255, 255, 255], "white on the dark background");
    assert_eq!(color_at(&list, Vec2::new(12.5, 7.5)), [0, 0, 0], "the clear part shows the background");
    let view = View { center: Vec2::new(10.0, 10.0), scale: 10.0, width: 200, height: 200 };
    let paper = RasterOptions { background: Rgb(255, 255, 255), antialias: false, ..Default::default() };
    let pm = render(&list, &view, &paper).unwrap();
    let (x, y) = view.to_screen(Vec2::new(7.5, 7.5));
    assert!(pm.pixel(x as u32, y as u32).unwrap().red() < 20, "black on paper");
}
