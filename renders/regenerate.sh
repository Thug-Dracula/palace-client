#!/usr/bin/env bash
# Regenerate the sample renders committed under renders/.
#
# Every PNG is produced by the headless CLI from the local corpus, so the set is
# reproducible: run this script and image-diff the output.
#
#   ./renders/regenerate.sh
#
# Asset roots, in priority order (later wins):
#   ~/palace-corpus/reference/media/media_dl, ~/palace-corpus/http_harvest, ~/palace-corpus/arks
#   ~/Pictures/Prop Files/Palace Media, ~/Pictures/ThePalace Props/Media
#   ~/.local/share/PalaceChat/Media, ~/Sync/Colosseum-BGs/all
# These are the local mirrors of the Balamb Garden media server (the corpus
# harvest under ~/palace-corpus holds almost no backgrounds).

set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out="$root/renders"
mkdir -p "$out"
cd "$root"

run() {
  local name="$1"; shift
  echo "== $name"
  cargo run -q -p palace-render -- "$@" 2>&1 | grep -v '^roster' | tee "$out/$name.report.txt"
}

# Room 901 "Balamb Garden" — the canonical room, from the captured logon frame.
# Real 512x384 background (sqoom23.gif) + one hotspot-state overlay (notebar.gif).
run 901-balamb-garden \
  --frame-file fixtures/logon-run1/frames/0007-server-room.bin \
  --out "$out/901-balamb-garden.png"

# Room 167 "Avatar-Editor" — a room larger than the classic size: a real
# 1280x720 background with a composited hotspot-state overlay.
run 167-avatar-editor \
  --room 167 --out "$out/167-avatar-editor.png"

# Room 1021 "RPG Loading" — exactly 512x384, the floor case.
run 1021-rpg-loading \
  --room 1021 --out "$out/1021-rpg-loading.png"

# Room 1672 "Ludo II" — a real 1024x640 background, one resolvable overlay and
# one prop this corpus does not have, which draws as a placeholder and is
# reported (the "never silently skip an asset" policy).
run 1672-ludo-ii \
  --room 1672 --out "$out/1672-ludo-ii.png"

# Room 1672 again, with real prop sprites: three loose props and three avatars
# placed by hand. Demonstrates background -> overlays -> loose props -> avatars
# with real art, and the avatar y-order (the avatar at y=330 draws in front).
run 1672-ludo-ii-props-avatars \
  --room 1672 \
  --loose-prop 1132272264@150,300 \
  --loose-prop 1162331078@220,320 \
  --loose-prop 1275954151@290,300 \
  --avatar 420,300,1020504091 \
  --avatar 460,330,1001545398 \
  --avatar 500,300,995674750 \
  --out "$out/1672-ludo-ii-props-avatars.png"

# Same scene at devicePixelRatio 2: the buffer is floor(W*2) x floor(H*2) and all
# drawing still happens in logical room units (2048x1280).
run 1672-ludo-ii-dpr2 \
  --room 1672 \
  --loose-prop 1132272264@150,300 \
  --avatar 460,330,1001545398 \
  --dpr 2 --out "$out/1672-ludo-ii-dpr2.png"

# Room 7022 "[Oka's Magic Bar]" — a real 1200x720 background and 15 loose props,
# none of which are in the local roster, so the placeholder path is exercised at
# scale.
run 7022-okas-magic-bar \
  --room 7022 --out "$out/7022-okas-magic-bar.png"

# Room 90 "Template Room" — 128 loose props and no background anywhere, so the
# room is rendered on the flat backdrop with its geometry intact.
run 90-template-room \
  --room 90 --out "$out/90-template-room.png"

echo
echo "wrote $(find "$out" -maxdepth 1 -name '*.png' | wc -l) PNGs to $out"
