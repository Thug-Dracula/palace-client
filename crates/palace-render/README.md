# `palace-render` — the Palace room compositor

Turns a parsed `MSG_ROOMDESC` plus local assets into a rendered RGBA frame, and
owns the one piece of geometry the whole client depends on: the mapping between
**room coordinates** (where hotspots live) and **viewport pixels** (where mouse
events arrive).

Headless. No Tauri, no windowing, no network, no IPTSCRAE. It takes local files
and blobs and writes PNGs.

```console
$ cargo run -p palace-render -- --room 1672 --out renders/1672.png
$ cargo run -p palace-render -- --frame-file fixtures/logon-run1/frames/0007-server-room.bin --out 901.png
$ cargo run -p palace-render -- --room 1672 --loose-prop 1132272264@150,300 \
      --avatar 460,330,1001545398 --dpr 2 --out demo.png
$ cargo run -p palace-render -- --frame-file .../0007-server-room.bin --viewport 1000x700 --zoom 1.5
```

`--help` lists every flag. `renders/regenerate.sh` reproduces the committed
sample frames.

## The two facts that matter most

1. **A room is not 512×384.** `RoomRec` carries no width or height. The room's
   logical size is **the background image's intrinsic size, floored at 512×384**
   (`viewport::room_size_from_background`). A smaller background is padded; a
   larger one is a larger room. The corpus proves it: room 167 is 1280×720, room
   7022 is 1200×720, room 1024×640. Hard-coding 512×384 renders those rooms at
   the wrong scale.
2. **The compositor works in logical room pixels.** The DPR transform lives
   entirely in `canvas.rs`; presentation-time filtering is somebody else's job.
   That is the scope's decision — *composite in a fixed logical coordinate space,
   then scale at presentation*.

## Modules

| Module | Responsibility |
|---|---|
| `corpus` | find a room by id in a payload directory, or decode a captured frame |
| `assets` | resolve backgrounds/overlays (PNG/JPEG/GIF) and props (`.prp` roster or blob dirs) |
| `build` | apply the room's geometry rules to produce a `Scene` |
| `scene` | the resolved, drawable scene model (no filesystem, no `palace-room`) |
| `compositor` | stack the scene's layers in the reference z-order |
| `canvas` | RGBA frame buffer with the DPR transform and the blitter |
| `viewport` | room ↔ viewport ↔ buffer coordinate maths (forward **and** inverse) |
| `image_clut` | the hotspot-image palette (`imageClutARGB`), generated from the reference |

## Z-order rules

The order below is the child order of `PalaceRoomView.mxml`, which is the
authoritative statement of what the reference client stacks where. Bottom to top:

| # | Layer | Reference element |
|---|---|---|
| 1 | flat backdrop, then the background image at `(0, 0)` in intrinsic pixels | `backgroundImage` |
| 2 | hotspot-state overlays flagged **pictures above nothing** | `hotSpotCanvas` → `imageGroup` |
| 3 | **room dim** (`DIMROOM`) — dims layers 1–2 only | the dim `s:Rect` |
| 4 | draw commands flagged *back* | `PaintLayer(drawBackCommands)` |
| 5 | loose props, in scene order | `loosePropsGroup` |
| 6 | avatars, **y-ordered** | `avatarsGroup` |
| 7 | hotspot-state overlays flagged **pictures above props** | `hotSpotAboveAvatarsCanvas` |
| 8 | draw commands flagged *front* | `PaintLayer(drawFrontCommands)` |
| 9 | name tags | `nameTagGroup` |
| 10 | hotspot-state overlays flagged **pictures above name tags** | `hotSpotAboveNametagsCanvas` |
| 11 | chat text | `textGroup` |
| 12 | hotspot-state overlays flagged **pictures above all** | `hotSpotAboveEverythingCanvas` |

The four overlay bands are not arbitrary: `Hotspot.flags` selects them and
`scene::layer_for_hotspot_flags` reproduces `PalaceHotspot`'s getters exactly
(`0x01` → above all, `0x04` → above props/avatars, `0x40` → above name tags,
otherwise above nothing). Priority follows the reference: above-all wins.

