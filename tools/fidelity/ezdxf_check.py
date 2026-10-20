#!/usr/bin/env python3
"""Cross-check CADaddy's DXF with ezdxf (MIT), outside CI.

Write the fidelity matrix's DXF files and manifest, then check them:

    CADADDY_FIDELITY_KEEP=/tmp/fid cargo test -p cadaddy-engine --test fidelity_matrix
    python3 -m venv /tmp/ezdxf-venv && /tmp/ezdxf-venv/bin/pip install ezdxf
    /tmp/ezdxf-venv/bin/python tools/fidelity/ezdxf_check.py /tmp/fid

For every DXF: ezdxf reads it, its auditor runs (errors and fixes are listed), and model space
holds the entity types and counts the manifest says CADaddy wrote. Exit status 1 when a file
can't be read, the auditor finds errors, or a type's count differs.
"""

import json
import sys
from collections import Counter
from pathlib import Path

import ezdxf

# How CADaddy writes types other programs don't have, so they read as something sensible: a
# point cloud as a POINT carrying its data in xdata, a region as a closed LWPOLYLINE (CADaddy
# reads both back as what they were).
WRITTEN_AS = {"POINTCLOUDEX": "POINT", "REGION": "LWPOLYLINE"}


def check(path: Path, want: dict) -> list:
    problems = []
    try:
        doc = ezdxf.readfile(path)
    except Exception as e:  # noqa: BLE001 - report any reader failure
        return [f"ezdxf can't read it: {e}"]
    auditor = doc.audit()
    for err in auditor.errors:
        problems.append(f"audit error: {err.message}")
    for fix in auditor.fixes:
        problems.append(f"audit fix: {fix.message}")
    have = Counter(e.dxftype() for e in doc.modelspace())
    expected = Counter()
    for t, n in want.items():
        expected[WRITTEN_AS.get(t, t)] += n
    want = dict(expected)
    # Types ezdxf names differently or splits: a 3D polyline is a POLYLINE; MESH and polyface
    # meshes are POLYLINE in older versions; a 2D solid is SOLID.
    for t, n in sorted(want.items()):
        if have.get(t, 0) != n:
            problems.append(f"{t}: CADaddy wrote {n}, ezdxf reads {have.get(t, 0)}")
    extra = sorted(set(have) - set(want))
    if extra:
        problems.append(f"types ezdxf reads that CADaddy didn't list: {', '.join(extra)}")
    return problems


def main() -> int:
    if len(sys.argv) != 2:
        print(__doc__)
        return 2
    folder = Path(sys.argv[1])
    want = json.loads((folder / "manifest.json").read_text())["model"]
    bad = 0
    for path in sorted(folder.glob("*.dxf")):
        problems = check(path, want)
        print(f"{path.name}: {'ok' if not problems else f'{len(problems)} problem(s)'} (ezdxf {ezdxf.__version__})")
        for p in problems:
            print(f"  - {p}")
        bad += bool(problems)
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
