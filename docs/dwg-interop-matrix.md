# DWG interop matrix

CADaddy's DWG files opened black-box by other programs, saved again, and read back into CADaddy.
Run on **2026-10-23** at `next` 0ba3838d plus this branch's STYLE fix (below), on Linux x86-64.

| Program (folder in the run) | Version | What it does with each file |
|---|---|---|
| ODA File Converter (`oda-dwg`, `oda-dxf`) | 27.9.0.0 (Linux, Qt6 build) | opens the DWG and saves it as DWG R2018 and as DXF R2018; audit off, so a defect is reported, not repaired |
| LibreCAD (`librecad`) | 2.2.0.2 (the distribution's package) | LibreCAD's DWG reader is its bundled libdxfrw. LibreCAD has no batch mode for DWG, so that bundled library, built from the 2.2.0.2 release sources with libdxfrw's own `dwg2dxf` tool, opens the DWG and writes DXF R2010 |
| LibreCAD control (`librecad-via-oda2007`) | same reader | the same reader on the ODA converter's R2007 re-save of each of our files: what the reader loses from a file that isn't ours |
| libdxfrw development build (`libdxfrw`) | libdxfrw `master` d68fb1e (2026-10-12), not in any LibreCAD release yet | its `dwg2dxf` opens the DWG and writes DXF |
| FreeCAD | 1.1 | no column; see "FreeCAD" below |

Drawings (all made up, written by `interop_write`): `kitchen` (one of each object and table type CADaddy
writes), `floorplan` and `bracket` (the built-in samples). Writers: `native` (CADaddy's own codec) and
`bridge` (the acadrust bridge, the default DWG writer). Versions R2000, R2004, R2007, R2010, R2013 and R2018.
That makes 36 files, and each one is a row per program.

The result column holds `same` when CADaddy reads the program's copy back with the same objects by
space and type, layers, named blocks (with their object counts), anonymous-block count, layouts,
every text string and every dimension (type and measurement) as from its own file. Otherwise it
lists the changes. `REJECTED` holds the program's message.

Rebuild: `scripts/dwg-interop.sh <empty folder>` (needs the programs; see the script's header), then
replace the matrix below with the folder's `matrix.md`. The harness is
`crates/engine/tests/dwginterop.rs`; the fixes it found are pinned in `crates/io/tests/dwg_interop.rs`.

## Summary

| Program | same | changed | rejected |
|---|---|---|---|
| ODA File Converter, DWG out | 33 | 3 (bridge kitchen R2000–R2007: table title) | 0 |
| ODA File Converter, DXF out | 33 | 3 (same) | 0 (12 before this run's fix) |
| LibreCAD 2.2.0.2 reader | 0 | 27 (all reader-side, see below) | 9 (all R2018; native R2007) |
| libdxfrw development build | 0 | 0 | 36 |

## Fixed in this run

- **STYLE record bit order.** Before this run the ODA converter rejected DXF output for all 12 native
  `bracket` and `floorplan` files: `OdError thrown during writeFile … Invalid Symbol Table name`. The
  linetype shape file's (unnamed) STYLE record came out of the ODA converter as a vertical font
  (70 = 68) instead of a shape file (70 = 1). The bridge's identical record read as 1. The native
  codec wrote the record's two flag bits in the order vertical, shape file. The converter and the
  bridge use shape file, then vertical. Both the writer and CADaddy's reader now use that order.
  Test: `style_records_store_the_shape_file_bit_before_vertical` (`crates/io/tests/dwg_interop.rs`)
  pins the two raw bits of a shape-file record, a vertical style and Standard in every version.

## Known losses (not CADaddy's writer, or not fixed here)

Every non-`same` row in the matrix falls under one of these:

1. **ODA, bridge `kitchen` R2000/R2004/R2007: `text "Schedule": 1→0`.** The converter reads the
   bridge's pre-R2010 TABLE without its title cell's text. "Schedule" stays in the table's display
   block, and the cells `a`, `b`, `c`, `1.5`, `H1`, `H2` survive. The native writer's table keeps
   the title in the same versions (`same` rows), and the bridge's R2010+ table keeps it. Not fixed
   here: the bridge (acadrust) encodes the TABLE, and a fixup would have to re-encode the entity.
   Workaround: R2010 or later, or the native writer (`CADADDY_DWG_WRITE=native`).
2. **LibreCAD, every R2018 file (both writers): `Error reading file`.** LibreCAD 2.2.0.2's reader
   reads no R2018 DWG. It rejects the ODA converter's own R2018 copies of the same drawings too.
3. **LibreCAD, native R2007 (3 files): `Error reading file`.** The bridge's R2007 files and the ODA
   converter's R2007 copies of the native files open. The native codec stores R2007 pages
   uncompressed (compressed size = uncompressed size, which the format allows and the ODA converter
   reads). This reader's debug output fails while decompressing the page map
   (`decompress21 => sourceOffset > dstIndex`), so it doesn't take the uncompressed form. Not fixed
   here: it needs an R2007 (LZ21) compressor in the native writer. Workaround: the bridge writer
   (the default) or another version.
4. **LibreCAD, the 27 files it opens: dimensions, hatches, layouts other than Layout1, 3D and newer
   object types, table and tolerance text.** These are the reader's losses. The control column
   (`librecad-via-oda2007`) runs the same reader on the ODA converter's re-save. On each of the 27
   rows the reader opens, the control lists the same changes, apart from two points. (a) The anonymous-block count differs,
   because the converter rebuilds dimension blocks. Those blocks only hold the dimensions the reader
   drops anyway. (b) In `kitchen-2000` one circle moves from block `TAG` to model space
   (`block TAG: 2→0`, `count model:Circle: 1→2`). The same reader does the same with the ODA
   converter's R2000 copy (checked separately), so that is its R2000 behaviour. Hatches reach its
   DXF with empty boundaries, from our file and from the converter's file alike, and CADaddy drops
   an empty hatch.
5. **libdxfrw development build, all 36 files.** Mostly `BLOCK/ENDBLK owner does not match
   BLOCK_RECORD`, plus `format 16/17 error 6` (native R2010/R2013) and `Entity type 34 has failed`
   (native `kitchen` R2000/R2004, a VIEWPORT). The ODA converter's re-saves of the same drawings pass.
   This is unreleased code with checks LibreCAD 2.2.0.2 doesn't have. The released reader opens all
   of these files except the rows in 2 and 3. Watch it: if a LibreCAD release ships these checks,
   every DWG CADaddy writes stops opening there. It is listed for the next DWG writer task, not fixed
   here.

## FreeCAD

FreeCAD 1.1 has no DWG reader of its own. It converts DWG to DXF with an external program, picked
in its DWG import preferences (LibreDWG's `dwg2dxf`, the ODA File Converter, or QCAD), and imports
that DXF. Its DWG route with the ODA converter therefore gets the `oda-dxf` column's file. Its
LibreDWG route isn't in the matrix: LibreDWG has no package for this distribution and wasn't built
for the run. FreeCAD's own DXF importer turns the file into FreeCAD shapes, which this
CADaddy-to-CADaddy comparison can't measure, so it has no column of its own.

## The matrix

| file | program | output | result | changes against CADaddy's reading of its own file |
|---|---|---|---|---|
| native/bracket-2000 | librecad | dxf | 10 changes | count model:Dimension: 6→0; count model:Hatch: 4→0; layout -Layout2; text "4X <>": 1→0; dim "diameter" 3.0000: 1→0; dim "radius" 0.3125: 1→0; dim {"linear":{"rotation":0.0}} 2.0000: 1→0; dim {"linear":{"rotation":0.0}} 6.0000: 1→0; dim {"linear":{"rotation":0.0}} 8.0000: 1→0; dim {"linear":{"rotation":1.5707963267948966}} 5.0000: 1→0 |
| native/bracket-2000 | oda-dwg | dwg | same |  |
| native/bracket-2000 | libdxfrw | - | REJECTED: BLOCK/ENDBLK owner does not match BLOCK_RECORD DWG file error: format 13 error 10 | |
| native/bracket-2000 | oda-dxf | dxf | same |  |
| native/bracket-2000 | librecad-via-oda2007 | dxf | 10 changes | count model:Dimension: 6→0; count model:Hatch: 4→0; layout -Layout2; text "4X <>": 1→0; dim "diameter" 3.0000: 1→0; dim "radius" 0.3125: 1→0; dim {"linear":{"rotation":0.0}} 2.0000: 1→0; dim {"linear":{"rotation":0.0}} 6.0000: 1→0; dim {"linear":{"rotation":0.0}} 8.0000: 1→0; dim {"linear":{"rotation":1.5707963267948966}} 5.0000: 1→0 |
| native/bracket-2004 | librecad | dxf | 10 changes | count model:Dimension: 6→0; count model:Hatch: 4→0; layout -Layout2; text "4X <>": 1→0; dim "diameter" 3.0000: 1→0; dim "radius" 0.3125: 1→0; dim {"linear":{"rotation":0.0}} 2.0000: 1→0; dim {"linear":{"rotation":0.0}} 6.0000: 1→0; dim {"linear":{"rotation":0.0}} 8.0000: 1→0; dim {"linear":{"rotation":1.5707963267948966}} 5.0000: 1→0 |
| native/bracket-2004 | oda-dwg | dwg | same |  |
| native/bracket-2004 | libdxfrw | - | REJECTED: BLOCK/ENDBLK owner does not match BLOCK_RECORD DWG file error: format 14 error 10 | |
| native/bracket-2004 | oda-dxf | dxf | same |  |
| native/bracket-2004 | librecad-via-oda2007 | dxf | 10 changes | count model:Dimension: 6→0; count model:Hatch: 4→0; layout -Layout2; text "4X <>": 1→0; dim "diameter" 3.0000: 1→0; dim "radius" 0.3125: 1→0; dim {"linear":{"rotation":0.0}} 2.0000: 1→0; dim {"linear":{"rotation":0.0}} 6.0000: 1→0; dim {"linear":{"rotation":0.0}} 8.0000: 1→0; dim {"linear":{"rotation":1.5707963267948966}} 5.0000: 1→0 |
| native/bracket-2007 | librecad | - | REJECTED: Error reading file | |
| native/bracket-2007 | oda-dwg | dwg | same |  |
| native/bracket-2007 | libdxfrw | - | REJECTED: BLOCK/ENDBLK owner does not match BLOCK_RECORD DWG file error: format 15 error 10 | |
| native/bracket-2007 | oda-dxf | dxf | same |  |
| native/bracket-2007 | librecad-via-oda2007 | dxf | 10 changes | count model:Dimension: 6→0; count model:Hatch: 4→0; layout -Layout2; text "4X <>": 1→0; dim "diameter" 3.0000: 1→0; dim "radius" 0.3125: 1→0; dim {"linear":{"rotation":0.0}} 2.0000: 1→0; dim {"linear":{"rotation":0.0}} 6.0000: 1→0; dim {"linear":{"rotation":0.0}} 8.0000: 1→0; dim {"linear":{"rotation":1.5707963267948966}} 5.0000: 1→0 |
| native/bracket-2010 | librecad | dxf | 10 changes | count model:Dimension: 6→0; count model:Hatch: 4→0; layout -Layout2; text "4X <>": 1→0; dim "diameter" 3.0000: 1→0; dim "radius" 0.3125: 1→0; dim {"linear":{"rotation":0.0}} 2.0000: 1→0; dim {"linear":{"rotation":0.0}} 6.0000: 1→0; dim {"linear":{"rotation":0.0}} 8.0000: 1→0; dim {"linear":{"rotation":1.5707963267948966}} 5.0000: 1→0 |
| native/bracket-2010 | oda-dwg | dwg | same |  |
| native/bracket-2010 | libdxfrw | - | REJECTED: DWG file error: format 16 error 6 | |
| native/bracket-2010 | oda-dxf | dxf | same |  |
| native/bracket-2010 | librecad-via-oda2007 | dxf | 10 changes | count model:Dimension: 6→0; count model:Hatch: 4→0; layout -Layout2; text "4X <>": 1→0; dim "diameter" 3.0000: 1→0; dim "radius" 0.3125: 1→0; dim {"linear":{"rotation":0.0}} 2.0000: 1→0; dim {"linear":{"rotation":0.0}} 6.0000: 1→0; dim {"linear":{"rotation":0.0}} 8.0000: 1→0; dim {"linear":{"rotation":1.5707963267948966}} 5.0000: 1→0 |
| native/bracket-2013 | librecad | dxf | 10 changes | count model:Dimension: 6→0; count model:Hatch: 4→0; layout -Layout2; text "4X <>": 1→0; dim "diameter" 3.0000: 1→0; dim "radius" 0.3125: 1→0; dim {"linear":{"rotation":0.0}} 2.0000: 1→0; dim {"linear":{"rotation":0.0}} 6.0000: 1→0; dim {"linear":{"rotation":0.0}} 8.0000: 1→0; dim {"linear":{"rotation":1.5707963267948966}} 5.0000: 1→0 |
| native/bracket-2013 | oda-dwg | dwg | same |  |
| native/bracket-2013 | libdxfrw | - | REJECTED: DWG file error: format 17 error 6 | |
| native/bracket-2013 | oda-dxf | dxf | same |  |
| native/bracket-2013 | librecad-via-oda2007 | dxf | 10 changes | count model:Dimension: 6→0; count model:Hatch: 4→0; layout -Layout2; text "4X <>": 1→0; dim "diameter" 3.0000: 1→0; dim "radius" 0.3125: 1→0; dim {"linear":{"rotation":0.0}} 2.0000: 1→0; dim {"linear":{"rotation":0.0}} 6.0000: 1→0; dim {"linear":{"rotation":0.0}} 8.0000: 1→0; dim {"linear":{"rotation":1.5707963267948966}} 5.0000: 1→0 |
| native/bracket-2018 | librecad | - | REJECTED: Error reading file | |
| native/bracket-2018 | oda-dwg | dwg | same |  |
| native/bracket-2018 | libdxfrw | - | REJECTED: BLOCK/ENDBLK owner does not match BLOCK_RECORD DWG file error: format 18 error 10 | |
| native/bracket-2018 | oda-dxf | dxf | same |  |
| native/bracket-2018 | librecad-via-oda2007 | dxf | 10 changes | count model:Dimension: 6→0; count model:Hatch: 4→0; layout -Layout2; text "4X <>": 1→0; dim "diameter" 3.0000: 1→0; dim "radius" 0.3125: 1→0; dim {"linear":{"rotation":0.0}} 2.0000: 1→0; dim {"linear":{"rotation":0.0}} 6.0000: 1→0; dim {"linear":{"rotation":0.0}} 8.0000: 1→0; dim {"linear":{"rotation":1.5707963267948966}} 5.0000: 1→0 |
| native/floorplan-2000 | librecad | dxf | 3 changes | count model:Dimension: 1→0; layout -Layout2; dim {"linear":{"rotation":0.0}} 360.0000: 1→0 |
| native/floorplan-2000 | oda-dwg | dwg | same |  |
| native/floorplan-2000 | libdxfrw | - | REJECTED: BLOCK/ENDBLK owner does not match BLOCK_RECORD DWG file error: format 13 error 10 | |
| native/floorplan-2000 | oda-dxf | dxf | same |  |
| native/floorplan-2000 | librecad-via-oda2007 | dxf | 3 changes | count model:Dimension: 1→0; layout -Layout2; dim {"linear":{"rotation":0.0}} 360.0000: 1→0 |
| native/floorplan-2004 | librecad | dxf | 3 changes | count model:Dimension: 1→0; layout -Layout2; dim {"linear":{"rotation":0.0}} 360.0000: 1→0 |
| native/floorplan-2004 | oda-dwg | dwg | same |  |
| native/floorplan-2004 | libdxfrw | - | REJECTED: BLOCK/ENDBLK owner does not match BLOCK_RECORD DWG file error: format 14 error 10 | |
| native/floorplan-2004 | oda-dxf | dxf | same |  |
| native/floorplan-2004 | librecad-via-oda2007 | dxf | 3 changes | count model:Dimension: 1→0; layout -Layout2; dim {"linear":{"rotation":0.0}} 360.0000: 1→0 |
| native/floorplan-2007 | librecad | - | REJECTED: Error reading file | |
| native/floorplan-2007 | oda-dwg | dwg | same |  |
| native/floorplan-2007 | libdxfrw | - | REJECTED: BLOCK/ENDBLK owner does not match BLOCK_RECORD DWG file error: format 15 error 10 | |
| native/floorplan-2007 | oda-dxf | dxf | same |  |
| native/floorplan-2007 | librecad-via-oda2007 | dxf | 3 changes | count model:Dimension: 1→0; layout -Layout2; dim {"linear":{"rotation":0.0}} 360.0000: 1→0 |
| native/floorplan-2010 | librecad | dxf | 3 changes | count model:Dimension: 1→0; layout -Layout2; dim {"linear":{"rotation":0.0}} 360.0000: 1→0 |
| native/floorplan-2010 | oda-dwg | dwg | same |  |
| native/floorplan-2010 | libdxfrw | - | REJECTED: DWG file error: format 16 error 6 | |
| native/floorplan-2010 | oda-dxf | dxf | same |  |
| native/floorplan-2010 | librecad-via-oda2007 | dxf | 3 changes | count model:Dimension: 1→0; layout -Layout2; dim {"linear":{"rotation":0.0}} 360.0000: 1→0 |
| native/floorplan-2013 | librecad | dxf | 3 changes | count model:Dimension: 1→0; layout -Layout2; dim {"linear":{"rotation":0.0}} 360.0000: 1→0 |
| native/floorplan-2013 | oda-dwg | dwg | same |  |
| native/floorplan-2013 | libdxfrw | - | REJECTED: DWG file error: format 17 error 6 | |
| native/floorplan-2013 | oda-dxf | dxf | same |  |
| native/floorplan-2013 | librecad-via-oda2007 | dxf | 3 changes | count model:Dimension: 1→0; layout -Layout2; dim {"linear":{"rotation":0.0}} 360.0000: 1→0 |
| native/floorplan-2018 | librecad | - | REJECTED: Error reading file | |
| native/floorplan-2018 | oda-dwg | dwg | same |  |
| native/floorplan-2018 | libdxfrw | - | REJECTED: BLOCK/ENDBLK owner does not match BLOCK_RECORD DWG file error: format 18 error 10 | |
| native/floorplan-2018 | oda-dxf | dxf | same |  |
| native/floorplan-2018 | librecad-via-oda2007 | dxf | 3 changes | count model:Dimension: 1→0; layout -Layout2; dim {"linear":{"rotation":0.0}} 360.0000: 1→0 |
| native/kitchen-2000 | librecad | dxf | 40 changes | count model:3D Face: 1→0; count model:3D Solid: 1→0; count model:Circle: 1→2; count model:Dimension: 6→0; count model:Hatch: 1→0; count model:Helix: 1→0; count model:Light: 1→0; count model:Line: 1→2; count model:MLine: 1→0; count model:PDF Underlay: 1→0; count model:Polyface Mesh: 1→0; count model:Polyline: 2→3; count model:Ray: 1→0; count model:Region: 1→0; count model:Section Plane: 1→0; count model:Table: 1→0; count model:Tolerance: 1→0; count model:Trace: 1→0; count model:Wipeout: 1→0; count model:Xline: 1→0; count paper:Viewport: 2→0; block TAG: 2→0; anonymous blocks: 6→7; layout -Layout2; layout -Sheet A; text "1.5": 1→0; text "204": 1→0; text "H1": 1→0; text "H2": 1→0; text "Schedule": 1→0; text "a": 1→0; text "b": 1→0; text "c": 1→0; text "{\\Fgdt;j}%%v{\\Fgdt;n}0.1{\\Fgdt;m}%%v%%vA%%vB": 1→0; dim "aligned" 14.1421: 1→0; dim "angular3P" 1.1071: 1→0; dim "diameter" 8.0000: 1→0; dim "radius" 4.0000: 1→0; dim {"linear":{"rotation":0.0}} 20.0000: 1→0; dim {"ordinate":{"x_type":false}} 80.0000: 1→0 |
| native/kitchen-2000 | oda-dwg | dwg | same |  |
| native/kitchen-2000 | libdxfrw | - | REJECTED: Warning: Entity type 34has failed, handle: 4511 DWG file error: format 13 error 10 | |
| native/kitchen-2000 | oda-dxf | dxf | same |  |
| native/kitchen-2000 | librecad-via-oda2007 | dxf | 38 changes | count model:3D Face: 1→0; count model:3D Solid: 1→0; count model:Dimension: 6→0; count model:Hatch: 1→0; count model:Helix: 1→0; count model:Light: 1→0; count model:Line: 1→2; count model:MLine: 1→0; count model:PDF Underlay: 1→0; count model:Polyface Mesh: 1→0; count model:Polyline: 2→3; count model:Ray: 1→0; count model:Region: 1→0; count model:Section Plane: 1→0; count model:Table: 1→0; count model:Tolerance: 1→0; count model:Trace: 1→0; count model:Wipeout: 1→0; count model:Xline: 1→0; count paper:Viewport: 2→0; block TAG: 2→1; layout -Layout2; layout -Sheet A; text "1.5": 1→0; text "204": 1→0; text "H1": 1→0; text "H2": 1→0; text "Schedule": 1→0; text "a": 1→0; text "b": 1→0; text "c": 1→0; text "{\\Fgdt;j}%%v{\\Fgdt;n}0.1{\\Fgdt;m}%%v%%vA%%vB": 1→0; dim "aligned" 14.1421: 1→0; dim "angular3P" 1.1071: 1→0; dim "diameter" 8.0000: 1→0; dim "radius" 4.0000: 1→0; dim {"linear":{"rotation":0.0}} 20.0000: 1→0; dim {"ordinate":{"x_type":false}} 80.0000: 1→0 |
| native/kitchen-2004 | librecad | dxf | 39 changes | count model:3D Face: 1→0; count model:3D Solid: 1→0; count model:Dimension: 6→0; count model:Hatch: 1→0; count model:Helix: 1→0; count model:Light: 1→0; count model:Line: 1→2; count model:MLine: 1→0; count model:PDF Underlay: 1→0; count model:Polyface Mesh: 1→0; count model:Polyline: 2→3; count model:Ray: 1→0; count model:Region: 1→0; count model:Section Plane: 1→0; count model:Table: 1→0; count model:Tolerance: 1→0; count model:Trace: 1→0; count model:Wipeout: 1→0; count model:Xline: 1→0; count paper:Viewport: 2→0; block TAG: 2→1; anonymous blocks: 6→7; layout -Layout2; layout -Sheet A; text "1.5": 1→0; text "204": 1→0; text "H1": 1→0; text "H2": 1→0; text "Schedule": 1→0; text "a": 1→0; text "b": 1→0; text "c": 1→0; text "{\\Fgdt;j}%%v{\\Fgdt;n}0.1{\\Fgdt;m}%%v%%vA%%vB": 1→0; dim "aligned" 14.1421: 1→0; dim "angular3P" 1.1071: 1→0; dim "diameter" 8.0000: 1→0; dim "radius" 4.0000: 1→0; dim {"linear":{"rotation":0.0}} 20.0000: 1→0; dim {"ordinate":{"x_type":false}} 80.0000: 1→0 |
| native/kitchen-2004 | oda-dwg | dwg | same |  |
| native/kitchen-2004 | libdxfrw | - | REJECTED: Warning: Entity type 34has failed, handle: 297 DWG file error: format 14 error 10 | |
| native/kitchen-2004 | oda-dxf | dxf | same |  |
| native/kitchen-2004 | librecad-via-oda2007 | dxf | 38 changes | count model:3D Face: 1→0; count model:3D Solid: 1→0; count model:Dimension: 6→0; count model:Hatch: 1→0; count model:Helix: 1→0; count model:Light: 1→0; count model:Line: 1→2; count model:MLine: 1→0; count model:PDF Underlay: 1→0; count model:Polyface Mesh: 1→0; count model:Polyline: 2→3; count model:Ray: 1→0; count model:Region: 1→0; count model:Section Plane: 1→0; count model:Table: 1→0; count model:Tolerance: 1→0; count model:Trace: 1→0; count model:Wipeout: 1→0; count model:Xline: 1→0; count paper:Viewport: 2→0; block TAG: 2→1; layout -Layout2; layout -Sheet A; text "1.5": 1→0; text "204": 1→0; text "H1": 1→0; text "H2": 1→0; text "Schedule": 1→0; text "a": 1→0; text "b": 1→0; text "c": 1→0; text "{\\Fgdt;j}%%v{\\Fgdt;n}0.1{\\Fgdt;m}%%v%%vA%%vB": 1→0; dim "aligned" 14.1421: 1→0; dim "angular3P" 1.1071: 1→0; dim "diameter" 8.0000: 1→0; dim "radius" 4.0000: 1→0; dim {"linear":{"rotation":0.0}} 20.0000: 1→0; dim {"ordinate":{"x_type":false}} 80.0000: 1→0 |
| native/kitchen-2007 | librecad | - | REJECTED: Error reading file | |
| native/kitchen-2007 | oda-dwg | dwg | same |  |
| native/kitchen-2007 | libdxfrw | - | REJECTED: BLOCK/ENDBLK owner does not match BLOCK_RECORD DWG file error: format 15 error 10 | |
| native/kitchen-2007 | oda-dxf | dxf | same |  |
| native/kitchen-2007 | librecad-via-oda2007 | dxf | 40 changes | count model:3D Face: 1→0; count model:3D Solid: 1→0; count model:Dimension: 6→0; count model:Hatch: 1→0; count model:Helix: 1→0; count model:Light: 1→0; count model:Line: 1→2; count model:MLine: 1→0; count model:Multileader: 1→0; count model:PDF Underlay: 1→0; count model:Polyface Mesh: 1→0; count model:Polyline: 2→3; count model:Ray: 1→0; count model:Region: 1→0; count model:Section Plane: 1→0; count model:Table: 1→0; count model:Tolerance: 1→0; count model:Trace: 1→0; count model:Wipeout: 1→0; count model:Xline: 1→0; count paper:Viewport: 2→0; block TAG: 2→1; layout -Layout2; layout -Sheet A; text "1.5": 1→0; text "204": 1→0; text "Callout": 1→0; text "H1": 1→0; text "H2": 1→0; text "Schedule": 1→0; text "a": 1→0; text "b": 1→0; text "c": 1→0; text "{\\Fgdt;j}%%v{\\Fgdt;n}0.1{\\Fgdt;m}%%v%%vA%%vB": 1→0; dim "aligned" 14.1421: 1→0; dim "angular3P" 1.1071: 1→0; dim "diameter" 8.0000: 1→0; dim "radius" 4.0000: 1→0; dim {"linear":{"rotation":0.0}} 20.0000: 1→0; dim {"ordinate":{"x_type":false}} 80.0000: 1→0 |
| native/kitchen-2010 | librecad | dxf | 41 changes | count model:3D Face: 1→0; count model:3D Solid: 1→0; count model:Dimension: 6→0; count model:Hatch: 1→0; count model:Helix: 1→0; count model:Light: 1→0; count model:Line: 1→2; count model:MLine: 1→0; count model:Mesh: 1→0; count model:Multileader: 1→0; count model:PDF Underlay: 1→0; count model:Polyline: 2→3; count model:Ray: 1→0; count model:Region: 1→0; count model:Section Plane: 1→0; count model:Table: 1→0; count model:Tolerance: 1→0; count model:Trace: 1→0; count model:Wipeout: 1→0; count model:Xline: 1→0; count paper:Viewport: 2→0; block TAG: 2→1; anonymous blocks: 6→7; layout -Layout2; layout -Sheet A; text "1.5": 1→0; text "204": 1→0; text "Callout": 1→0; text "H1": 1→0; text "H2": 1→0; text "Schedule": 1→0; text "a": 1→0; text "b": 1→0; text "c": 1→0; text "{\\Fgdt;j}%%v{\\Fgdt;n}0.1{\\Fgdt;m}%%v%%vA%%vB": 1→0; dim "aligned" 14.1421: 1→0; dim "angular3P" 1.1071: 1→0; dim "diameter" 8.0000: 1→0; dim "radius" 4.0000: 1→0; dim {"linear":{"rotation":0.0}} 20.0000: 1→0; dim {"ordinate":{"x_type":false}} 80.0000: 1→0 |
| native/kitchen-2010 | oda-dwg | dwg | same |  |
| native/kitchen-2010 | libdxfrw | - | REJECTED: DWG file error: format 16 error 6 | |
| native/kitchen-2010 | oda-dxf | dxf | same |  |
| native/kitchen-2010 | librecad-via-oda2007 | dxf | 40 changes | count model:3D Face: 1→0; count model:3D Solid: 1→0; count model:Dimension: 6→0; count model:Hatch: 1→0; count model:Helix: 1→0; count model:Light: 1→0; count model:Line: 1→2; count model:MLine: 1→0; count model:Mesh: 1→0; count model:Multileader: 1→0; count model:PDF Underlay: 1→0; count model:Polyline: 2→3; count model:Ray: 1→0; count model:Region: 1→0; count model:Section Plane: 1→0; count model:Table: 1→0; count model:Tolerance: 1→0; count model:Trace: 1→0; count model:Wipeout: 1→0; count model:Xline: 1→0; count paper:Viewport: 2→0; block TAG: 2→1; layout -Layout2; layout -Sheet A; text "1.5": 1→0; text "204": 1→0; text "Callout": 1→0; text "H1": 1→0; text "H2": 1→0; text "Schedule": 1→0; text "a": 1→0; text "b": 1→0; text "c": 1→0; text "{\\Fgdt;j}%%v{\\Fgdt;n}0.1{\\Fgdt;m}%%v%%vA%%vB": 1→0; dim "aligned" 14.1421: 1→0; dim "angular3P" 1.1071: 1→0; dim "diameter" 8.0000: 1→0; dim "radius" 4.0000: 1→0; dim {"linear":{"rotation":0.0}} 20.0000: 1→0; dim {"ordinate":{"x_type":false}} 80.0000: 1→0 |
| native/kitchen-2013 | librecad | dxf | 41 changes | count model:3D Face: 1→0; count model:3D Solid: 1→0; count model:Dimension: 6→0; count model:Hatch: 1→0; count model:Helix: 1→0; count model:Light: 1→0; count model:Line: 1→2; count model:MLine: 1→0; count model:Mesh: 1→0; count model:Multileader: 1→0; count model:PDF Underlay: 1→0; count model:Polyline: 2→3; count model:Ray: 1→0; count model:Region: 1→0; count model:Section Plane: 1→0; count model:Table: 1→0; count model:Tolerance: 1→0; count model:Trace: 1→0; count model:Wipeout: 1→0; count model:Xline: 1→0; count paper:Viewport: 2→0; block TAG: 2→1; anonymous blocks: 6→7; layout -Layout2; layout -Sheet A; text "1.5": 1→0; text "204": 1→0; text "Callout": 1→0; text "H1": 1→0; text "H2": 1→0; text "Schedule": 1→0; text "a": 1→0; text "b": 1→0; text "c": 1→0; text "{\\Fgdt;j}%%v{\\Fgdt;n}0.1{\\Fgdt;m}%%v%%vA%%vB": 1→0; dim "aligned" 14.1421: 1→0; dim "angular3P" 1.1071: 1→0; dim "diameter" 8.0000: 1→0; dim "radius" 4.0000: 1→0; dim {"linear":{"rotation":0.0}} 20.0000: 1→0; dim {"ordinate":{"x_type":false}} 80.0000: 1→0 |
| native/kitchen-2013 | oda-dwg | dwg | same |  |
| native/kitchen-2013 | libdxfrw | - | REJECTED: DWG file error: format 17 error 6 | |
| native/kitchen-2013 | oda-dxf | dxf | same |  |
| native/kitchen-2013 | librecad-via-oda2007 | dxf | 40 changes | count model:3D Face: 1→0; count model:3D Solid: 1→0; count model:Dimension: 6→0; count model:Hatch: 1→0; count model:Helix: 1→0; count model:Light: 1→0; count model:Line: 1→2; count model:MLine: 1→0; count model:Mesh: 1→0; count model:Multileader: 1→0; count model:PDF Underlay: 1→0; count model:Polyline: 2→3; count model:Ray: 1→0; count model:Region: 1→0; count model:Section Plane: 1→0; count model:Table: 1→0; count model:Tolerance: 1→0; count model:Trace: 1→0; count model:Wipeout: 1→0; count model:Xline: 1→0; count paper:Viewport: 2→0; block TAG: 2→1; layout -Layout2; layout -Sheet A; text "1.5": 1→0; text "204": 1→0; text "Callout": 1→0; text "H1": 1→0; text "H2": 1→0; text "Schedule": 1→0; text "a": 1→0; text "b": 1→0; text "c": 1→0; text "{\\Fgdt;j}%%v{\\Fgdt;n}0.1{\\Fgdt;m}%%v%%vA%%vB": 1→0; dim "aligned" 14.1421: 1→0; dim "angular3P" 1.1071: 1→0; dim "diameter" 8.0000: 1→0; dim "radius" 4.0000: 1→0; dim {"linear":{"rotation":0.0}} 20.0000: 1→0; dim {"ordinate":{"x_type":false}} 80.0000: 1→0 |
| native/kitchen-2018 | librecad | - | REJECTED: Error reading file | |
| native/kitchen-2018 | oda-dwg | dwg | same |  |
| native/kitchen-2018 | libdxfrw | - | REJECTED: BLOCK/ENDBLK owner does not match BLOCK_RECORD DWG file error: format 18 error 10 | |
| native/kitchen-2018 | oda-dxf | dxf | same |  |
| native/kitchen-2018 | librecad-via-oda2007 | dxf | 40 changes | count model:3D Face: 1→0; count model:3D Solid: 1→0; count model:Dimension: 6→0; count model:Hatch: 1→0; count model:Helix: 1→0; count model:Light: 1→0; count model:Line: 1→2; count model:MLine: 1→0; count model:Mesh: 1→0; count model:Multileader: 1→0; count model:PDF Underlay: 1→0; count model:Polyline: 2→3; count model:Ray: 1→0; count model:Region: 1→0; count model:Section Plane: 1→0; count model:Table: 1→0; count model:Tolerance: 1→0; count model:Trace: 1→0; count model:Wipeout: 1→0; count model:Xline: 1→0; count paper:Viewport: 2→0; block TAG: 2→1; layout -Layout2; layout -Sheet A; text "1.5": 1→0; text "204": 1→0; text "Callout": 1→0; text "H1": 1→0; text "H2": 1→0; text "Schedule": 1→0; text "a": 1→0; text "b": 1→0; text "c": 1→0; text "{\\Fgdt;j}%%v{\\Fgdt;n}0.1{\\Fgdt;m}%%v%%vA%%vB": 1→0; dim "aligned" 14.1421: 1→0; dim "angular3P" 1.1071: 1→0; dim "diameter" 8.0000: 1→0; dim "radius" 4.0000: 1→0; dim {"linear":{"rotation":0.0}} 20.0000: 1→0; dim {"ordinate":{"x_type":false}} 80.0000: 1→0 |
| bridge/bracket-2000 | librecad | dxf | 11 changes | count model:Dimension: 6→0; count model:Hatch: 4→0; anonymous blocks: 6→1; layout -Layout2; text "4X <>": 1→0; dim "diameter" 3.0000: 1→0; dim "radius" 0.3125: 1→0; dim {"linear":{"rotation":0.0}} 2.0000: 1→0; dim {"linear":{"rotation":0.0}} 6.0000: 1→0; dim {"linear":{"rotation":0.0}} 8.0000: 1→0; dim {"linear":{"rotation":1.5707963267948966}} 5.0000: 1→0 |
| bridge/bracket-2000 | oda-dwg | dwg | same |  |
| bridge/bracket-2000 | libdxfrw | - | REJECTED: BLOCK/ENDBLK owner does not match BLOCK_RECORD DWG file error: format 13 error 10 | |
| bridge/bracket-2000 | oda-dxf | dxf | same |  |
| bridge/bracket-2000 | librecad-via-oda2007 | dxf | 10 changes | count model:Dimension: 6→0; count model:Hatch: 4→0; layout -Layout2; text "4X <>": 1→0; dim "diameter" 3.0000: 1→0; dim "radius" 0.3125: 1→0; dim {"linear":{"rotation":0.0}} 2.0000: 1→0; dim {"linear":{"rotation":0.0}} 6.0000: 1→0; dim {"linear":{"rotation":0.0}} 8.0000: 1→0; dim {"linear":{"rotation":1.5707963267948966}} 5.0000: 1→0 |
| bridge/bracket-2004 | librecad | dxf | 11 changes | count model:Dimension: 6→0; count model:Hatch: 4→0; anonymous blocks: 6→1; layout -Layout2; text "4X <>": 1→0; dim "diameter" 3.0000: 1→0; dim "radius" 0.3125: 1→0; dim {"linear":{"rotation":0.0}} 2.0000: 1→0; dim {"linear":{"rotation":0.0}} 6.0000: 1→0; dim {"linear":{"rotation":0.0}} 8.0000: 1→0; dim {"linear":{"rotation":1.5707963267948966}} 5.0000: 1→0 |
| bridge/bracket-2004 | oda-dwg | dwg | same |  |
| bridge/bracket-2004 | libdxfrw | - | REJECTED: BLOCK/ENDBLK owner does not match BLOCK_RECORD DWG file error: format 14 error 10 | |
| bridge/bracket-2004 | oda-dxf | dxf | same |  |
| bridge/bracket-2004 | librecad-via-oda2007 | dxf | 10 changes | count model:Dimension: 6→0; count model:Hatch: 4→0; layout -Layout2; text "4X <>": 1→0; dim "diameter" 3.0000: 1→0; dim "radius" 0.3125: 1→0; dim {"linear":{"rotation":0.0}} 2.0000: 1→0; dim {"linear":{"rotation":0.0}} 6.0000: 1→0; dim {"linear":{"rotation":0.0}} 8.0000: 1→0; dim {"linear":{"rotation":1.5707963267948966}} 5.0000: 1→0 |
| bridge/bracket-2007 | librecad | dxf | 11 changes | count model:Dimension: 6→0; count model:Hatch: 4→0; anonymous blocks: 6→1; layout -Layout2; text "4X <>": 1→0; dim "diameter" 3.0000: 1→0; dim "radius" 0.3125: 1→0; dim {"linear":{"rotation":0.0}} 2.0000: 1→0; dim {"linear":{"rotation":0.0}} 6.0000: 1→0; dim {"linear":{"rotation":0.0}} 8.0000: 1→0; dim {"linear":{"rotation":1.5707963267948966}} 5.0000: 1→0 |
| bridge/bracket-2007 | oda-dwg | dwg | same |  |
| bridge/bracket-2007 | libdxfrw | - | REJECTED: BLOCK/ENDBLK owner does not match BLOCK_RECORD DWG file error: format 15 error 10 | |
| bridge/bracket-2007 | oda-dxf | dxf | same |  |
| bridge/bracket-2007 | librecad-via-oda2007 | dxf | 10 changes | count model:Dimension: 6→0; count model:Hatch: 4→0; layout -Layout2; text "4X <>": 1→0; dim "diameter" 3.0000: 1→0; dim "radius" 0.3125: 1→0; dim {"linear":{"rotation":0.0}} 2.0000: 1→0; dim {"linear":{"rotation":0.0}} 6.0000: 1→0; dim {"linear":{"rotation":0.0}} 8.0000: 1→0; dim {"linear":{"rotation":1.5707963267948966}} 5.0000: 1→0 |
| bridge/bracket-2010 | librecad | dxf | 11 changes | count model:Dimension: 6→0; count model:Hatch: 4→0; anonymous blocks: 6→1; layout -Layout2; text "4X <>": 1→0; dim "diameter" 3.0000: 1→0; dim "radius" 0.3125: 1→0; dim {"linear":{"rotation":0.0}} 2.0000: 1→0; dim {"linear":{"rotation":0.0}} 6.0000: 1→0; dim {"linear":{"rotation":0.0}} 8.0000: 1→0; dim {"linear":{"rotation":1.5707963267948966}} 5.0000: 1→0 |
| bridge/bracket-2010 | oda-dwg | dwg | same |  |
| bridge/bracket-2010 | libdxfrw | - | REJECTED: BLOCK/ENDBLK owner does not match BLOCK_RECORD DWG file error: format 16 error 10 | |
| bridge/bracket-2010 | oda-dxf | dxf | same |  |
| bridge/bracket-2010 | librecad-via-oda2007 | dxf | 10 changes | count model:Dimension: 6→0; count model:Hatch: 4→0; layout -Layout2; text "4X <>": 1→0; dim "diameter" 3.0000: 1→0; dim "radius" 0.3125: 1→0; dim {"linear":{"rotation":0.0}} 2.0000: 1→0; dim {"linear":{"rotation":0.0}} 6.0000: 1→0; dim {"linear":{"rotation":0.0}} 8.0000: 1→0; dim {"linear":{"rotation":1.5707963267948966}} 5.0000: 1→0 |
| bridge/bracket-2013 | librecad | dxf | 11 changes | count model:Dimension: 6→0; count model:Hatch: 4→0; anonymous blocks: 6→1; layout -Layout2; text "4X <>": 1→0; dim "diameter" 3.0000: 1→0; dim "radius" 0.3125: 1→0; dim {"linear":{"rotation":0.0}} 2.0000: 1→0; dim {"linear":{"rotation":0.0}} 6.0000: 1→0; dim {"linear":{"rotation":0.0}} 8.0000: 1→0; dim {"linear":{"rotation":1.5707963267948966}} 5.0000: 1→0 |
| bridge/bracket-2013 | oda-dwg | dwg | same |  |
| bridge/bracket-2013 | libdxfrw | - | REJECTED: BLOCK/ENDBLK owner does not match BLOCK_RECORD DWG file error: format 17 error 10 | |
| bridge/bracket-2013 | oda-dxf | dxf | same |  |
| bridge/bracket-2013 | librecad-via-oda2007 | dxf | 10 changes | count model:Dimension: 6→0; count model:Hatch: 4→0; layout -Layout2; text "4X <>": 1→0; dim "diameter" 3.0000: 1→0; dim "radius" 0.3125: 1→0; dim {"linear":{"rotation":0.0}} 2.0000: 1→0; dim {"linear":{"rotation":0.0}} 6.0000: 1→0; dim {"linear":{"rotation":0.0}} 8.0000: 1→0; dim {"linear":{"rotation":1.5707963267948966}} 5.0000: 1→0 |
| bridge/bracket-2018 | librecad | - | REJECTED: Error reading file | |
| bridge/bracket-2018 | oda-dwg | dwg | same |  |
| bridge/bracket-2018 | libdxfrw | - | REJECTED: BLOCK/ENDBLK owner does not match BLOCK_RECORD DWG file error: format 18 error 10 | |
| bridge/bracket-2018 | oda-dxf | dxf | same |  |
| bridge/bracket-2018 | librecad-via-oda2007 | dxf | 10 changes | count model:Dimension: 6→0; count model:Hatch: 4→0; layout -Layout2; text "4X <>": 1→0; dim "diameter" 3.0000: 1→0; dim "radius" 0.3125: 1→0; dim {"linear":{"rotation":0.0}} 2.0000: 1→0; dim {"linear":{"rotation":0.0}} 6.0000: 1→0; dim {"linear":{"rotation":0.0}} 8.0000: 1→0; dim {"linear":{"rotation":1.5707963267948966}} 5.0000: 1→0 |
| bridge/floorplan-2000 | librecad | dxf | 3 changes | count model:Dimension: 1→0; layout -Layout2; dim {"linear":{"rotation":0.0}} 360.0000: 1→0 |
| bridge/floorplan-2000 | oda-dwg | dwg | same |  |
| bridge/floorplan-2000 | libdxfrw | - | REJECTED: BLOCK/ENDBLK owner does not match BLOCK_RECORD DWG file error: format 13 error 10 | |
| bridge/floorplan-2000 | oda-dxf | dxf | same |  |
| bridge/floorplan-2000 | librecad-via-oda2007 | dxf | 3 changes | count model:Dimension: 1→0; layout -Layout2; dim {"linear":{"rotation":0.0}} 360.0000: 1→0 |
| bridge/floorplan-2004 | librecad | dxf | 3 changes | count model:Dimension: 1→0; layout -Layout2; dim {"linear":{"rotation":0.0}} 360.0000: 1→0 |
| bridge/floorplan-2004 | oda-dwg | dwg | same |  |
| bridge/floorplan-2004 | libdxfrw | - | REJECTED: BLOCK/ENDBLK owner does not match BLOCK_RECORD DWG file error: format 14 error 10 | |
| bridge/floorplan-2004 | oda-dxf | dxf | same |  |
| bridge/floorplan-2004 | librecad-via-oda2007 | dxf | 3 changes | count model:Dimension: 1→0; layout -Layout2; dim {"linear":{"rotation":0.0}} 360.0000: 1→0 |
| bridge/floorplan-2007 | librecad | dxf | 3 changes | count model:Dimension: 1→0; layout -Layout2; dim {"linear":{"rotation":0.0}} 360.0000: 1→0 |
| bridge/floorplan-2007 | oda-dwg | dwg | same |  |
| bridge/floorplan-2007 | libdxfrw | - | REJECTED: BLOCK/ENDBLK owner does not match BLOCK_RECORD DWG file error: format 15 error 10 | |
| bridge/floorplan-2007 | oda-dxf | dxf | same |  |
| bridge/floorplan-2007 | librecad-via-oda2007 | dxf | 3 changes | count model:Dimension: 1→0; layout -Layout2; dim {"linear":{"rotation":0.0}} 360.0000: 1→0 |
| bridge/floorplan-2010 | librecad | dxf | 3 changes | count model:Dimension: 1→0; layout -Layout2; dim {"linear":{"rotation":0.0}} 360.0000: 1→0 |
| bridge/floorplan-2010 | oda-dwg | dwg | same |  |
| bridge/floorplan-2010 | libdxfrw | - | REJECTED: BLOCK/ENDBLK owner does not match BLOCK_RECORD DWG file error: format 16 error 10 | |
| bridge/floorplan-2010 | oda-dxf | dxf | same |  |
| bridge/floorplan-2010 | librecad-via-oda2007 | dxf | 3 changes | count model:Dimension: 1→0; layout -Layout2; dim {"linear":{"rotation":0.0}} 360.0000: 1→0 |
| bridge/floorplan-2013 | librecad | dxf | 3 changes | count model:Dimension: 1→0; layout -Layout2; dim {"linear":{"rotation":0.0}} 360.0000: 1→0 |
| bridge/floorplan-2013 | oda-dwg | dwg | same |  |
| bridge/floorplan-2013 | libdxfrw | - | REJECTED: BLOCK/ENDBLK owner does not match BLOCK_RECORD DWG file error: format 17 error 10 | |
| bridge/floorplan-2013 | oda-dxf | dxf | same |  |
| bridge/floorplan-2013 | librecad-via-oda2007 | dxf | 3 changes | count model:Dimension: 1→0; layout -Layout2; dim {"linear":{"rotation":0.0}} 360.0000: 1→0 |
| bridge/floorplan-2018 | librecad | - | REJECTED: Error reading file | |
| bridge/floorplan-2018 | oda-dwg | dwg | same |  |
| bridge/floorplan-2018 | libdxfrw | - | REJECTED: BLOCK/ENDBLK owner does not match BLOCK_RECORD DWG file error: format 18 error 10 | |
| bridge/floorplan-2018 | oda-dxf | dxf | same |  |
| bridge/floorplan-2018 | librecad-via-oda2007 | dxf | 3 changes | count model:Dimension: 1→0; layout -Layout2; dim {"linear":{"rotation":0.0}} 360.0000: 1→0 |
| bridge/kitchen-2000 | librecad | dxf | 40 changes | count model:3D Face: 1→0; count model:3D Solid: 1→0; count model:Circle: 1→2; count model:Dimension: 6→0; count model:Hatch: 1→0; count model:Helix: 1→0; count model:Light: 1→0; count model:Line: 1→2; count model:MLine: 1→0; count model:PDF Underlay: 1→0; count model:Polyface Mesh: 1→0; count model:Polyline: 2→3; count model:Ray: 1→0; count model:Region: 1→0; count model:Section Plane: 1→0; count model:Table: 1→0; count model:Tolerance: 1→0; count model:Trace: 1→0; count model:Wipeout: 1→0; count model:Xline: 1→0; count paper:Viewport: 2→0; block TAG: 2→0; anonymous blocks: 6→2; layout -Layout2; layout -Sheet A; text "1.5": 1→0; text "204": 1→0; text "H1": 1→0; text "H2": 1→0; text "Schedule": 1→0; text "a": 1→0; text "b": 1→0; text "c": 1→0; text "{\\Fgdt;j}%%v{\\Fgdt;n}0.1{\\Fgdt;m}%%v%%vA%%vB": 1→0; dim "aligned" 14.1421: 1→0; dim "angular3P" 1.1071: 1→0; dim "diameter" 8.0000: 1→0; dim "radius" 4.0000: 1→0; dim {"linear":{"rotation":0.0}} 20.0000: 1→0; dim {"ordinate":{"x_type":false}} 80.0000: 1→0 |
| bridge/kitchen-2000 | oda-dwg | dwg | 1 changes | text "Schedule": 1→0 |
| bridge/kitchen-2000 | libdxfrw | - | REJECTED: BLOCK/ENDBLK owner does not match BLOCK_RECORD DWG file error: format 13 error 10 | |
| bridge/kitchen-2000 | oda-dxf | dxf | 1 changes | text "Schedule": 1→0 |
| bridge/kitchen-2000 | librecad-via-oda2007 | dxf | 39 changes | count model:3D Face: 1→0; count model:3D Solid: 1→0; count model:Dimension: 6→0; count model:Hatch: 1→0; count model:Helix: 1→0; count model:Light: 1→0; count model:Line: 1→2; count model:MLine: 1→0; count model:PDF Underlay: 1→0; count model:Polyface Mesh: 1→0; count model:Polyline: 2→3; count model:Ray: 1→0; count model:Region: 1→0; count model:Section Plane: 1→0; count model:Table: 1→0; count model:Tolerance: 1→0; count model:Trace: 1→0; count model:Wipeout: 1→0; count model:Xline: 1→0; count paper:Viewport: 2→0; block TAG: 2→1; anonymous blocks: 6→7; layout -Layout2; layout -Sheet A; text "1.5": 1→0; text "204": 1→0; text "H1": 1→0; text "H2": 1→0; text "Schedule": 1→0; text "a": 1→0; text "b": 1→0; text "c": 1→0; text "{\\Fgdt;j}%%v{\\Fgdt;n}0.1{\\Fgdt;m}%%v%%vA%%vB": 1→0; dim "aligned" 14.1421: 1→0; dim "angular3P" 1.1071: 1→0; dim "diameter" 8.0000: 1→0; dim "radius" 4.0000: 1→0; dim {"linear":{"rotation":0.0}} 20.0000: 1→0; dim {"ordinate":{"x_type":false}} 80.0000: 1→0 |
| bridge/kitchen-2004 | librecad | dxf | 39 changes | count model:3D Face: 1→0; count model:3D Solid: 1→0; count model:Dimension: 6→0; count model:Hatch: 1→0; count model:Helix: 1→0; count model:Light: 1→0; count model:Line: 1→2; count model:MLine: 1→0; count model:PDF Underlay: 1→0; count model:Polyface Mesh: 1→0; count model:Polyline: 2→3; count model:Ray: 1→0; count model:Region: 1→0; count model:Section Plane: 1→0; count model:Table: 1→0; count model:Tolerance: 1→0; count model:Trace: 1→0; count model:Wipeout: 1→0; count model:Xline: 1→0; count paper:Viewport: 2→0; block TAG: 2→1; anonymous blocks: 6→2; layout -Layout2; layout -Sheet A; text "1.5": 1→0; text "204": 1→0; text "H1": 1→0; text "H2": 1→0; text "Schedule": 1→0; text "a": 1→0; text "b": 1→0; text "c": 1→0; text "{\\Fgdt;j}%%v{\\Fgdt;n}0.1{\\Fgdt;m}%%v%%vA%%vB": 1→0; dim "aligned" 14.1421: 1→0; dim "angular3P" 1.1071: 1→0; dim "diameter" 8.0000: 1→0; dim "radius" 4.0000: 1→0; dim {"linear":{"rotation":0.0}} 20.0000: 1→0; dim {"ordinate":{"x_type":false}} 80.0000: 1→0 |
| bridge/kitchen-2004 | oda-dwg | dwg | 1 changes | text "Schedule": 1→0 |
| bridge/kitchen-2004 | libdxfrw | - | REJECTED: BLOCK/ENDBLK owner does not match BLOCK_RECORD DWG file error: format 14 error 10 | |
| bridge/kitchen-2004 | oda-dxf | dxf | 1 changes | text "Schedule": 1→0 |
| bridge/kitchen-2004 | librecad-via-oda2007 | dxf | 39 changes | count model:3D Face: 1→0; count model:3D Solid: 1→0; count model:Dimension: 6→0; count model:Hatch: 1→0; count model:Helix: 1→0; count model:Light: 1→0; count model:Line: 1→2; count model:MLine: 1→0; count model:PDF Underlay: 1→0; count model:Polyface Mesh: 1→0; count model:Polyline: 2→3; count model:Ray: 1→0; count model:Region: 1→0; count model:Section Plane: 1→0; count model:Table: 1→0; count model:Tolerance: 1→0; count model:Trace: 1→0; count model:Wipeout: 1→0; count model:Xline: 1→0; count paper:Viewport: 2→0; block TAG: 2→1; anonymous blocks: 6→7; layout -Layout2; layout -Sheet A; text "1.5": 1→0; text "204": 1→0; text "H1": 1→0; text "H2": 1→0; text "Schedule": 1→0; text "a": 1→0; text "b": 1→0; text "c": 1→0; text "{\\Fgdt;j}%%v{\\Fgdt;n}0.1{\\Fgdt;m}%%v%%vA%%vB": 1→0; dim "aligned" 14.1421: 1→0; dim "angular3P" 1.1071: 1→0; dim "diameter" 8.0000: 1→0; dim "radius" 4.0000: 1→0; dim {"linear":{"rotation":0.0}} 20.0000: 1→0; dim {"ordinate":{"x_type":false}} 80.0000: 1→0 |
| bridge/kitchen-2007 | librecad | dxf | 41 changes | count model:3D Face: 1→0; count model:3D Solid: 1→0; count model:Dimension: 6→0; count model:Hatch: 1→0; count model:Helix: 1→0; count model:Light: 1→0; count model:Line: 1→2; count model:MLine: 1→0; count model:Multileader: 1→0; count model:PDF Underlay: 1→0; count model:Polyface Mesh: 1→0; count model:Polyline: 2→3; count model:Ray: 1→0; count model:Region: 1→0; count model:Section Plane: 1→0; count model:Table: 1→0; count model:Tolerance: 1→0; count model:Trace: 1→0; count model:Wipeout: 1→0; count model:Xline: 1→0; count paper:Viewport: 2→0; block TAG: 2→1; anonymous blocks: 6→2; layout -Layout2; layout -Sheet A; text "1.5": 1→0; text "204": 1→0; text "Callout": 1→0; text "H1": 1→0; text "H2": 1→0; text "Schedule": 1→0; text "a": 1→0; text "b": 1→0; text "c": 1→0; text "{\\Fgdt;j}%%v{\\Fgdt;n}0.1{\\Fgdt;m}%%v%%vA%%vB": 1→0; dim "aligned" 14.1421: 1→0; dim "angular3P" 1.1071: 1→0; dim "diameter" 8.0000: 1→0; dim "radius" 4.0000: 1→0; dim {"linear":{"rotation":0.0}} 20.0000: 1→0; dim {"ordinate":{"x_type":false}} 80.0000: 1→0 |
| bridge/kitchen-2007 | oda-dwg | dwg | 1 changes | text "Schedule": 1→0 |
| bridge/kitchen-2007 | libdxfrw | - | REJECTED: BLOCK/ENDBLK owner does not match BLOCK_RECORD DWG file error: format 15 error 10 | |
| bridge/kitchen-2007 | oda-dxf | dxf | 1 changes | text "Schedule": 1→0 |
| bridge/kitchen-2007 | librecad-via-oda2007 | dxf | 41 changes | count model:3D Face: 1→0; count model:3D Solid: 1→0; count model:Dimension: 6→0; count model:Hatch: 1→0; count model:Helix: 1→0; count model:Light: 1→0; count model:Line: 1→2; count model:MLine: 1→0; count model:Multileader: 1→0; count model:PDF Underlay: 1→0; count model:Polyface Mesh: 1→0; count model:Polyline: 2→3; count model:Ray: 1→0; count model:Region: 1→0; count model:Section Plane: 1→0; count model:Table: 1→0; count model:Tolerance: 1→0; count model:Trace: 1→0; count model:Wipeout: 1→0; count model:Xline: 1→0; count paper:Viewport: 2→0; block TAG: 2→1; anonymous blocks: 6→7; layout -Layout2; layout -Sheet A; text "1.5": 1→0; text "204": 1→0; text "Callout": 1→0; text "H1": 1→0; text "H2": 1→0; text "Schedule": 1→0; text "a": 1→0; text "b": 1→0; text "c": 1→0; text "{\\Fgdt;j}%%v{\\Fgdt;n}0.1{\\Fgdt;m}%%v%%vA%%vB": 1→0; dim "aligned" 14.1421: 1→0; dim "angular3P" 1.1071: 1→0; dim "diameter" 8.0000: 1→0; dim "radius" 4.0000: 1→0; dim {"linear":{"rotation":0.0}} 20.0000: 1→0; dim {"ordinate":{"x_type":false}} 80.0000: 1→0 |
| bridge/kitchen-2010 | librecad | dxf | 41 changes | count model:3D Face: 1→0; count model:3D Solid: 1→0; count model:Dimension: 6→0; count model:Hatch: 1→0; count model:Helix: 1→0; count model:Light: 1→0; count model:Line: 1→2; count model:MLine: 1→0; count model:Mesh: 1→0; count model:Multileader: 1→0; count model:PDF Underlay: 1→0; count model:Polyline: 2→3; count model:Ray: 1→0; count model:Region: 1→0; count model:Section Plane: 1→0; count model:Table: 1→0; count model:Tolerance: 1→0; count model:Trace: 1→0; count model:Wipeout: 1→0; count model:Xline: 1→0; count paper:Viewport: 2→0; block TAG: 2→1; anonymous blocks: 6→2; layout -Layout2; layout -Sheet A; text "1.5": 1→0; text "204": 1→0; text "Callout": 1→0; text "H1": 1→0; text "H2": 1→0; text "Schedule": 1→0; text "a": 1→0; text "b": 1→0; text "c": 1→0; text "{\\Fgdt;j}%%v{\\Fgdt;n}0.1{\\Fgdt;m}%%v%%vA%%vB": 1→0; dim "aligned" 14.1421: 1→0; dim "angular3P" 1.1071: 1→0; dim "diameter" 8.0000: 1→0; dim "radius" 4.0000: 1→0; dim {"linear":{"rotation":0.0}} 20.0000: 1→0; dim {"ordinate":{"x_type":false}} 80.0000: 1→0 |
| bridge/kitchen-2010 | oda-dwg | dwg | same |  |
| bridge/kitchen-2010 | libdxfrw | - | REJECTED: BLOCK/ENDBLK owner does not match BLOCK_RECORD DWG file error: format 16 error 10 | |
| bridge/kitchen-2010 | oda-dxf | dxf | same |  |
| bridge/kitchen-2010 | librecad-via-oda2007 | dxf | 41 changes | count model:3D Face: 1→0; count model:3D Solid: 1→0; count model:Dimension: 6→0; count model:Hatch: 1→0; count model:Helix: 1→0; count model:Light: 1→0; count model:Line: 1→2; count model:MLine: 1→0; count model:Mesh: 1→0; count model:Multileader: 1→0; count model:PDF Underlay: 1→0; count model:Polyline: 2→3; count model:Ray: 1→0; count model:Region: 1→0; count model:Section Plane: 1→0; count model:Table: 1→0; count model:Tolerance: 1→0; count model:Trace: 1→0; count model:Wipeout: 1→0; count model:Xline: 1→0; count paper:Viewport: 2→0; block TAG: 2→1; anonymous blocks: 6→7; layout -Layout2; layout -Sheet A; text "1.5": 1→0; text "204": 1→0; text "Callout": 1→0; text "H1": 1→0; text "H2": 1→0; text "Schedule": 1→0; text "a": 1→0; text "b": 1→0; text "c": 1→0; text "{\\Fgdt;j}%%v{\\Fgdt;n}0.1{\\Fgdt;m}%%v%%vA%%vB": 1→0; dim "aligned" 14.1421: 1→0; dim "angular3P" 1.1071: 1→0; dim "diameter" 8.0000: 1→0; dim "radius" 4.0000: 1→0; dim {"linear":{"rotation":0.0}} 20.0000: 1→0; dim {"ordinate":{"x_type":false}} 80.0000: 1→0 |
| bridge/kitchen-2013 | librecad | dxf | 41 changes | count model:3D Face: 1→0; count model:3D Solid: 1→0; count model:Dimension: 6→0; count model:Hatch: 1→0; count model:Helix: 1→0; count model:Light: 1→0; count model:Line: 1→2; count model:MLine: 1→0; count model:Mesh: 1→0; count model:Multileader: 1→0; count model:PDF Underlay: 1→0; count model:Polyline: 2→3; count model:Ray: 1→0; count model:Region: 1→0; count model:Section Plane: 1→0; count model:Table: 1→0; count model:Tolerance: 1→0; count model:Trace: 1→0; count model:Wipeout: 1→0; count model:Xline: 1→0; count paper:Viewport: 2→0; block TAG: 2→1; anonymous blocks: 6→2; layout -Layout2; layout -Sheet A; text "1.5": 1→0; text "204": 1→0; text "Callout": 1→0; text "H1": 1→0; text "H2": 1→0; text "Schedule": 1→0; text "a": 1→0; text "b": 1→0; text "c": 1→0; text "{\\Fgdt;j}%%v{\\Fgdt;n}0.1{\\Fgdt;m}%%v%%vA%%vB": 1→0; dim "aligned" 14.1421: 1→0; dim "angular3P" 1.1071: 1→0; dim "diameter" 8.0000: 1→0; dim "radius" 4.0000: 1→0; dim {"linear":{"rotation":0.0}} 20.0000: 1→0; dim {"ordinate":{"x_type":false}} 80.0000: 1→0 |
| bridge/kitchen-2013 | oda-dwg | dwg | same |  |
| bridge/kitchen-2013 | libdxfrw | - | REJECTED: BLOCK/ENDBLK owner does not match BLOCK_RECORD DWG file error: format 17 error 10 | |
| bridge/kitchen-2013 | oda-dxf | dxf | same |  |
| bridge/kitchen-2013 | librecad-via-oda2007 | dxf | 41 changes | count model:3D Face: 1→0; count model:3D Solid: 1→0; count model:Dimension: 6→0; count model:Hatch: 1→0; count model:Helix: 1→0; count model:Light: 1→0; count model:Line: 1→2; count model:MLine: 1→0; count model:Mesh: 1→0; count model:Multileader: 1→0; count model:PDF Underlay: 1→0; count model:Polyline: 2→3; count model:Ray: 1→0; count model:Region: 1→0; count model:Section Plane: 1→0; count model:Table: 1→0; count model:Tolerance: 1→0; count model:Trace: 1→0; count model:Wipeout: 1→0; count model:Xline: 1→0; count paper:Viewport: 2→0; block TAG: 2→1; anonymous blocks: 6→7; layout -Layout2; layout -Sheet A; text "1.5": 1→0; text "204": 1→0; text "Callout": 1→0; text "H1": 1→0; text "H2": 1→0; text "Schedule": 1→0; text "a": 1→0; text "b": 1→0; text "c": 1→0; text "{\\Fgdt;j}%%v{\\Fgdt;n}0.1{\\Fgdt;m}%%v%%vA%%vB": 1→0; dim "aligned" 14.1421: 1→0; dim "angular3P" 1.1071: 1→0; dim "diameter" 8.0000: 1→0; dim "radius" 4.0000: 1→0; dim {"linear":{"rotation":0.0}} 20.0000: 1→0; dim {"ordinate":{"x_type":false}} 80.0000: 1→0 |
| bridge/kitchen-2018 | librecad | - | REJECTED: Error reading file | |
| bridge/kitchen-2018 | oda-dwg | dwg | same |  |
| bridge/kitchen-2018 | libdxfrw | - | REJECTED: BLOCK/ENDBLK owner does not match BLOCK_RECORD DWG file error: format 18 error 10 | |
| bridge/kitchen-2018 | oda-dxf | dxf | same |  |
| bridge/kitchen-2018 | librecad-via-oda2007 | dxf | 41 changes | count model:3D Face: 1→0; count model:3D Solid: 1→0; count model:Dimension: 6→0; count model:Hatch: 1→0; count model:Helix: 1→0; count model:Light: 1→0; count model:Line: 1→2; count model:MLine: 1→0; count model:Mesh: 1→0; count model:Multileader: 1→0; count model:PDF Underlay: 1→0; count model:Polyline: 2→3; count model:Ray: 1→0; count model:Region: 1→0; count model:Section Plane: 1→0; count model:Table: 1→0; count model:Tolerance: 1→0; count model:Trace: 1→0; count model:Wipeout: 1→0; count model:Xline: 1→0; count paper:Viewport: 2→0; block TAG: 2→1; anonymous blocks: 6→7; layout -Layout2; layout -Sheet A; text "1.5": 1→0; text "204": 1→0; text "Callout": 1→0; text "H1": 1→0; text "H2": 1→0; text "Schedule": 1→0; text "a": 1→0; text "b": 1→0; text "c": 1→0; text "{\\Fgdt;j}%%v{\\Fgdt;n}0.1{\\Fgdt;m}%%v%%vA%%vB": 1→0; dim "aligned" 14.1421: 1→0; dim "angular3P" 1.1071: 1→0; dim "diameter" 8.0000: 1→0; dim "radius" 4.0000: 1→0; dim {"linear":{"rotation":0.0}} 20.0000: 1→0; dim {"ordinate":{"x_type":false}} 80.0000: 1→0 |

## What was written (`ours/written.tsv`)

"not encoded" lists what the native writer skipped; "kept" counts records it carried through its own dictionary.

| file | writer used | not encoded | kept |
|---|---|---|---|
| native/kitchen-2000.dwg | native | [] | 10 |
| native/kitchen-2004.dwg | native | [] | 10 |
| native/kitchen-2007.dwg | native | [] | 10 |
| native/kitchen-2010.dwg | native | [] | 12 |
| native/kitchen-2013.dwg | native | [] | 12 |
| native/kitchen-2018.dwg | native | [] | 12 |
| bridge/kitchen-2000.dwg | bridge | - | 0 |
| bridge/kitchen-2004.dwg | bridge | - | 0 |
| bridge/kitchen-2007.dwg | bridge | - | 0 |
| bridge/kitchen-2010.dwg | bridge | - | 0 |
| bridge/kitchen-2013.dwg | bridge | - | 0 |
| bridge/kitchen-2018.dwg | bridge | - | 0 |
| native/floorplan-2000.dwg | native | [] | 4 |
| native/floorplan-2004.dwg | native | [] | 4 |
| native/floorplan-2007.dwg | native | [] | 4 |
| native/floorplan-2010.dwg | native | [] | 4 |
| native/floorplan-2013.dwg | native | [] | 4 |
| native/floorplan-2018.dwg | native | [] | 4 |
| bridge/floorplan-2000.dwg | bridge | - | 0 |
| bridge/floorplan-2004.dwg | bridge | - | 0 |
| bridge/floorplan-2007.dwg | bridge | - | 0 |
| bridge/floorplan-2010.dwg | bridge | - | 0 |
| bridge/floorplan-2013.dwg | bridge | - | 0 |
| bridge/floorplan-2018.dwg | bridge | - | 0 |
| native/bracket-2000.dwg | native | [] | 4 |
| native/bracket-2004.dwg | native | [] | 4 |
| native/bracket-2007.dwg | native | [] | 4 |
| native/bracket-2010.dwg | native | [] | 4 |
| native/bracket-2013.dwg | native | [] | 4 |
| native/bracket-2018.dwg | native | [] | 4 |
| bridge/bracket-2000.dwg | bridge | - | 0 |
| bridge/bracket-2004.dwg | bridge | - | 0 |
| bridge/bracket-2007.dwg | bridge | - | 0 |
| bridge/bracket-2010.dwg | bridge | - | 0 |
| bridge/bracket-2013.dwg | bridge | - | 0 |
| bridge/bracket-2018.dwg | bridge | - | 0 |
