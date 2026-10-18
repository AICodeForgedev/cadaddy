# The .cdy drawing format (version 1)

`.cdy` ("CADaddy drawing") is CADaddy's native file format. It saves exactly what CADaddy holds: every table,
header variable, layout, block and object, and the file data kept from other programs. DWG and DXF remain the
formats for exchange. This document is the public specification; the reference implementation is
`crates/io/src/cdy/`.

## Why our own chunked layout, not zip

| need | chunked layout | zip |
|------|----------------|-----|
| random access to a part | index chunk at the end, one seek | central directory, same |
| streaming write | chunks appended in order, index last | same |
| a checksum per part | CRC-32 in every chunk header | CRC-32 per entry |
| big binary blobs (images, meshes) | a chunk stored uncompressed | stored entry |
| small implementation, no new dependency | ~200 lines, `crc32fast` + `miniz_oxide` (already used) | a zip crate, or a zip writer of our own |
| recovering a damaged file | chunks are self-delimiting: walk from the header | needs local headers, which writers may omit |
| a schema version per part | in every chunk header | needs a naming convention |

A person can still see what a file is: the manifest chunk is plain JSON (see below), and `cadaddy-cli info`
prints it.

## Layout

All integers are little-endian.

```
header   16 bytes   magic "CADDY" 1A 0D 0A (8) · format version u16 (= 1) · flags u16 (= 0) · reserved u32 (= 0)
chunk*              see below, in any order the writer chose
INDX               an index chunk listing every chunk before it
trailer  16 bytes   offset of the INDX chunk u64 · "CDYINDEX"
```

The magic's last three bytes (`1A 0D 0A`) make a text-mode copy that rewrites line ends visibly corrupt the
file. A reader refuses a format version greater than its own with a "made by a newer CADaddy" message.

### Chunk

```
tag            4 bytes   ASCII, e.g. "ENTS"
schema         u16       the version of this chunk type's content
flags          u16       bit 0: payload is deflate-compressed (RFC 1951, raw)
stored length  u64       bytes of payload that follow
crc32          u32       CRC-32 (IEEE) of the stored payload
raw length     u64       bytes after decompression (= stored length when not compressed)
payload        stored length bytes
```

A writer compresses a payload only when that makes it smaller. Already-compressed data (PNG) is stored as it is.

### Index

