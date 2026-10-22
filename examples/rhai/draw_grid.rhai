// draw_grid.rhai — Draw a grid of lines.
//
// Demonstrates loops and the command() function.

let cols = 5;
let rows = 5;
let spacing = 10.0;

// Horizontal lines.
for r in 0..=rows {
    let y = r as_decimal * spacing;
    command("line", #{ points: [[0.0, y], [cols as_decimal * spacing, y]] });
}

// Vertical lines.
for c in 0..=cols {
    let x = c as_decimal * spacing;
    command("line", #{ points: [[x, 0.0], [x, rows as_decimal * spacing]] });
}

print(`Drew a ${cols}x${rows} grid with ${spacing} unit spacing.`);
