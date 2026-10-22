// list_layers.rhai — List all layers in the current drawing.
//
// Demonstrates getvar() for reading system variables.

let result = command("layer", #{ action: "list" });
print("Layers in the current drawing:");
if result.layers != () {
    for layer in result.layers {
        print(`  ${layer}`);
    }
}
