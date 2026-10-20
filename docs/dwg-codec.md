# CADaddy's own DWG codec (`crates/dwgcodec`)

Status: containers done (Q198); entities decoded (Q199), table records and the common objects decoded (Q200). Objects (Q200), the
writer from the drawing model (Q201) and R2007 (Q202) are next.

## Why

Opening a large DWG today goes DWG → acadrust → DXF text → our DXF reader → the drawing. On a
214k-object plan that is about 15 s, and almost all of it is the DXF text detour. Our own codec
reads DWG straight into our model and writes it back without the detour.

## Clean room

The only source is the Open Design Alliance's public *Open Design Specification for .dwg files*
(cited below and in the code as "spec §n"). No other DWG implementation's code has been read:
not acadrust's DWG code, not LibreDWG, not any ODA SDK. acadrust is used only as a black box in
tests. It writes sample DWG files from a made-up DXF, and its DXF output lists the handles it read.
Where the spec is silent or wrong, the behaviour was worked out from those black-box files and is
listed under "Spec gaps found by observation".

## Design

`read(bytes) -> Dwg` and `write(&Dwg) -> bytes`. A `Dwg` holds:

- `version`
- `header`: the header variables section (`Framed`: body, the second size field, trailing bytes).
  `header_vars()` decodes it into named values; `set_header_vars()` encodes them back.
- `classes`: the classes section. `classes()` / `set_classes()` decode and encode it.
- `objects`: every object in the object map as `RawObject { handle, type_code, data }`, in stored
  order. The data is the record's bytes between its size and its CRC.
- `sections`: every other section kept raw (preview, summary info, app info, aux header, …).
- a private layout, so the file is written back the way it was.

Modules:

| Module | Spec | What |
|---|---|---|
| `bits` | §2 | `BitReader` / `BitWriter` for every bit code; the R2007+ string-stream locator |
| `crc` | §2.14, §4.2 | 16-bit CRC (0xA001, seeded), CRC-32, the page checksum |
| `frame` | §9, §10 | sentinel / size / data / CRC / sentinel sections |
| `objmap` | §23 | the object map |
| `objects` | §20.1 | object records |
| `r2000` | §3, §14.2, §21, §25, §26, §27 | the R2000 file layout |
| `lz77` | §4.7 | the R2004+ compression, both ways |
| `r2004` | §4, §6–§8, §12 | the R2004+ file layout (system pages, page map, section map, data pages) |
| `header` | §9, §5.9 | header variables |
| `classes` | §10, §5.8 | classes |

Writing:

- **R2000** keeps the order of the regions and the bytes between them. Addresses are recomputed,
  and the values that hold addresses are updated: the locators, the image seeker, the objects'
  start in the free-space data, and the preview's own pointers. A second file header (§26) is kept
  only while its addresses are unchanged.
- **R2004+** reuses every page of an unchanged section, re-masking a page header if the page moves.
  A changed section is split into its page size, compressed, and checksummed again. When anything
  changes, pages are renumbered 1, 2, … in file order, and the page map, section map, file header
  data (with its CRC-32) and its closing copy are rebuilt.
- An unchanged file comes back byte for byte in every supported version.

Hostile input: every size, count and loop is capped (`Limits`, plus constants in each module),
every read is bounds-checked, and errors are values, never panics. `examples/fuzz_read.rs`
mutates sample files and random bytes through read, decode, encode and write.

## Version matrix

| Version | Id | Read | Write | Byte for byte | Notes |
|---|---|---|---|---|---|
| R13, R14 | AC1012, AC1014 | refused | — | — | "not read yet" |
| R2000 | AC1015 | yes | yes | yes | §3 layout; new files (Q201) |
| R2004 | AC1018 | yes | yes | yes | §4 layout |
| R2007 | AC1021 | yes | yes | no (laid out anew) | own container (§5), Q202 |
| R2010 | AC1024 | yes | yes | yes | §4 layout, R2010 object type and stream split |
| R2013 | AC1027 | yes | yes | yes | adds REQUIREDVERSIONS, AcDs section kept raw |
| R2018 | AC1032 | yes | yes | yes | as R2013 |

Also refused: encrypted sections (`Encrypted == 1`, password-protected files).

Tested for every readable version, on files acadrust writes with 0, 12, 400 and 3000 entities:

