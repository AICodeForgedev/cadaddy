// draw_star.rhai — Draw a five-pointed star using LINE commands.
//
// Run with:  rhai {code: "<this file's contents>"}
// Or:        rhairun {path: "examples/rhai/draw_star.rhai"}

let cx = 0.0;
let cy = 0.0;
let r = 10.0;
let pi = 3.14159265358979;

// Compute star vertices (5 outer points, every other one connected).
let pts = [];
for i in 0..5 {
    let angle = pi / 2.0 + (i * 2) * 2.0 * pi / 5.0;
    let x = cx + r * cos(angle);
    let y = cy + r * sin(angle);
    pts.push([x, y]);
}

// Close the star.
pts.push(pts[0]);

// Draw it as a single polyline.
command("line", #{ points: pts });

print(`Drew a star with ${pts.len() - 1} points at (${cx}, ${cy}).`);