The `INDX` chunk's payload is a postcard list of `(tag [u8;4], schema u16, offset u64, stored length u64,
raw length u64)`, one per chunk in file order. A reader uses it when the trailer and the index are sound. Otherwise
it walks the chunks from byte 16, one after the other, until the bytes run out or stop forming a chunk.

## Rules for readers (schema evolution and damage)

1. **Unknown chunk types are skipped.** A later version can add chunk types and older readers still open the file.
2. **A known chunk type at a newer schema version is refused** with "made by a newer CADaddy". Changing what a chunk
   type holds in a way older readers can't read means raising its schema version.
3. **Inside a chunk, data is self-describing** (see Encoding). A record that gains a field still reads in an older
   reader, which ignores the field. A record that lacks a field reads with the field's default. A new field
   therefore doesn't need a new schema version when its default is right for older files.
4. **Damage is reported, not fatal.** A chunk whose CRC, compression or lengths don't check out is left out, and the
   drawing opens without it. So is a chunk that doesn't decode, or an object that doesn't read. Each omission adds a
   note to the drawing's reading notes, as RECOVER does.
5. **Every length and count is hostile.** Each is checked against the bytes that are there and against caps before
   anything is allocated. A chunk inflates to at most 2 GiB; a file holds at most 10⁶ chunks, a chunk at most
   5·10⁷ tokens and 2²⁰ keys; values nest at most 256 deep and are rebuilt without recursion.

## Encoding of model data

Bulk chunks hold a value tree. It has the shape every model type serialises to: null, booleans, unsigned and signed
64-bit integers, 64-bit floats (every bit kept), UTF-8 text, lists and keyed records. The tree is written as a
postcard struct `(keys: [text], tokens: [token])`. `keys` lists every record key the chunk uses, once, in the order
first met. `tokens` is the tree's pre-order walk:

| token (postcard enum index) | meaning |
|-----------------------------|---------|
| 0 `Null` | null |
| 1 `Bool(bool)` | a boolean |
| 2 `U(u64)` | a non-negative integer |
| 3 `I(i64)` | a negative integer |
| 4 `F(f64)` | a float |
| 5 `S(text)` | text |
| 6 `List(u32)` | a list of this many values follows |
| 7 `Record(u32)` | a record of this many entries follows, each a `Key` token then its value |
| 8 `Key(u32)` | the key at this place in `keys` |

A record's entries are written in key order, so equal drawings give equal bytes (files diff and dedupe well).
Non-finite floats aren't representable. The model doesn't hold them; one that did would be read as a damaged
object.

## Chunk types (schema 1)

| tag | holds | encoding |
|-----|-------|----------|
| `MANI` | the manifest: `format`, `writer` (e.g. `CADaddy 0.7.7`), `insunits`, `measurement`, `extents` (`[minx, miny, maxx, maxy]` of model space or null), `objects`, `blocks`, `layouts` (names), `thumbnail` (the tag of the preview chunk or null) | JSON text, the first chunk |
| `HEAD` | the header variables | value tree |
| `TABL` | a record of `layers`, `linetypes`, `textStyles`, `dimStyles`, `mleaderStyles`, `tableStyles`, `mlineStyles`, `views`, `vports`, `ucss`, `layerStates`, `groups`, `plotStyleTables`, `constraints`, `parametric`, `scales`, `handseed`, `hiddenSaved`, `hidden` (handles) | value tree |
| `BLKS` | a list of block definitions, each `[name, block, objects]`; a writer puts one per chunk | value tree |
| `ENTS` | model space objects in draw order, one chunk of the drawing's object store (up to 1024) each; as many chunks as needed | value tree |
| `LAYO` | one layout: `[layout, objects]` (its paper space objects) | value tree |
| `FRGN` | data from other programs kept without modelling it: `classes`, `objects` (`[handle, type, groups]`), `dictionary`, `entities` and `records` (extension dictionaries, reactors, xdata, colour books, properties), `targets`, `dataStore` | value tree |
| `THMB` | a PNG preview of model space (256 × 192) | raw PNG, stored uncompressed |

Each record's fields are those of the model type it mirrors, named in camelCase. Objects carry their handle, common
properties (layer, colour, linetype, xdata, …) and their kind with its data.

What isn't saved, by design: session state, i.e. why an xref couldn't load, an ARRAYEDIT in progress, a REFEDIT in
progress, the notes from reading the file and the records the reader skipped.

## Incremental saves

Saving a drawing again to the .cdy file it was saved to or read from appends only the chunks that
changed. The appended bytes are those chunks, a new `INDX` listing every current chunk (the unchanged
ones where they already are), and a new trailer. Readers only ever follow the last index, so the
superseded chunks are dead space. A writer:

- appends only when the file is exactly as it last wrote or read it (same length and modification
  time); otherwise it writes the whole file;
- writes the chunks, makes them durable, then writes the index and trailer;
- writes the whole file instead once the dead space would exceed 30% of the file and 64 KiB
  (compaction). A whole file is written to a temporary file and renamed over the old one;
- keeps chunks of types it doesn't know in the new index;
- keeps the existing preview (`THMB`) on an appended save; a whole-file save renders a new one.

A save stopped partway (the app or machine stopped) leaves no trailer at the file's end. A reader
then walks the chunks, skipping earlier trailers, and uses the last `INDX` whose chunks are all
present, i.e. the previous complete save.

## Compatibility

`crates/io/src/cdy/v1_sample.cdy` is a file written by the first version-1 writer. Every later version must read it
(test `the_frozen_v1_sample_still_reads`).

## Revision history

| date | change |
|------|--------|
| 2026-10-18 | Version 1. |
| 2026-10-18 | Incremental saves (append, last complete index wins); one block per `BLKS` chunk, one store chunk per `ENTS` chunk. Readers of version 1 read these files unchanged. |
