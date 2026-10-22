# Labels from the model

Panel directories, device plate labels, equipment and disconnect labels, panel nameplates and arc
flash labels are generated from the building spec every time: the gear (`mep.equipment`), each
panel's schedule (`circuits`: number, description, room, breaker, poles), the devices and the
circuit each is on (`mep.devices`), and the feeders (what each piece of gear is fed from). A
circuit that changes changes its labels; `labels.list` marks every label **new**, **changed** or
**printed** (as last exported), and `labels.export {changed_only: true}` prints only what changed.

| Command | What it does |
|---|---|
| `labels.list {set?}` | Every label, its lines and status; the directories; what's missing |
| `labels.export {path, format?, set?, changed_only?, mark?}` | PDF sheets, PNG pages, a CSV for label printers, or DXF |
| `labels.arcflash {rows \| path}` | The arc flash study's results (CSV columns: equipment or bus, incident energy, working distance, arc flash boundary, PPE, voltage, limited and restricted approach, study, date) |
| `labels.check {review?}` | What the labels can't be made right without; `review` makes each an RFI (a document request) |
| `labels.bom` | The BOM lines the labels need (also on `mep.qty`, part `labels`) |

## The labels

- **Panel directory** (Letter, a page a panel): the panel, its voltage and rating, what it is fed
  from, then every circuit (odd left, even right): number (every pole of a multi-pole breaker),
  description, room or area.
- **Device plate labels** (½" × 1¾", or 12 mm tape from the CSV): `{panel}-{circuit}`.
- **Nameplates** (panels, switchboards) and **equipment labels** (disconnects, transformers),
  2" × 4": tag, voltage, rating, fed from.
- **Arc flash labels** (4" × 6"), one per labeled piece of gear: incident energy at the working
  distance, arc flash boundary, PPE, nominal voltage, approach boundaries, the study and its date.
  The values come from the engineer's arc flash study, imported; **CADaddy doesn't calculate
  them**. Gear without study data gets no label: it is a failure in `labels.check`, and with
  `review` a document request, so the labels are ordered in time.

## Project rules

| Rule | Default | |
|---|---|---|
| `naming/label_case` | `upper` | or `as_is` |
| `naming/room_label` | `RM {room}` | how a room shows in a directory |
| `naming/circuit_label` | `{panel}-{circuit}` | a device plate's text |
| `custom/label_tape_mm` | `12` | the tape the device labels print on |
| `custom/label_cartridge_m` | `8` | tape per cartridge |

## BOM lines

The label printer (a tool), tape cartridges for the device labels (from the labels' length), the
2" × 4" label stock, a typed directory card per panel, and an arc flash label per labeled item
(with the number still waiting for study data).

Tests: `crates/engine/tests/labels.rs`, and `crates/mepfix/tests/elecequip.rs` (a made-up set's
directories against its panel schedules).
