# Sheet Sets and Publishing

A sheet set organises multiple drawings and layouts into a numbered, publishable
package. CADaddy stores sheet sets as `.sheetset.json` files (human-readable
JSON, not AutoCAD's binary `.dst` format).

## Create a sheet set

```
SHEETSET new "Project A Plans"
```

Or through the Sheet Set Manager palette (**Tools > Palettes > Sheet Set
Manager**, alias `SSM`).

This creates a `.sheetset.json` file that tracks your sheets, numbering,
properties and publishing settings.

## Add sheets

Each sheet is a layout in a drawing file. Add layouts from one or more drawings
using `execute` with JSON parameters:

```json
execute {"command": "sheetset", "params": {"set": "Project A Plans.sheetset.json", "action": "add", "drawing": "Floor Plan.cdy", "layouts": ["First Floor", "Second Floor"]}}
```

Parameters for the `add` action:

| Parameter | Description |
|:----------|:------------|
| `drawing` | Path to a `.cdy`, `.dwg` or `.dxf` file |
| `drawings` | Array of paths (batch add) |
| `layouts` | Which layouts to add (default: all) |
| `subset` | Assign to a subset (organisational group) |
| `number` | Starting sheet number |
| `title` | Sheet title (default: layout name) |

## Organise with subsets

Group sheets into subsets (like folders):

```json
execute {"command": "sheetset", "params": {"set": "Project A Plans.sheetset.json", "action": "subset", "add": "Architectural"}}
execute {"command": "sheetset", "params": {"set": "Project A Plans.sheetset.json", "action": "subset", "add": "Electrical"}}
```

Move a sheet to a subset:

```json
execute {"command": "sheetset", "params": {"set": "Project A Plans.sheetset.json", "action": "set", "sheet": 3, "subset": "Electrical"}}
```

## Renumber sheets

```json
execute {"command": "sheetset", "params": {"set": "Project A Plans.sheetset.json", "action": "renumber", "start": 1, "prefix": "A", "digits": 3}}
```

Produces A001, A002, A003 and so on. You can renumber a subset independently:

```json
execute {"command": "sheetset", "params": {"set": "Project A Plans.sheetset.json", "action": "renumber", "subset": "Electrical", "start": 1, "prefix": "E"}}
```

## Properties and fields

Sheet sets support custom properties at both the set level and individual sheet
level:

```json
execute {"command": "sheetset", "params": {"set": "Project A Plans.sheetset.json", "action": "properties", "values": {"Client": "Sample Corp", "ProjectNo": "2024-001"}}}
```

These properties are available as field codes inside your drawings:

| Field code | Value |
|:-----------|:------|
| `SheetSet.Name` | The set name |
| `SheetSet.Client` | Custom property "Client" |
| `Sheet.Number` | The sheet's number (e.g. A001) |
| `Sheet.Title` | The sheet's title |

Insert a field in a drawing with the FIELD command or through the MText editor.
Fields update automatically when the set changes.

## Sync

```
SHEETSET sync
```

Writes the set metadata into each drawing file: renames layouts to match sheet
numbers and titles, updates view labels and callout markers. Run sync after
renumbering or changing titles.

## Views and callouts

Place a named model-space view on a sheet:

```json
execute {"command": "sheetset", "params": {"set": "Project A Plans.sheetset.json", "action": "view", "sheet": 1, "view": "Detail A", "at": [5,4], "scale": 48}}
```

Then reference that view from another sheet with a callout:

```json
execute {"command": "sheetset", "params": {"set": "Project A Plans.sheetset.json", "action": "callout", "sheet": 2, "view": 1, "at": [10,8]}}
```

The callout shows the target sheet number and view number, and updates when sheets
are renumbered.

## Page setups

Apply a named page setup to all sheets or a subset:

```json
execute {"command": "sheetset", "params": {"set": "Project A Plans.sheetset.json", "action": "pagesetup", "name": "Full Size D", "use": "Full Size D"}}
```

Each subset can have its own page setup override.

## Publish to PDF

```json
execute {"command": "sheetset", "params": {"set": "Project A Plans.sheetset.json", "action": "publish", "path": "Project A Plans.pdf"}}
```

All sheets are plotted into a single PDF with bookmarks for each sheet and
subset. The page setup for each sheet controls size, orientation and scale.

You can also publish from the CLI:

```sh
cadaddy-cli run --script 'SHEETSET\n' --cmd 'sheetset {"set":"plans.sheetset.json","action":"publish","path":"output.pdf"}' -- drawing.cdy
```

## Limits

| Limit | Value |
|:------|:------|
| Sheets per set | 5,000 |
| Custom properties, views, callouts total | 10,000 |
| Set file size | 16 MB |

## Migrating from AutoCAD sheet sets

AutoCAD's `.dst` files cannot be opened directly. To migrate:

1. Open each drawing from the old set in CADaddy.
2. Create a new sheet set and add those drawings.
3. Re-enter numbering, titles and custom properties.

The drawing content, layouts and viewports carry over through DWG; only the sheet
set metadata needs to be recreated.
