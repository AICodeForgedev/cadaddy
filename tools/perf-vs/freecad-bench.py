#!/usr/bin/env python3
"""FreeCAD timing script for the perf-vs comparison (docs/perf-vs.md).

Run with the FreeCAD command-line interpreter::

    freecadcmd tools/perf-vs/freecad-bench.py target/perf/vs/plan-10000.dxf

The script opens the given DXF file via importDXF, counts the imported objects,
and saves them as DXF, printing the time each step took.  Run it on the plan
files that ``cargo xtask perf --vs`` writes to ``target/perf/vs/``.

FreeCAD converts each DXF entity into a 3D shape (importDXF.insert), which is
much slower than programs that keep the 2D geometry.  There is no scriptable
"select all" in FreeCAD's headless mode, so that column is reported as "-".
"""

import os
import sys
import time

path = None
for arg in sys.argv[1:]:
    if arg.lower().endswith(".dxf"):
        path = arg
        break

if not path:
    print("usage: freecadcmd freecad-bench.py <plan.dxf>")
    sys.exit(1)

import FreeCAD  # noqa: E402  (available inside freecadcmd)
import importDXF  # noqa: E402

# Open the DXF file.
t0 = time.monotonic()
doc = FreeCAD.newDocument("Bench")
importDXF.insert(path, doc.Name)
open_ms = (time.monotonic() - t0) * 1000.0

count = len(doc.Objects)

# Save as DXF.
out_path = os.path.splitext(path)[0] + "-out.dxf"
t1 = time.monotonic()
importDXF.export(doc.Objects, out_path)
save_ms = (time.monotonic() - t1) * 1000.0

print(f"FreeCAD results for {path}:")
print(f"  objects:    {count}")
print(f"  open:       {open_ms:.0f} ms")
print(f"  select all: - (no headless select-all)")
print(f"  save DXF:   {save_ms:.0f} ms")

FreeCAD.closeDocument(doc.Name)