- byte-for-byte round trip
- object map ⊇ acadrust's handles; the extras are only DXF-less types (VX_CONTROL 0x46,
  VX_TABLE_RECORD 0x47), and the counts are equal once those are added
- a changed object makes a file that we and acadrust both read (same handles), and that comes
  back byte for byte again
- header variables and classes decode, point at objects of the right types, and encode to the
  same bits
- changed variables are written and read back

## Spec sections used

§2 (bit codes, CRCs), §3 (R13–R15 organization), §5 (R2007 organization, compression, CRCs, Reed–Solomon), §4.1–4.7 (R2004 header, page checksum, system
pages, page map, section map, masked page headers, compression), §5.8–5.9 (string streams for
classes and the header), §6–§8 (R2010–R2018), §9 (header variables), §10 (classes), §12 (empty
section), §14.2 (preview), §20.1 (object records), §21 (free space), §22 (template), §23 (object
map), §25 (unknown section), §26 (second header), §27 (aux header).

## Spec gaps found by observation

Each was worked out from files acadrust wrote (black box). The evidence is the check that passes
only one way.

1. **Object map deltas restart in every chunk** (§23). Both the handle and the location deltas
   start again from 0 at each chunk. *Evidence:* in a 3000-entity R2000 file (5 chunks, 3041
   objects), all 3041 object CRCs check with the reset and only 1004 without it.
2. **LZ77 copy distance is compOffset + 1** (§4.7). A compOffset of 0 copies the byte just
   written. *Evidence:* with +1, the R2004+ page maps decode to their stated sizes as sensible
   (page number, size) pairs, and every section of every sample decompresses to its stated size
   with all page checksums passing. Without +1 the page map decodes to garbage.
3. **From R2010 an object's MS size does not count the MC handle-stream size** (§20.1). The
   record is MS, MC, `size` bytes, CRC (the CRC covers all of them). *Evidence:* in R2010 and
   R2013 files every object's CRC checks only when the bytes after `size` include the MC, and the
   bytes left over equal the MC's length (1, or 2 when the handle stream is over 127 bits).
4. **The closing copy of the R2004 header data can start inside the last page's extent**
   (§4.1). acadrust puts it where the page map's compressed data ends, which can be 1 byte before
   the page's declared end (R2010) or 2 bytes past it (R2004). *Evidence:* the 0x34 "second
   header address" field points there, and the 0x6C bytes there equal the encrypted copy at 0x80.
   Reading takes the system pages' length from their compressed size, not their extent.
5. **R2010+ header data starts with the RL "size in bits"** (§9 says "R2007 only"). *Evidence:*
   in R2010/R2013/R2018 files the RL after the section size(s) is just under the data's bit length
   (3997 of 4624), the string-stream flag at that end bit is set, and the strings and handles
   decode completely only with it.
6. **BLL's byte count is a plain 3-bit number, not a 3B** (§2.4 refers to §2.1). *Evidence:* the
   R2013 header's REQUIREDVERSIONS is a BLL; with a 3-bit count the four values after it read as
   their documented defaults (412148564080.0, 1.0, 1.0, 1.0); with a 3B count they do not.

Also confirmed (the spec gives them but leaves the detail to the reader):

- The section CRCs (header variables, classes, object map chunks, object records) are the 16-bit
  CRC seeded 0xC0C1, over the size field(s) and the data.
- R2004+ page padding is the §4.1 magic sequence from its start.
- Header variables and classes take the second size field from R2010 when the maintenance version
  is above 3, and always from R2018.
- R2007+ string streams of 0x8000 bits or more use the second size short (§20.1) in the classes
  section too (R2013 has ~32 kB of classes).

## Entities (Q199)

