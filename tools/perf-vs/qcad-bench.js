// QCAD timing script for the perf-vs comparison (docs/perf-vs.md).
//
// Run inside QCAD Professional's script console or with:
//   qcad -exec tools/perf-vs/qcad-bench.js -- target/perf/vs/plan-10000.dxf
//
// The script opens the given DXF file via RDocumentInterface, selects all
// entities, and saves as DXF, printing the time each step took.  Run it on
// the plan files that `cargo xtask perf --vs` writes to target/perf/vs/.

var args = QCoreApplication.arguments();
var path = "";
for (var i = 0; i < args.length; i++) {
    if (args[i].match(/\.dxf$/i)) {
        path = args[i];
    }
}
if (!path) {
    print("usage: qcad -exec qcad-bench.js -- <plan.dxf>");
    quit();
}

function now() {
    return Date.now();
}

// Open the DXF file.
var t0 = now();
var di = new RDocumentInterface();
di.importFile(path);
var openMs = now() - t0;

var doc = di.getDocument();
var count = doc.countEntities();

// Select all entities.
var t1 = now();
var op = new RModifyObjectsOperation();
var ids = doc.queryAllEntities(false, false);
for (var j = 0; j < ids.length; j++) {
    var e = doc.queryEntity(ids[j]);
    if (!isNull(e)) {
        e.setSelected(true);
        op.addObject(e);
    }
}
di.applyOperation(op);
var selectMs = now() - t1;

// Save as DXF.
var outPath = path.replace(/\.dxf$/i, "-out.dxf");
var t2 = now();
di.exportFile(outPath);
var saveMs = now() - t2;

print("QCAD results for " + path + ":");
print("  objects:    " + count);
print("  open:       " + openMs + " ms");
print("  select all: " + selectMs + " ms");
print("  save DXF:   " + saveMs + " ms");

di.destroy();
