# Grounding and bonding in the model and the takeoff

The grounding electrode system, the bonding and the telecom grounding are modelled as real routed
elements with quantities (`mep.grounding` in the building spec), as the one-line, the grounding
details, the riser and the notes show them. **Sizes are as drawn: CADaddy does no sizing
calculations.**

- **Electrodes** and their grounding electrode conductors (GEC) from the service (`service`,
  default the switchboard): metal water pipe (at the water point of entry), concrete-encased
  ("Ufer"), ground rods, ground ring, building steel.
- **Bonds**: water and gas piping, other metal piping, structural steel, the main and system bonding
  jumpers, separately derived systems (transformers), telecom (`from` the service or a busbar `to`
  a busbar, gear, point of entry or electrode), racks and tray.
- **Busbars**: the telecom main grounding busbar (TMGB) and each data room's (TGB).
- **Requirements**: what the drawings call for (`required`: kind, to, the note and its sheet).

| Command | What it does |
|---|---|
| `grounding.entries` | The water and gas points of entry: given (`entries`), else found from the cold water and gas runs (a dead end of the piping on its lowest level, nearest the building's edge); and whether the clamp can be reached (not above a gypsum ceiling) |
| `grounding.route {all?}` | Routes every conductor: up to the level's routing height (above its ceiling), across, along the riser, to its end; risers count in the lengths |
| `grounding.check {review?}` | What the drawings call for that isn't modelled or routed; clamps that can't be reached; data rooms (DATA, IDF, MDF, TELECOM in the room's name) without a busbar or its bonding conductor; references to nothing; the Ufer against its note and its pour; `review` makes each an RFI |
| `grounding.alternative {electrode, kind?}` | A missed Ufer modelled as two 10 ft rods (or a ring) instead, with its takeoff |
| `grounding.takeoff` | The required items (also on `mep.qty`, part `grounding`) |

## The concrete-encased electrode

Modelled to its dimensions inside the footing: its conductor's length, size and kind (bare copper,
or bonded rebar by diameter), the concrete cover, whether that part of the footing is in direct
contact with earth, its direction, and its stub-up connection. The check compares them with the
drawings' note (`ufer_note`; typical: at least 20 ft, #4 bare copper or ½" rebar, 2" of cover —
check the drawings and the code edition). The footing's pour is a **hold point**: while it is
`pending` the check lists it to sign off in 3D before the concrete; once `poured` without
`documented: true` it is a failure, and `grounding.alternative` models the alternative electrode
with its added takeoff.

## The takeoff

Conductor feet by size (the routes, risers included, plus makeup at each end; the Ufer's bare
conductor and a ring by their lengths), water and gas clamps, ground rods and test wells, busbars by
size and the two-hole lugs landing on them, the connections — exothermic welds (with their moulds,
weld metal and the igniter), irreversible compression connectors (with the crimper) or mechanical
clamps, by project rule — bonding jumper lugs, bonding bushings and conduit for GECs run in metal
raceway.

Project rules: `custom/ground_connection` (`exothermic`, `compression`, `mechanical`),
`custom/ground_test_wells` (`yes`/`no`), `custom/ground_makeup_ft`.

## 3D

`model.build` builds the conductors along their routes (an inch thick, to be seen), rods, the Ufer in
its footing, rings and busbars on the `3D-GROUNDING` layer.

Tests: `crates/engine/tests/grounding.rs` (a made-up hotel corner), `crates/mepfix/tests/grounding_hotel.rs`
(the synthetic hotel's own cold water piping and IDF).
