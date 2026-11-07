# COBie and handover schedules

At handover the owner gets the building's assets as data: **COBie** (Construction Operations
Building information exchange, the open spreadsheet standard; version 2.4's sheets and columns)
and room, door and equipment **schedules**, both as Excel workbooks.

**File ▸ Export ▸ COBie...** and **File ▸ Export ▸ Handover Schedules...**, or the commands:

| Command | What it does |
|---|---|
| `cobie.export {path, author?, company?, project?, facility?, category?, phase?, currency?, created_on?, zones?}` | The COBie workbook, checked before it is written (the result lists any problems) |
| `cobie.validate {path}` | Check a COBie workbook, ours or another program's |
| `handover.schedules {path, schedules?}` | Rooms, Doors and Equipment sheets |

The model is the drawing's IFC objects (an IFC model opened with `ifc.open`, see
`docs/ifc-viewer.md`). A model CADaddy built (WALLGEN, `model.build`) goes through its own IFC
export first, so both are read the same way; the drawing itself is not changed.

## What goes on each sheet

| Sheet | From |
|---|---|
| Contact | The author (`author`, an email; COBie keys contacts by email) and the people the BCF issues name |
| Facility | The building (its IFC name, else `facility`, else the drawing's name), its site, the project; metres |
| Floor | The storeys bottom up: elevation, height to the next |
| Space | Each IFC space: its storey, height, gross and net area (the model's `Qto_SpaceBaseQuantities` when it states them, else the footprint of its geometry), category (`Pset_SpaceCommon.Category`) |
| Zone | The `zones` given: `[{name, category?, spaces: [space names]}]` |
| Type | One per type the components use (an untyped object's class names it): maker and model (`Pset_ManufacturerTypeInformation`), nominal size, fire rating, Fixed or Moveable (furniture) |
| Component | Each maintained asset: doors, windows, equipment, terminals, fixtures, furniture, and anything in a system; not the building's fabric (walls, slabs, structure, coverings) or loose duct and pipe runs. Its room: the one the model states, else the one it stands in, else (a door) the rooms either side |
| System | The IFC systems and their components |
| Attribute | Every property set value of spaces and components (not quantities) |
| Coordinate | Each space's and component's box (`box-lowerleft`, `box-upperright`) |
| Issue | The drawing's BCF issues: who raised it, its priority, the component or space it is about, its owner, the last comment |
| Assembly, Connection, Spare, Resource, Job, Impact, Document | Their columns, empty |

Names are unique within a sheet (a repeat becomes `Light (2)`). Where nothing applies a cell
reads `n/a`, as COBie asks; no required cell is left blank.

## The rules `cobie.validate` checks

- Contact, Facility, Floor, Space, Type and Component sheets are present; each sheet's columns
  start with COBie 2.4's, in order.
- Required columns hold a value (`n/a` counts): every sheet's Name, CreatedBy and CreatedOn, and
  per sheet the rest (Space: Category, FloorName, Description; Component: TypeName, Space,
  Description; …).
- Names are unique per sheet (attributes and coordinates per the row they describe).
- CreatedBy is on the Contact sheet; Contact emails are emails; CreatedOn is
  `yyyy-mm-ddThh:mm:ss`; the Facility sheet holds one row.
- References: a space's floor, a component's type and spaces, a system's components, a zone's
  spaces, a spare's or job's type, an issue's owner, and the sheet and row that attributes,
  coordinates, documents, impacts and issues describe. `n/a` where a reference is required is a
  warning.
- Numbers (floor elevation and height, space areas and height, type sizes, coordinates) and pick
  lists (floor category, asset type, coordinate category, units).
- A type no component uses is a warning.

## Schedules

- **Rooms**: room, storey, area (m²), height, doors, equipment, GlobalId.
- **Doors**: mark, type, storey, the rooms it opens between, width, height, fire rating, GlobalId.
- **Equipment**: tag, class, type, maker, model, systems, room, storey, GlobalId (the components
  that aren't doors or windows).

Tests: `crates/engine/tests/suite/files/cobie.rs` (a made-up hotel corner written as IFC4 in the test; every
sheet checked; fifteen ways a workbook breaks, each caught at its sheet, row and column; the
schedules; a model built with WALLGEN).
