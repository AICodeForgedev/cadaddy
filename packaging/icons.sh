#!/usr/bin/env bash
# Regenerate the app icons from the vector sources in assets/app-icon:
#   cadaddy-icon.svg        the mark (Windows, Linux, 64/256/1024 PNGs)
#   cadaddy-icon-small.svg  the mark without its dashed lines, for 32 px and below
#   cadaddy-icon-macos.svg  the mark on Apple's icon grid (margin and shadow)
# Needs resvg; the .icns step needs macOS (iconutil). The outputs are committed, so builds and
# packaging never need these tools.
#
#   packaging/icons.sh
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIR="$ROOT/assets/app-icon"
ID="dev.aicodeforge.cadaddy"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
command -v resvg >/dev/null || { echo "error: resvg not found (cargo install resvg)" >&2; exit 1; }

# render SIZE OUT [macos]: the small drawing at 32 px and below, else the full (or macOS) one.
render() {
  local src="$DIR/cadaddy-icon.svg"
  [ "${3:-}" = macos ] && src="$DIR/cadaddy-icon-macos.svg"
  [ "$1" -le 32 ] && src="$DIR/cadaddy-icon-small.svg"
  resvg -w "$1" -h "$1" "$src" "$2"
}

for s in 16 24 32 48 64 128 256 512; do
  mkdir -p "$DIR/hicolor/${s}x${s}/apps"
  render "$s" "$DIR/hicolor/${s}x${s}/apps/$ID.png"
done
render 64 "$DIR/cadaddy-64.png"
render 256 "$DIR/cadaddy-256.png"
render 1024 "$DIR/cadaddy-1024.png"
render 512 "$DIR/cadaddy-macos-512.png" macos

ICO_PNGS=()
for s in 16 20 24 32 40 48 64 128 256; do
  render "$s" "$TMP/ico-$s.png"
  ICO_PNGS+=("$TMP/ico-$s.png")
done
(cd "$ROOT" && cargo run -q -p xtask -- ico "$DIR/cadaddy.ico" "${ICO_PNGS[@]}")

if command -v iconutil >/dev/null; then
  SET="$TMP/cadaddy.iconset"
  mkdir -p "$SET"
  for s in 16 32 128 256 512; do
    render "$s" "$SET/icon_${s}x${s}.png" macos
    render $((s * 2)) "$SET/icon_${s}x${s}@2x.png" macos
  done
  iconutil -c icns -o "$DIR/cadaddy.icns" "$SET"
else
  echo "note: iconutil not found (macOS only); cadaddy.icns left as it was" >&2
fi
echo "icons written to $DIR"
