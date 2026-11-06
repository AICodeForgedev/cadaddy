#!/bin/sh
# Rebuilds the DWG interop matrix (docs/dwg-interop-matrix.md):
#   1. `interop_write` writes the made-up drawings as DWG R2000…R2018 by both writers into $DIR/ours;
#   2. each program below opens every file and saves it again into $DIR/<program>/<writer>/;
#   3. `interop_compare` reads every copy back into CADaddy and writes $DIR/matrix.md.
#
# Usage: scripts/dwg-interop.sh <empty folder>
# Programs (each skipped when its variable is empty or the command is missing):
#   ODA       ODA File Converter (default `ODAFileConverter`; needs a display, so `xvfb-run -a` is
#             used when there is none): DWG R2018 out -> oda-dwg, DXF R2018 out -> oda-dxf.
#   LC_DWG2DXF  libdxfrw's `dwg2dxf` built from the libdxfrw sources bundled with the LibreCAD
#             release under test (its old command line: `dwg2dxf <in> -v2010 <out>`) -> librecad.
#   DXFRW_DWG2DXF  `dwg2dxf` from current libdxfrw (`dwg2dxf <in> -o <out> -y`) -> libdxfrw.
# A rejected file gets `<stem>.err` next to where its copy would be, holding the program's message.
set -eu
DIR=${1:?usage: scripts/dwg-interop.sh <folder>}
mkdir -p "$DIR"
DIR=$(cd "$DIR" && pwd)
ODA=${ODA-ODAFileConverter}
LC_DWG2DXF=${LC_DWG2DXF-}
DXFRW_DWG2DXF=${DXFRW_DWG2DXF-}

CADADDY_INTEROP_DIR=$DIR cargo test -p cadaddy-engine --test suite_files_plotpdf -- --ignored --exact dwginterop::interop_write

have() { [ -n "$1" ] && command -v "$1" >/dev/null 2>&1; }

if have "$ODA"; then
    run=""
    [ -n "${DISPLAY-}" ] || run="xvfb-run -a"
    for w in native bridge; do
        for t in DWG DXF; do
            out="$DIR/oda-$(echo $t | tr 'A-Z' 'a-z')/$w"
            mkdir -p "$out"
            # in, out, output version, output type, recurse, audit (0: report, don't repair), filter
            $run "$ODA" "$DIR/ours/$w" "$out" ACAD2018 $t 0 0 "*.dwg" >/dev/null 2>&1 || true
        done
    done
fi

# One dwg2dxf per file; a failure leaves its last message lines in <stem>.err.
dwg2dxf_all() { # program-folder, command-style (old|new), binary
    for w in native bridge; do
        out="$DIR/$1/$w"
        mkdir -p "$out"
        for f in "$DIR/ours/$w"/*.dwg; do
            s=$(basename "$f" .dwg)
            rm -f "$out/$s.dxf" "$out/$s.err"
            if [ "$2" = old ]; then
                msg=$(cd "$out" && "$3" "$f" -v2010 "$out/$s.dxf" 2>&1) || true
            else
                msg=$(cd "$out" && "$3" "$f" -o "$out/$s.dxf" -y -d 2>&1) || true
            fi
            if [ ! -s "$out/$s.dxf" ]; then
                rm -f "$out/$s.dxf"
                printf '%s\n' "$msg" | grep -v '^[[:space:]]*$' | grep -i -E 'error|fail|not match|unsupported|warning' | grep -v -E '^(Error reading file|Conversion failed)' | tail -2 > "$out/$s.err" || true
                [ -s "$out/$s.err" ] || echo "Error reading file" > "$out/$s.err"
            fi
        done
    done
}
if have "$LC_DWG2DXF"; then dwg2dxf_all librecad old "$LC_DWG2DXF"; fi
if have "$DXFRW_DWG2DXF"; then dwg2dxf_all libdxfrw new "$DXFRW_DWG2DXF"; fi

CADADDY_INTEROP_DIR=$DIR cargo test -p cadaddy-engine --test suite_files_plotpdf -- --ignored --exact dwginterop::interop_compare
echo "matrix: $DIR/matrix.md"