**Only layer 11 (chat text) is not rasterized.** Layers 4 and 8 (draw commands)
are rasterized by `crate::draw`, and **layer 9 (name tags)** is rasterized by
`nametag`, from an embedded Liberation Sans Bold face (see below). No room in the
799-payload corpus declares a draw command, and chat text is font work for the
presentation layer; the layers are listed so the ordering is on record rather
than rediscovered later.

**Image overlays are selected, not drawn wholesale.** A hotspot's *current state*
(`Hotspot.state`) names a picture id; that id selects a `PictureOverlay` from the
room's picture array. Only the current state is visible, and an unreferenced
picture draws nothing. This is the reference behaviour (`PalaceHotspotView.addHotSpot`).

**Loose props are always behind avatars.** The reference puts them in a group
before `avatarsGroup`, and the web client *sparky* gives every loose prop
`zIndex = 50 + index` while avatars start at 100 — so props never interleave with
avatars by depth.

## Coordinate mapping maths

Three spaces (`viewport.rs`):

```text
  room      logical Palace pixels: hotspots, overlays, props, avatars.
            Absolute signed 16-bit pixels in the background's space.
  viewport  CSS / layout pixels — what mouse events arrive in.
  buffer    device pixels of the offscreen frame, floor(room * dpr).
```

```text
  fit     = min(viewport.w / room.w, viewport.h / room.h)
  scale   = fit * zoom                    (ScaleMode::Fit)
  scale   = 1                             (ScaleMode::Native — the 1:1 toggle)
  content = (room.w * scale, room.h * scale)
  offset  = ((viewport.w - content.w) / 2, (viewport.h - content.h) / 2)

  room → viewport :  p * scale + offset          viewport → room : (p - offset) / scale
  room → buffer   :  p * dpr                     buffer → room   : p / dpr
```

`Native` ignores zoom by design: the 1:1 toggle means exactly one room pixel per
viewport pixel whatever the slider says. Zoom is clamped to `[0.5, 3]` and DPR to
`[1, 2]` (`clamp_dpr`, `clamp_zoom`); a non-finite value cannot poison the
transform.

`ViewTransform` exposes all six conversions — `room_to_viewport`,
`viewport_to_room`, `room_to_buffer`, `buffer_to_room`, `viewport_to_buffer`,
`buffer_to_viewport` — plus `content_rect()` (the letterbox) and `buffer_size()`.

### How it is tested

`viewport.rs` carries a round-trip sweep: **5 room sizes × 10 viewport sizes
(incl. letterbox, pillarbox, 16:9, 4K, 320×240, 1×1) × 6 zooms × 5 non-integer
DPRs × 2 modes**, asserting identity to `1e-9` for:

* room → viewport → room,
* viewport → room → viewport,
* room → buffer → room,
* buffer → viewport → buffer.

Plus targeted checks that a click 10 px into the pillarbox maps to **negative**
room x (the classic "clicks land on the wrong hotspot" bug), that the content-rect
corners map to the room corners, that zoom pins the room centre to the viewport
centre, and that degenerate/NaN inputs stay finite.

## The DPR transform

`Canvas::for_room(room_w, room_h, dpr)` allocates
`floor(room_w × clamp(dpr,1,2)) × floor(room_h × clamp(dpr,1,2))` device pixels —
exactly the scope's formula. Every drawing method takes **logical** coordinates
and expands each logical pixel across its device footprint:

```text
  for a logical pixel at x:  dest device columns = floor(x*dpr) .. floor((x+1)*dpr)
```

At `dpr == 1` that is a byte-exact copy (unit-tested); at `dpr == 2` it is a
2×2 replication; at fractional DPR every logical column is still covered with no
gaps (unit-tested at 1.5×). **No source resampling happens here** — the
compositor stays out of the resampling hot path, as required. Presentation picks
nearest for sprites and smooth for photographic backgrounds.

## Room, overlay, prop and avatar geometry

