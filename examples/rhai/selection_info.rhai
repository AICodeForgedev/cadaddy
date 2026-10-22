// selection_info.rhai — Report on the current selection.
//
// Demonstrates selection(), sslength(), ssname(), and entity().

let ss = selection();
let count = sslength(ss);

if count == 0 {
    print("Nothing selected.");
} else {
    print(`${count} object(s) selected:`);
    for i in 0..count {
        let handle = ssname(ss, i);
        let ent = entity(handle);
        print(`  [${handle}] ${ent.type}`);
    }
}
