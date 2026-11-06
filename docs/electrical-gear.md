# Electrical gear: lugs and installability, before the gear is ordered

Panels, switchboards, transformers and disconnects are scheduled equipment (`mep.equipment`, kind
`panel`, `switchboard`, `transformer`, `disconnect`): sized boxes at their tag and mounting height,
the schedule's row as their properties (as text: no load, voltage-drop or code calculations), and
IFC IfcElectricDistributionBoard (DISTRIBUTIONBOARD, SWITCHBOARD), IfcTransformer and
IfcSwitchingDevice (SWITCHDISCONNECTOR). What lands on them comes from the feeder schedule
(`mep.feeders`). The review answers two questions a submittal must: **do the lugs take the
conductors**, and **can the field install it**.

## What the spec holds

- On each piece of gear, its **terminations** (from the gear schedule, the one-line and the approved
  submittal): the point (`main`, `feed_through`, `breaker`, `primary`, `secondary`, `line`,
  `load`, `neutral_bar`, `ground_bar`), lugs per phase or positions on a bar, conductors per lug,
  wire range, `cu`/`al`/`cu/al`, mechanical or compression, supplied with the gear or ordered
  separately (with the bolt diameter, holes per lug and the stack it passes through), and the
  wire-bending space at the terminal.
- Its **enclosure**: usable top and bottom entry plates and the wiring gutter's cross-section.
- Each **feeder**: from and to (and the points it lands on, by default the source's `breaker`,
  `secondary` or `load` and the load's `main`, `primary` or `line`), parallel sets, phases, size and
  material, neutrals and their size, equipment grounding conductors (one per raceway), grounding
  electrode conductors and bonding jumpers landing on the load end's ground bar, raceways (count and
  trade size) and their entry, top or bottom.

## The review (`gear.check`, `gear.review`)

| Check | Fails when |
|---|---|
| lug count | a feeder's sets per phase need more lugs than the point has |
| lug range | a conductor is outside the lug's wire range |
| lug material | aluminium conductors land on CU-only lugs |
| neutral bar | the neutrals landing there (every set of every feeder) need more positions than the bar has, or are outside its range |
| ground bar | the same for equipment grounding conductors, grounding electrode conductors and bonding jumpers |
| entry | the raceways coming in through a plate (each with room for its locknut and bushing) don't fit on it |
| gutter | the conductors in the wiring gutter fill more than its allowed share |
| bending | the wire-bending space at a terminal is less than its conductor size needs |

Each failure says what would fix it: more or different lugs, a larger or second neutral bar, a ground
bar kit, AL-rated lugs, a wider or deeper enclosure, an auxiliary gutter or wireway, a pull or
termination section, splitting the feed top and bottom. `gear.review` writes each failure as an
RFI (`gear_lugs`, `gear_install`) so it is answered before the gear is released for manufacture.

**Lug kits**: lugs ordered separately (compression lugs on a transformer's pads) are listed with
their bolt kits: the bolts (diameter, and length for the stack plus a flat washer each side, a
Belleville washer, the nut and two threads, to the next quarter inch), nuts, flat and Belleville
washers. They are on the MEP takeoff (`mep.qty`, part `lug_kits`) as required items.

**Accepted as submitted**: `gear.accept {tag, check}` records a failure accepted as it is; the
takeoff carries it as a labor condition with a factor on the installation (1.15 for the entry and
the gutter, 1.10 for the rest, multiplied).

## The tables

Raceway outside diameters, the room round each raceway at an entry plate, conductor areas with
insulation, wire-bending space by size and conductors per terminal (entering the wall opposite the
terminal or not), the gutter's allowed fill and the bolts' grade are data with defaults. A project
gives its own in `mep.gear_rules` (`conduit_od`, `spacing_in`, `areas`, `bending_adjacent`,
`bending_opposite`, `gutter_fill`, `bolt_grade`). Check the defaults against the code edition and
the manufacturers' data the project uses.

## 3D

`model.build` builds each piece of gear as a box and each feeder's raceways where they come in
through its entry plates, in rows across the gear: a crowded plate is visible at a glance.

## From the prints

`cadaddy-mepfix`'s generator prints made-up equipment, lug and feeder schedules on the E-sheets
with an answer key (`Truth::gear_problems`: the problems planted); `gear_reader` reads them back
and places the gear at its tags; `link_fixtures` puts the light fixture schedule's columns on every
fixture of its type. Tests: `crates/engine/tests/suite/jobs/gear.rs`, `crates/mepfix/tests/elecequip.rs`.
