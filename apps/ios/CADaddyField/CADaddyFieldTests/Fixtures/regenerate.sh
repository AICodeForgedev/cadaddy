#!/bin/sh
# Regenerates SyntheticSet/ from the real exporter (bim.export.ar {set: true}) on the made-up two-storey
# building of crates/engine/tests/suite/jobs/arexport_set.rs. Never edit SyntheticSet by hand.
# The .usdz files are left out (the app reads the .glb); building.json still lists them.
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../../.." && pwd)
cd "$root"
CADADDY_FIELD_FIXTURE="$here/SyntheticSet" cargo test -q -p cadaddy-engine --test suite_jobs the_field_apps_fixture_comes_from_this_exporter
