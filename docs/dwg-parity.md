# DWG: CADaddy's codec vs the bridge (Q203)

CADaddy reads and writes DWG two ways: through the DWG bridge (acadrust, converting to and from
DXF) or with its own codec (`crates/dwgcodec`, `io::dwg_read`, `io::dwg_write`). This page is the
switch, the parity report and the timings. Measured 2026-10-20 on the Mac (M-series, release).

## The switch

| variable | values | applies to |
|---|---|---|
| `CADADDY_DWG` | `native`, `acadrust` (or `bridge`) | reading and writing |
| `CADADDY_DWG_READ` | same | reading (over `CADADDY_DWG`) |
| `CADADDY_DWG_WRITE` | same | writing (over `CADADDY_DWG`) |

Open, save, DWGCONVERT and `cadaddy-cli convert` all go through `cadaddy_io::read` / `write`, so
they follow it. In code: `io::read_dwg(bytes, engine)` and `io::write_dwg(drawing, version,
engine)`. CADaddy's reader hands a file it can't read (encrypted, damaged) to the bridge and says
so in the read notes; its writer hands a version it doesn't write to the bridge.

**Default: the bridge, both ways** (`io::DEFAULT_DWG_READ`, `io::DEFAULT_DWG_WRITE`). Parity holds on
the fidelity matrix and the corpus but not yet on everything the io tests check (below), and no
file the codec wrote has been opened in AutoCAD yet. Flip the defaults when the lists below are
empty and AutoCAD opens the files.

## Parity

### Fidelity matrix (`crates/engine/tests/fidelity_matrix.rs`)

Six "DWG native" columns sit beside the bridge's. The codec's differences are the same version
limits the bridge has (no multileaders or MESH before 2010, …) and the same viewport numbering
(DWG doesn't store the number), **without** the bridge's DIMFXL loss and its 2000 multileader and
table style losses.

### Synthetic corpus (`crates/engine/tests/corpus_roundtrip.rs`, `cargo xtask corpus synthetic`)

The `dwg-native` format has the bridge's difference kinds, no others (dimension text positions,
hatch pattern lines, viewport numbers, header variables filled in).

### What the codec doesn't do yet

The io tests run with the codec (`CADADDY_DWG_READ=native` for files the bridge wrote, i.e. files
from other programs; `CADADDY_DWG=native` for both ways). Each line is a gap to close.

| test | reading others' files | both ways | why |
|---|---|---|---|
| `acis_data_store::dwg_keeps_the_solids_geometry`, `solids_dxf::solids_survive_dwg`, `composites_round_trip…` | ✗ | ✗ (first) | 3DSOLID / REGION / BODY and their ACIS data aren't decoded |
| `mesh_roundtrip::meshes_survive_dwg` | ✗ | | MESH isn't decoded |
| `helix_roundtrip::helix_survives_dwg_save_and_reopen` | ✗ | | HELIX isn't decoded |
| `pdf_underlay_roundtrip::dwg_keeps_underlays` | ✗ | | PDF/DWF/DGN underlays and their definitions aren't decoded |
| `xclip_roundtrip::*clip_survives_dxf_and_dwg` | ✗ | | SPATIAL_FILTER (block clips) isn't decoded |
| `materials_roundtrip::materials_survive_dxf_and_dwg` | ✗ | ✗ | MATERIAL isn't decoded or encoded |
| `dwg_roundtrip_losses::dwg_keeps_table_styles` | ✗ | | TABLESTYLE isn't decoded |
| `tests::dwg_roundtrip_keeps_extension_data` | ✗ | | ACAD_TABLE isn't decoded |
| `fields_dxf::fields_survive_a_dwg_round_trip` | ✗ | ✗ | FIELD / FIELDLIST aren't decoded (text shows the evaluated value) |
| `dynblock_read::a_saved_drawing_keeps_the_dynamic_block_as_read` | ✗ | ✗ | dynamic-block data (EVALUATION_GRAPH, parameters) isn't decoded |
| `roundtrip_audit::dwg_keeps_extension_dictionaries_xrecords_and_entity_xdata` | ✗ | ✗ | an entity's extension dictionary isn't written as DXF's 102 block |
| `dwg_keeps_custom_records::*` | ✗ | ✗ | proxies and other programs' classes: one flag differs after the keep (R2000) |
| `dwg_keeps_custom_records::a_drawing_without_custom_data_is_written_as_before` | | ✗ | the writer keeps DIMASSOC/MATERIAL in CADADDY_KEEP (no encoder) |
| `codepages::pre_2007_dwg_text_decodes_through_the_bridge`, `dxf_versions::dwg_at_every_version_reads_back` | ✗ | ✗ | pre-2007 strings are read as Latin-1, not in the file's code page |
| `dwg_header_vars::header_settings_round_trip_through_dwg` | ✗ | | $ANGBASE (and angles like it) taken as degrees: DWG stores radians |
| `dwg_roundtrip_losses::the_current_dimension_style_survives…` | | ✗ | DIMSCALE written 0 in the header when the drawing doesn't set it |
| `dwg_roundtrip_losses::mtext_background_masks_survive_dxf_and_dwg` | ✗ | | the mask colour's index (63) read as ByBlock |
| `tests::gradient_hatch_roundtrips_through_dwg` | ✗ | ✗ | a gradient's index colours (63 rather than 421) |
| `hatch_exact_edges::elliptical_and_spline_edges_roundtrip_in_dwg` | ✗ | | an elliptical edge's angles (degrees vs radians) |
| `attrib_modes::modes_survive_dxf_and_dwg` | | ✗ | ATTRIB/ATTDEF lock-position bit (R2007+) not written |
| `tests::damaged_dwg_opens_partially_and_says_what_it_left_out` | ✗ | ✗ | a damaged file goes to the bridge whole (the codec stops at the first bad object) |

