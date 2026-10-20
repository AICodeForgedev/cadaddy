# CADaddy's own DWG codec (`crates/dwgcodec`)

Status: containers done (Q198). Entities (Q199), objects (Q200), the writer from the drawing model
(Q201) and R2007 (Q202) are next.

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
| R2000 | AC1015 | yes | yes | yes | §3 layout |
| R2004 | AC1018 | yes | yes | yes | §4 layout |
| R2007 | AC1021 | refused | — | — | own container (§5), Q202 |
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

§2 (bit codes, CRCs), §3 (R13–R15 organization), §4.1–4.7 (R2004 header, page checksum, system
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

## Not done yet / known limits

- R2000 second file header (§26) is dropped when the addresses it repeats change. acadrust writes
  none, so it is untested.
- The 4 trailing "type 5/6 only" header shorts (§9) are not named. They stay in the kept bits.
- Code-page strings are decoded as Latin-1 (`Text::to_string_lossy`). Code-page mapping belongs
  in the model layer.
- Objects and entities are raw bytes until Q199/Q200.
