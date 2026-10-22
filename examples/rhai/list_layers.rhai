// list_layers.rhai — List all layers in the current drawing.
//
// Demonstrates reading a command's result: command("layer") with no changes returns the
// drawing's layers, each a map with its name, colour, on/frozen/locked state and so on.

let result = command("layer");
print("Layers in the current drawing:");
for layer in result.layers {
    let state = if layer.on { "on" } else { "off" };
    print(`  ${layer.name} (${state})`);
}