Fixed while making this list (Q203): multileader styles (decoded and encoded, field order per
§20.4.87), viewport numbers from the layout's order, the DWG code page as `$DWGCODEPAGE`, block
records' names, xref paths and flags, extended-data points (1021/1031), extended-data handles
(8 bytes, high byte first: observed), DIMPOST with text around `<>`, a spline with fit and control
points (fit data kept), the writer's table controls (a set, not a scan per entry), and the keep
dictionary's read-back (one pass, not one per record).

## Timings

Release build, 100k objects of the synthetic corpus (seed 1), DWG 2018:

| | save | open |
|---|---|---|
| bridge | 2.31 s | 2.37 s |
| CADaddy's codec | 2.37 s | 1.45 s |

Measured at 500k (505,562 objects, seed 1, release, a Linux x86-64 build server that is about twice as
slow as the Mac above at 100k; `cargo xtask corpus synthetic --sizes 100000,500000 --seed 1`):

| | 100k save | 500k save | ratio | 100k open | 500k open | ratio |
|---|---|---|---|---|---|---|
| bridge (default) | 4.66 s | 50.90 s | 10.9x | 5.21 s | 61.74 s | 11.8x |
| CADaddy's codec (`CADADDY_DWG=native`) | 8.07 s | 35.39 s | 4.4x | 5.22 s | 25.69 s | 4.9x |

The codec grows about linearly. The bridge does not: 5x the objects costs about 11x the time. Both
bridge directions go through acadrust's `extension_dictionary_handle`, which, for a block record with
no extension dictionary, scans every object in the drawing; the corpus has about one block record per
ten objects, so the work is block records x objects. Timed inside the bridge, the DWG to DXF step is
0.7 s read + 2.6 s DXF write at 100k and 3.3 s read + 49 s DXF write at 500k, all of the difference in
the `BLOCK_RECORD` table; CADaddy's own DXF read and write stay linear (1.8 s to 9.7 s, 1.7 s to 9.3 s).
That scan is inside acadrust (used unmodified), and the index that would avoid it is not public, so it
can't be removed from CADaddy's side. The Mac 500k numbers are measured by the Mac gate; the budget
file now fails `--strict` runs on a 500k/100k ratio over 7 and, on macOS arm64, a 500k bridge open
over 15 s.

The target (a 500k-object plan opens and saves in under 3 s) isn't met by either yet. The codec's next
steps there: encode and decode in parallel, and read DIMASSOC so the keep dictionary isn't needed.