`entities::decode` turns an entity object into the groups the DXF Reference gives that entity
(code and value, in DXF order), so the DXF reader builds the same entity from a DWG as from a DXF
file, with no DXF text in between. `entities::Obj` opens an object's three streams for every
version (the data stream after the type, the R2000–R2007 size in bits and the own handle; from
R2007 the string stream at the data's end; the handle stream), `entities::common` reads what
every entity starts with (§20.4.1: extended data, proxy graphics, entity mode, reactors, the
extension dictionary, colour with true colour, colour reference and transparency, linetype,
plot style, material, shadow, visual styles, visibility, lineweight, and their handles), and
`entities::Names` reads the names entities point at (block records, layers, text styles,
linetypes, views, UCSs, viewports, applications, dimension styles, multiline styles).

Decoded (§20.4.x): LINE, CIRCLE, ARC, POINT, ELLIPSE, TEXT, ATTRIB, ATTDEF, INSERT, MINSERT,
VERTEX (2D, 3D, mesh, polyface, face record), POLYLINE (2D, 3D, mesh, polyface), SEQEND, SOLID,
TRACE, 3DFACE, RAY, XLINE, SPLINE (control and fit points), MTEXT, LWPOLYLINE, HATCH (polyline and
edge boundaries — lines, arcs, elliptical arcs, splines — patterns, gradients, seeds, boundary
objects), DIMENSION (linear, aligned, angular 2-line and 3-point, radius, diameter, ordinate),
LEADER, TOLERANCE, MLINE, IMAGE, WIPEOUT, VIEWPORT and MULTILEADER (context data, leader roots
and lines, text or block content, block attribute labels).

Not decoded yet (the reader keeps the bridge's record for them): 3DSOLID, REGION and BODY (their
ACIS data is in the AcDs section from R2013), ACAD_TABLE (its contents are TABLECONTENT objects,
Q200), MESH, LIGHT, the underlays, HELIX, ARC_DIMENSION, SHAPE (its name comes from the shape
file), OLE2FRAME, proxies and custom classes. A VIEWPORT's status (68) and id (69) aren't stored
in a DWG; they come from the layout's viewport list (Q200), so the bridge's are used meanwhile.

`cadaddy_io::dwg_native::read_with_native_entities` reads a file through the bridge into groups
and puts the codec's groups in place of every entity it decoded (an entity's VERTEXes, ATTRIBs and
SEQEND follow it), then builds the drawing from those groups.

Tested: `dwgcodec/tests/entities.rs` (the decoded groups equal acadrust's DXF for the same file,
group by group, in R2000, R2004, R2010, R2013 and R2018; mutated object data never panics),
`io/tests/dwg_native_entities.rs` (the drawing read with native entities equals the bridge's,
entity by entity and field by field, every record from the codec) and
`io/tests/dwg_native_groups.rs` (multileaders with text and with block content and attributes).

Entity facts found by observation (files acadrust wrote; to be confirmed on AutoCAD's own files):

1. **R2010+ LEADER has no annotation box size.** After the annotation offset only the hookline
   and arrowhead flags and the three trailing fields remain (the data ends exactly there); up to
   R2007 the box height and width come first.
2. **A diameter dimension stores its arc point (15) before its definition point (10)**; a radius
   dimension the other way round.
3. **MULTILEADER's block-content flag always follows the text content** (also when there is
   text), and the word-break flag is present from R2010.
4. A DIMENSION's DXF type flag 32 ("block used by this dimension only") isn't stored; the
   codec leaves it out, as the bridge does.

## Table records and objects (Q200)

`entities::records::decode` does for table records and objects what `entities::decode` does for
entities: the record's DXF groups, in the TABLES or OBJECTS section's form.

Decoded: every table — BLOCK_RECORD (with its preview, units, explodability and scaling), LAYER
(on/off, frozen, frozen in new viewports, locked, plotting, colour including true colour,
linetype, lineweight, plot style, material), LTYPE (dashes with shapes and text), STYLE, VIEW,
UCS, VPORT, APPID, DIMSTYLE (every dimension variable) — and the objects DICTIONARY, the
dictionary with a default, PLACEHOLDER, LAYOUT with its plot settings, PLOTSETTINGS, GROUP,
MLINESTYLE, XRECORD (layer states live in them), SCALE, DICTIONARYVAR, IMAGEDEF and its reactor.