| Rule | Source | Status |
|---|---|---|
| Room size = background intrinsic, floor 512×384 | scope *Resolution model*; `PalaceRoomView.mxml` `minWidth=512`/`minHeight=384` + `scaleContent=false` | **verified** |
| Background drawn 1:1 at `(0,0)` | `backgroundImage` (`scaleContent="false"`) | **verified** |
| Overlay centred on `hotspot.loc + state.pic_loc` | `HotSpotImage.mxml`: `x = hotspot.location.x + state.x - width/2` | **verified** |
| Only the current hotspot state's overlay draws | `PalaceHotspotView.addHotSpot` (`visible = isCurrentState`) | **verified** |
| Loose prop at `loc`, list order | `PalaceRoomView.handleLoosePropAdded` (`image.x = looseProp.x`) | **verified** |
| Dim covers background + above-nothing overlays only | dim `s:Rect` sits after `hotSpotCanvas`, before the prop/avatar groups | **verified** |
| Avatar movement clamped to a **22 px** margin | `PalaceRoomView.updateSelfPosition` (`x < 22`, `x > width-22`) | **verified** |
| Avatar is 44×44; worn props offset by the prop header | `Avatar.mxml` (`height=44 width=44`, `image.x = prop.horizontalOffset`) | **verified** |
| Avatars y-ordered; loose props always behind | sparky `zIndex = round(100 + y/roomHeight * 4999)`, loose prop `zIndex = 50 + index` | **verified (community client)** |
| Avatar anchored at `x-22, y-22` | `linpal` uses `-22`; OpenPalace `Avatar.mxml` and sparky use `-21` | **assumed** — the references disagree by 1 px; see below |
| Horizontal mirror of `imageClutARGB` transparency | `PalaceImageOverlay.processTransparency` | **verified** |
| Exact-RGB transparency match | reference compares through a `0x00AAFF00` mask, which ignores the blue channel (a Flash `threshold` artefact) | **deliberate divergence** |
| Prop id → blob via `.prp` `id → dataOffset` | `PRP-FORMAT.md`; cross-checked against 180,661 roster records | **verified** |
| Name tag geometry, font, glow | `NameTag.mxml`: `x = user.x - 1 - width/2`, `y = user.y + 17`, Arial 12 bold white, `GlowFilter(blur=2, strength=5)` | **verified** (glow is a documented 2 px dilation, not a Gaussian blur) |
| Chat text | — | **not implemented**; draw commands and name tags are (0/799 rooms use a draw command) |

**The 1 px avatar discrepancy.** `linpal`'s C++ client draws the avatar box at
`x-22, y-22` and each prop at `x + hOffset - 22`; OpenPalace's AS3 client and the
web client both draw at `x-21, y-21`. The movement clamp in `PalaceRoomView` uses
22. This crate uses the symmetric **22** so the clamp and the anchor agree, and
flags the divergence rather than hiding it. It is a one-pixel shift of the whole
avatar; if a side-by-side diff against a commercial client ever disagrees by one
pixel, this constant is why.

## Transparency, per asset type

* **Props** are RGBA after decoding (`palace-prop`). The 8-bit path is
  palette-indexed with **no alpha channel**; its transparency is the palette
  entry's own alpha (the M&M table is opaque, so 8-bit props are opaque unless
  the payload's RLE skips pixels). Formats 16/20/S20/32 carry real alpha.
* **Backgrounds and overlays** are ordinary PNG/JPEG/GIF decoded by the `image`
  crate. A PNG or GIF with an alpha channel is used as-is — the reference's rule
  is that *pre-existing transparency wins*.
* **Legacy opaque overlays** get the `PictureRec.transColor` rule
  (`build::apply_transparency_index`): `-1` = no transparency, `0` = "the pixel at
  `(1, height-1)` is the transparent colour", `> 0` = an index into the
  hotspot-image palette. The palette is committed as `image_clut.rs`, generated
  from the reference's `imageClutARGB` (256 entries, all opaque, anchors
  unit-tested).
