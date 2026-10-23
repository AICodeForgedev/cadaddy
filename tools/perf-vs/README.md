# perf-vs: competitor timing scripts

Scripts that time QCAD and FreeCAD on the same plan files CADaddy uses in
`cargo xtask perf --vs` (see `docs/perf-vs.md`).

## Generating the plan files

```
cargo xtask perf --vs --sizes 10000,100000,1000000
```

This writes `target/perf/vs/plan-10000.dxf`, `plan-100000.dxf` and
`plan-1000000.dxf`.

## QCAD

Requires QCAD Professional (the Homebrew cask `qcad` or from qcad.org).

```
qcad -exec tools/perf-vs/qcad-bench.js -- target/perf/vs/plan-10000.dxf
qcad -exec tools/perf-vs/qcad-bench.js -- target/perf/vs/plan-100000.dxf
qcad -exec tools/perf-vs/qcad-bench.js -- target/perf/vs/plan-1000000.dxf
```

## FreeCAD

Requires FreeCAD with its command-line interpreter `freecadcmd` (the Homebrew
cask `freecad` or from freecad.org).

```
freecadcmd tools/perf-vs/freecad-bench.py target/perf/vs/plan-10000.dxf
freecadcmd tools/perf-vs/freecad-bench.py target/perf/vs/plan-100000.dxf
freecadcmd tools/perf-vs/freecad-bench.py target/perf/vs/plan-1000000.dxf
```

FreeCAD converts each DXF entity into a 3D shape, so its open times are much
slower than 2D programs.  There is no headless select-all.

## Peak memory

Wrap any of the above with `/usr/bin/time -l` (macOS) or `/usr/bin/time -v`
(Linux) to get the peak resident memory:

```
/usr/bin/time -l qcad -exec tools/perf-vs/qcad-bench.js -- target/perf/vs/plan-10000.dxf
```
