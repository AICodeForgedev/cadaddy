//! `mep-fixtures [--seed N] [--count K] [--out DIR] [--hotel [--small]]`: write fixture sets
//! `DIR/seed-N/` (sheets as PDF and DXF, `set.pdf`, `truth.json`). `cargo xtask mep-fixtures`
//! runs it. `--hotel` generates a 4-storey hotel; `--small` makes it smaller (4 rooms/side).

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut seed = 1u64;
    let mut count = 1u64;
    let mut out = PathBuf::from("target/mep-fixtures");
    let mut hotel = false;
    let mut small = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut value = || args.next().unwrap_or_default();
        match a.as_str() {
            "--seed" => seed = value().parse().unwrap_or(seed),
            "--count" => count = value().parse().unwrap_or(count).clamp(1, 1000),
            "--out" => out = PathBuf::from(value()),
            "--hotel" => hotel = true,
            "--small" => small = true,
            "-h" | "--help" => {
                println!("mep-fixtures [--seed N (1)] [--count K (1)] [--out DIR] [--hotel [--small]]");
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("mep-fixtures: unknown argument `{other}` (see --help)");
                return ExitCode::FAILURE;
            }
        }
    }
    let size = if small {
        cadaddy_mepfix::hotel::HotelSize::Small
    } else {
        cadaddy_mepfix::hotel::HotelSize::Full
    };
    for s in seed..seed.saturating_add(count) {
        let set = if hotel {
            match cadaddy_mepfix::generate_hotel(s, size) {
                Ok(set) => set,
                Err(e) => {
                    eprintln!("mep-fixtures: hotel seed {s}: {e}");
                    return ExitCode::FAILURE;
                }
            }
        } else {
            match cadaddy_mepfix::generate(s) {
                Ok(set) => set,
                Err(e) => {
                    eprintln!("mep-fixtures: seed {s}: {e}");
                    return ExitCode::FAILURE;
                }
            }
        };
        let dir = out.join(format!("seed-{s}"));
        if let Err(e) = set.write_to(&dir) {
            eprintln!("mep-fixtures: {}: {e}", dir.display());
            return ExitCode::FAILURE;
        }
        let t = &set.truth;
        if hotel {
            println!(
                "seed {s} (hotel {}): {} sheets, {} rooms, {} walls, {} segments, {} terminals, {} equipment, \
                 {} elec devices, {} FA devices, {} telecom, {} casework, {} spec items, {} site elements, \
                 {} gaps, {} clashes ({} planted) -> {}",
                if small { "small" } else { "full" },
                t.sheets.len(), t.building.rooms.len(), t.walls.len(),
                t.segments.len(), t.terminals.len(), t.equipment.len(),
                t.elec_devices.len(), t.fire_alarm_devices.len(), t.telecom_devices.len(),
                t.casework.len(), t.spec_items.len(), t.site_elements.len(),
                t.gaps.len(), t.clashes.len(), t.clashes.iter().filter(|c| c.planted).count(),
                dir.display()
            );
        } else {
            println!(
                "seed {s}: {} sheets, {} duct and {} pipe segments, {} fittings, {} terminals, {} equipment, \
                 {} elec devices, {} panels, {} circuits, {} clashes ({} planted) -> {}",
                t.sheets.len(),
                t.segments_of("duct").count(),
                t.segments_of("pipe").count(),
                t.nodes.iter().filter(|n| ["elbow", "tee", "cross", "transition", "reducer"].contains(&n.kind.as_str())).count(),
                t.terminals.len(), t.equipment.len(),
                t.elec_devices.len(), t.elec_panels.len(), t.elec_circuits.len(),
                t.clashes.len(), t.clashes.iter().filter(|c| c.planted).count(),
                dir.display()
            );
        }
    }
    ExitCode::SUCCESS
}