* **Ghost props** draw at 50 % alpha (the prop header's `GHOST` bit), for loose
  props and avatar parts alike.

## Asset resolution

```text
  background name  ->  MediaStore (indexed base names across media roots;
                       .png/.jpg extension fallback; `dir__file` flattening)
  loose prop id    ->  PropStore  (props_harvested/<id>_*.bin, arks/<id>.ark,
                       or a .prp roster indexed by asset id)
```

The `.prp` container layout (`dataOffset`/`assetMapOffset`, 32-byte records with
`id`, `dataOffset`, `dataSize`) is the one `palace-prop`'s corpus test already
uses, and it is re-verified here against the live 177 MB `pserver.prp`
(180,661 Prop records + 1 Fave).

The CLI's default media roots include the local mirrors of the Balamb Garden
media server (`~/.local/share/PalaceChat/Media`, `$MEDIA/colosseum-bgs`,
`$MEDIA/props`) because the `$CORPUS/` harvest holds
almost no backgrounds — room 901's `sqoom23.gif` lives only in those caches.

**A missing asset is never silently skipped:** a missing/undecodable background
renders the flat backdrop, and a missing/undecodable prop renders a magenta
checker placeholder. Both are reported as `AssetNote`s and printed by the CLI.

## Safety and determinism

* `#![forbid(unsafe_code)]`; `clippy::unwrap_used`/`expect_used`/`panic` are
  **denied** in non-test code.
* The render path never reads the wall clock. Animation is driven by an injected
  `AnimationClock`, so two renders of the same scene are byte-identical.
* Room size is capped at `MAX_ROOM_DIMENSION` (8192). Five corpus rooms park a
  hotspot at `x = 31743` (`0x7BFF`, a sentinel); without the cap a backgroundless
  render of one of those would try to allocate a ~4 GB frame.
* Malformed input cannot panic: `RoomDesc` decoding is already tolerant
  (`palace-room`), and every asset accessor returns `None`/placeholder.

## What renders (evidence)

`renders/regenerate.sh` produced these from the local corpus. Each has a
`.report.txt` next to it with the CLI's own account of what it resolved.

| PNG | What it demonstrates |
|---|---|
| `901-balamb-garden.png` | Room 901 "Balamb Garden" from the captured logon frame: real 512×384 background **and** the `notebar.gif` hotspot-state overlay |
| `167-avatar-editor.png` | Room sized **1280×720** from the background (not 512×384) + a composited overlay |
| `1021-rpg-loading.png` | Exactly 512×384 — the floor case |
| `1672-ludo-ii.png` | Real 1024×640 background + two overlays + one prop the corpus lacks, drawn as a reported placeholder |
| `1672-ludo-ii-props-avatars.png` | Real loose props **and** real avatars over a real background: all four layers |
| `1672-ludo-ii-dpr2.png` | The same scene at `dpr = 2`: a 2048×1280 buffer drawn in logical units |
| `7022-okas-magic-bar.png` | Real 1200×720 background + 15 loose props, all missing → the placeholder policy at scale |
| `90-template-room.png` | 128 loose props over `clouds.gif`; every prop reported as a `MissingProp` |

## Tests

```console
cargo test -p palace-render                 # 93 lib + 1 corpus + 22 CLI = 116
PALACE_ROOM_CORPUS=/path/to/payloads \
  cargo test -p palace-render --test corpus_render -- --nocapture
```

`tests/corpus_render.rs` composites **all 804 records** in the 799-payload corpus
with *empty* asset stores — the hostile case — asserting each renders at a size
≥ 512×384, ≤ the safety cap, exactly `w×h×4` bytes, and that every loose prop is
reported. It skips cleanly when the corpus is absent.

## Not in this milestone

Tauri, windowing, network, asset *fetching*, IPTSCRAE, chat text, prop animation
(the clock is injected but the scene is static), Type 1 avatars, and per-picture
effects. Draw commands and name tags *are* rasterized; name tags only in the fixed
Liberation Sans Bold face — there is no font selection or user-supplied font.
Multi-frame prop animation needs the `ANIMATE`/`BOUNCE` frame sequence, which is a
later milestone.

## Sources

Ported from, and cross-checked against:
`OpenPalace/PalaceClient/src/net/codecomposer/palace/{view,model}/` (layer order,
hotspot bands, overlay centring, avatar geometry, transparency),
`PalaceChat`/`PalacePalette.as` (`imageClutARGB`), `linpal`
(`gtkbigpage.cpp`, prop offsets), the web client **sparky** (y-ordering), and
`$CORPUS/PRP-FORMAT.md` + `pserver.prp` (the `.prp` container). Licensing is
deliberately not a constraint for this non-commercial port; the client as a whole
is GPL-3.0-or-later. The name-tag font is Liberation Sans Bold (metric-compatible
with the reference's Arial), embedded under the SIL OFL 1.1 — see
`assets/LiberationSans-Bold.LICENSE.txt`.
