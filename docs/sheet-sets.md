# Sheet sets

SHEETSET (SSM) opens the Sheet Set Manager palette. It docks like the
other palettes. A sheet set is CADaddy's own JSON file (`*.sheetset.json`). It lists layouts of
drawings as numbered, titled sheets in subsets. AutoCAD's `.dst` files are not read.

Everything the palette does is a SHEETSET action (`{"set": FILE, "action": …}`), so scripts, the
control channel and MCP can do the same.

## Sheets

`new`, `add` (a drawing's layouts), `remove`, `set` (number, title, description, subset),
`move`, `renumber`, `subset` (add, rename, remove). Each sheet has an id that it keeps when it is
renumbered, so views and callouts follow it.

## Sync

`sync` (the palette's **Sync Drawings**) writes the set into its drawings. Each drawing's
previous version is kept as `.bak`. Sync does four things:

- It renames each sheet's layout to "number title".
- It stores the set's and its sheets' names, descriptions and custom properties in each drawing
  (saved with the drawing; in DXF as the `CADADDY_SHEETSET` record).
- It puts each view's current number and title in the view's label.
- It puts "view number/sheet number" in each callout.

## Custom properties and fields

`properties` sets custom properties of the set, or of one sheet with `sheet`. A value of
`null` removes the property. After a sync, these fields read the values:

| field | value |
|---|---|
| `%<\AcSm SheetSet.Name>%`, `SheetSet.Description` | the set's |
| `%<\AcSm SheetSet.<property>>%` | a custom property of the set |
| `%<\AcSm Sheet.Number>%`, `Sheet.Title`, `Sheet.Description` | the sheet's |
| `%<\AcSm Sheet.<property>>%` | a custom property of the sheet |

A drawing that was never synced still gets `Sheet.Number` and `Sheet.Title` from its layout
name. `fields` lists the fields a set offers. `insertField` puts one on a sheet as MTEXT.

## Views and callouts

`view` places a model space named view of the sheet's drawing on the sheet, at a point and a
scale (paper units per drawing unit). It adds a viewport and a label under it ("number title").
`callout` puts text on a sheet that points at a view: "view number/sheet number". Sync keeps
labels and callouts up to date after renumbering. Removing a sheet removes its views and the
callouts that point at them.

## Page setup override

`pagesetup` stores named page setups in the set. A setup is given by its fields (`page`), or
copied from a sheet's layout (`from`). `use` picks the one that publishing applies to every
sheet, or to one subset's sheets with `subset`. `""` means each layout's own page setup. The
drawings are not changed.

## Publish

`publish` plots every sheet into one PDF, with a bookmark for each sheet under its subset.
