# Performance against other CAD programs

How CADaddy compares with QCAD and FreeCAD on the same large drawings, on the same Mac. The
drawings are made up: `cargo xtask perf --vs` writes them, as DXF 2018 grids of lines, circles,
arcs, closed polylines with an arc segment, and text on ten layers, with 10 000, 100 000 and
1 000 000 objects (1.6 MB, 16 MB and 161 MB). Each program opens each file in a fresh process.

Machine: Apple M5, 16 GB, macOS 26.7. Programs: CADaddy (this tree, release build), QCAD 3.33.1
(the Professional trial the Homebrew cask installs), FreeCAD 1.1.4.

## Results

Times are in milliseconds, measured inside each program (CADaddy: `perfbench --file`; QCAD: a
script using its documented `RDocumentInterface` API; FreeCAD: `freecadcmd` with `importDXF`).
Peak memory is the whole process's maximum resident size from `/usr/bin/time -l`, startup
included. CADaddy's numbers are the range of three runs, the others' the best of one or two. Every
FreeCAD run was stopped after 10 minutes.

| objects | program | open | select all | save DXF | peak memory |
|---|---|---|---|---|---|
| 10 000 | **CADaddy** | 25–42 | 2 | 9–15 | 57–61 MB |
| 10 000 | QCAD | 155 | 5 | 250 | 214 MB |
| 10 000 | FreeCAD | 85 231 | – | 31 | 567 MB |
| 100 000 | **CADaddy** | 284–306 | 9 | 123–141 | 340–378 MB |
| 100 000 | QCAD | 1 162 | 75 | 1 357 | 638 MB |
| 100 000 | FreeCAD | over 10 min | – | – | 1.7 GB when stopped |
| 1 000 000 | **CADaddy** | 2 341–3 262 | 108–166 | 1 295–1 662 | 2.9–3.7 GB |
| 1 000 000 | QCAD | 12 864–13 258 | 1 037–1 599 | 13 474–13 554 | 5.1–5.4 GB |
| 1 000 000 | FreeCAD | over 10 min | – | – | 3.8 GB when stopped |

"Open" for CADaddy is reading the file and building its display list. For QCAD it is its import
into a document. For FreeCAD it is `importDXF.insert`, which turns each object into a 3D shape.
FreeCAD has no headless select-all. Its save time is for the objects it had made.

## Panning

CADaddy pans on the GPU without rebuilding anything. At a million objects (2.3 million display
primitives, 225 MB of GPU buffers), each pan step costs about 0.4 ms of CPU, and the display
list is not rebuilt. The GPU's own draw time per frame is not reported yet, so this isn't a
frames-per-second figure.

QCAD and FreeCAD can only be panned by hand, in their windows. That part of the comparison
(frames per second with a stopwatch, by eye) has not been done. LibreCAD wasn't measured: its
Homebrew cask fails the macOS Gatekeeper check.

## Where the memory goes

At a million objects CADaddy's peak is made of three parts:

- 0.7 GB for the file's 12.5 million DXF groups (40 bytes each plus their text) while reading;
- about 2.0 GB at the end of reading;
- about 1 GB more for the display list.

These are measured with `PERF_STAGE=tags|read|open` on `perfbench --file`, each stage in its own
process. Reading DXF a section at a time, without holding every group, is the largest saving
available.

## Running it

- `cargo xtask perf --vs [--sizes 10000,100000,1000000]` writes the plans to `target/perf/vs/`
  and prints CADaddy's rows.
- The other programs are timed with their own tools, as above. Use the same files and one fresh
  process per file.