An entity's place comes from the block records' lists of their entities (§20.4.49: from R2004
the owned handles; in R2000 the first and last, the entities' next links between): model space
and the active paper space are the header's BLOCK_RECORD_MODEL_SPACE and BLOCK_RECORD_PAPER_SPACE;
any other block — another layout's paper space among them — becomes the entity's owner (330).
The entity mode alone marks every paper space entity as the active layout's.

Not decoded yet (kept from the bridge): MLEADERSTYLE, TABLESTYLE, TABLECONTENT, FIELD and
FIELDLIST, MATERIAL, VISUALSTYLE, SUN, dynamic-block data, SORTENTSTABLE, SPATIAL_FILTER,
proxies and custom classes.

Tested: `dwgcodec/tests/records.rs` (records = acadrust's DXF, group by group, five versions;
mutated data never panics) and `io/tests/dwg_native_objects.rs` (the drawing read with native
tables and objects equals the bridge's: layers, linetypes, text and dimension styles, layer
states, layouts with their viewports, groups, views, UCSs, multiline styles).

Facts found by observation:

1. **From R2007 a table record has no external-reference index** (the BS after the "referenced"
   bit): an application record's data is exactly the two bits and its byte.
2. **From R2004 a layer's colour index short is 0**; the colour is the raw value's (method 0xC3
   an index, 0xC2 a true colour).
3. **The VPORT's second double is the view width** (aspect ratio × height), not the ratio.
4. Files from some writers name every layout's block record *Paper_Space; DXF needs them
   distinct, so the reader keeps the bridge's names for block records until the names are
   assigned from the layouts here (Q203).

## R2007 (Q202)

AC1021 has its own container (spec §5): 0x80 bytes of meta data, the file header as a
Reed–Solomon coded page at 0x80 (and again at the end), then pages from 0x480: the data
sections' pages, the section map and page map (system pages) with a copy of each. `r2007.rs`
reads and writes it; `lz21.rs` is its compression (§5.10), `rs21.rs` the (255, 251) and
(255, 239) Reed–Solomon codes (§5.13), `crc64.rs` the normal and mirrored 64-bit CRCs (§5.12),
`rand21.rs` the CRC random encoding (§5.11). The sections inside are the R2004 ones (§5.5–5.9),
read by the same code; R2007's bit coding (strings in their own stream, §2) was already handled.

Writing lays the file out anew: the sections in the order read (the spec's §5.1 order for a new
file), every page stored uncompressed (the spec allows it when compression doesn't help; the
reader decompresses), the checksums, CRCs, seeds and check data as §5.2–§5.4 give them, random
values and padding from the random encoding seeded 0. So a file is not rewritten byte for byte,
but our own layout of a file is stable, and the header variables, classes, objects and other
sections come back unchanged.

Tested (`dwgcodec/tests/containers.rs` and every other codec test now run on AC1021 too, as do
`io/tests/dwg_native_*.rs` and `dwg_write_native.rs`): files acadrust writes read to the same
handles; our layout of them reads back to the same contents and acadrust reads it to the same
handles; entities, tables and objects decode like the bridge's; the native writer's AC1021 files
read back through the bridge as the drawing; mutated files never panic.

Facts found by observation (each checked against the values stored in acadrust's files):

1. **The literal copy order of §5.10.1 applies inside each listed block too** (a 2-byte block is
   reversed, a 16-byte block is 8 [8], 8 [0], …). *Evidence:* only then does the decompressed file
   header's CRC64 match the stored one.
2. **The CRC byte order of §5.12 applies inside each block too**: an 8-byte block is taken
   6, 7, 4, 5, 2, 3, 0, 1. *Evidence:* the same CRC.
3. **The section map's name length is in bytes, the terminator included** (the spec doubles it).
4. **Encoding 1 data pages have no Reed–Solomon parity**: the data as is, padded to 0x20.
   *Evidence:* every encoding 1 page's size in the page map is its data rounded up to 0x20.
5. **The codes run over the block reversed** (the data bytes last to first, the parity stored
   last to first); first generator root α¹; field polynomials 0x11D (data) and 0x169 (system).
   *Evidence:* the parity of every block checks only that way.
6. **The CRCs and their seeds:** a page's CRC is the mirrored CRC of its stored bytes seeded with
   UpdateSeed1(0, length), its checksum §5.4.1's with seed 0 over the data; the system pages'
   CRCs likewise; the file header's stored-data CRC is the normal CRC seeded with
   UpdateSeed2(0, length), its checking sequence's the normal CRC seeded with UpdateSeed1(0, 16).
   The random encoding (seed 0) gives, in order: the section map's and page map's encoded seeds,
   the check data's two random values and encoded seed, CrcSeedEncoded, the checking sequence.
7. **The page map ends with an empty (0, 0) pair**, and the file header's stored size is 8 bytes
   at 0x18 (§5.2.1.5's layout; §5.2's table splits it in two).

## Writing (Q201)

`io::dwg_write::write(drawing, version)` makes a new DWG file (AC1015, AC1018, AC1021, AC1024,
AC1027, AC1032) with the codec alone, no bridge:

1. The drawing's DXF groups as the DXF writer makes them (in memory), split into records by
   section; each record's reactors and extension dictionary (DXF's 102 blocks) are taken apart.
2. Names to handles from the TABLES section; the classes from CLASSES, numbered from 500.
3. Each record encoded (`entities::encode`, `entities::encode_kinds`): the inverse of its decoder,
   field for field, so `decode(encode(groups))` gives the groups back. The handles are the
   drawing's own (kept handles stay). INSERTs own their attributes and POLYLINEs their vertices
   (with their SEQEND); block records list their entities (from R2004 every one; R2000 the first
   and last, the entities linked to their neighbours); layouts list their viewports. Table
   controls list their records, the spaces' block records and ByLayer/ByBlock linetypes apart.
4. The header variables: each version's set at a new drawing's defaults (the DXF Reference's),
   the DXF header's values over them, the control objects, dictionaries and spaces as handles,
   FLAGS packed from CELWEIGHT, LWDISPLAY, PSTYLEMODE and the rest.
5. `Dwg::new` makes a new file's containers (§3 R2000, §5 R2007 or §4 R2004+ layout); `Dwg::new_file_sections`
   the template (MEASUREMENT), free-space data, auxiliary header and an empty preview.
6. Records whose type isn't encoded are left out and listed in the report, and the file is made
   again without the references to them (dictionary entries, reactors), so nothing points at
   nothing.

Encoded: every entity the reader decodes (LINE … MULTILEADER, DIMENSION kinds, HATCH with every
boundary kind and gradients, MTEXT, INSERT/MINSERT with attributes, the POLYLINE family, SPLINE,
IMAGE, WIPEOUT, MLINE, VIEWPORT, LEADER, TOLERANCE), every table record, and the objects
DICTIONARY (with a default too), PLACEHOLDER, LAYOUT, PLOTSETTINGS, GROUP, MLINESTYLE,
MLEADERSTYLE, XRECORD, SCALE, DICTIONARYVAR, IMAGEDEF and its reactor, RASTERVARIABLES; extended
data on all of them.

Tested (`io/tests/dwg_write_native.rs`, five versions, four made-up drawings): the file the bridge
reads back (acadrust, black box) equals what the bridge's own DWG of the same drawing reads back,
entity by entity, table by table and variable by variable (those the source has), and our reader
agrees; every reference points at an object in the file; handles are unique; an empty drawing
reads back; other versions are refused.

Facts found by observation (the files acadrust writes, and what its reader accepts):

1. **From R2010 a record is padded at its end, the padding counted with the handle stream** (the
   MC size), not between the data and the string stream.
2. **Table records have the "referenced" bit set from R2004** (readers refuse them without it)
   and clear in R2000.
3. **Before R2004 every object stores its extension dictionary handle**, null when there is none
   (there is no "missing" bit).
4. **An R2013+ LAYER has one more handle** after its linetype (null).
5. **An R2018 MTEXT stores its context data** after the background (not annotative: version 4,
   default flag, attachment, axis, insertion, sizes, no columns) and one more handle (null).
6. **The header ends in four short integers of -1 and two of 0** (and from R2007 two bits in the
   string stream); a reader takes the header without them as damaged and uses its defaults.
7. **An R2000 extended-data string's code page is stored high byte first.**
8. **The LTYPE control's special entries are ByLayer, then ByBlock.**

Known limits:

- Left out: MATERIAL, TABLESTYLE, VISUALSTYLE, FIELD, SORTENTSTABLE, ACIS solids, proxies and
  other programs' custom records (the drawing's kept DXF records; raw DWG records kept from a file
  need the native reader to keep them, Q203).
- acadrust can't read an R2000 VIEWPORT linked to its neighbours (its own files store none of
  the links §20.4.1 has there); our files keep the links, our reader reads them.
- Not yet opened in AutoCAD (not installed on this machine; Q83's acadverify is the check).

## Not done yet / known limits

- R2000 second file header (§26) is dropped when the addresses it repeats change. acadrust writes
  none, so it is untested.
- The 4 trailing "type 5/6 only" header shorts (§9) are not named. They stay in the kept bits
  (a new file writes them as observed, Q201).
- Code-page strings are decoded as Latin-1 (`Text::to_string_lossy`). Code-page mapping belongs
  in the model layer.
