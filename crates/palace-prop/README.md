# `palace-prop` — the Palace prop (sprite) codec

Reverse-engineered codec for Palace **props**: the small sprites that make up
avatars, loose props, deco objects and doors. Five pixel encodings, one encoder,
and a validation run over 227,874 real props.

This crate knows nothing about the wire protocol or the `.prp` roster container.
Props are a self-contained binary format, so there is no dependency on
`palace-wire`; the only dependencies are `flate2` (zlib) and `png` (debug output).

| | |
|---|---|
| Formats | 8-bit, 16-bit, 20-bit, S20-bit, 32-bit |
| Encoder | S20-bit only (the only format the reference client emits) |
| Corpus validation | 227,874 props, 227,867 decoded, 7 rejected (reference rejects them too) |
| Prop bag | `.pids`/`.props` reader: 3,846 records, 3,569 decode, 277 rejected — exactly the non-CRC records (see §2.13) |
| Differential | 227,874 props vs an ActionScript-reference port and `prop_decoder.py`: **0 disagreements** |
| Panics on protocol data | none, by construction (`#![forbid(unsafe_code)]`-style discipline; every read is bounds-checked) |

---

## 1. Quick start

```bash
cargo test -p palace-prop                 # 174 tests, no corpus needed

# Decode the whole local corpus (about 12 seconds):
cargo run -p palace-prop --release --bin prop-tool -- \
    inventory $CORPUS/pserver.prp $CORPUS/props_harvested

# Differential against the Python oracles (about 7 minutes):
python3 tools/diff_corpus.py $CORPUS/reference/PalacePalette.as \
    <digests-file> <manifest>
```

Library use:

```rust
use palace_prop::{decode, encode_s20_blob};

let prop = decode(&blob)?;                       // never panics on bad input
println!("{} {}x{}", prop.format().name(), prop.image.width(), prop.image.height());
let png  = prop.image.write_png("prop.png")?;    // for eyeballing only
let s20  = encode_s20_blob(&prop.image, prop.header.h_offset, prop.header.v_offset, 0)?;
# Ok::<(), palace_prop::PropError>(())
```

---

## 2. Format specification

### 2.1 Blob layout

```
+--------------------+--------------------------------------------+
| 12-byte header     | six sint16: width, height, hOffset,        |
|                    | vOffset, scriptOffset, flags               |
+--------------------+--------------------------------------------+
| pixel payload      | 8-bit: raw nibble RLE                      |
|                    | 16/20/S20/32-bit: zlib (RFC 1950)          |
+--------------------+--------------------------------------------+
```

There is no magic number, no version field and no length field: the only length
is whatever the caller was given (a roster record's `dataSize`, or a message
body). A `.prp` record stores the blob verbatim, header included.

### 2.2 Header — byte offsets and confidence

All six fields are read with the same byte order; there are no mixed-order fields.

| Offset | Size | Field | Meaning | Confidence |
|---|---|---|---|---|
| `0` | `s16` | `width` | pixels. **44 in every one of the 227,874 props examined.** | **Certain** |
| `2` | `s16` | `height` | pixels. 44 in every prop examined. | **Certain** |
| `4` | `s16` | `hOffset` | horizontal origin offset; negative values are common (`-44`, `-22`, `7`, …). The renderer places the prop at `(hOffset, vOffset)` relative to the anchor point. | **High** |
| `6` | `s16` | `vOffset` | vertical origin offset, likewise signed. | **High** |
| `8` | `s16` | `scriptOffset` | byte offset of the prop's own IptScrae script. **Observed `0` in all 227,874 props**, and the reference client always writes `0`. Treat as reserved. | **Low** — no non-zero value is attested |
| `10` | `u16` | `flags` | format selector plus the animation/type bits, see below. | **Certain** |

### 2.3 Endianness — sniffed per prop, not per session

```rust
let little_endian = data[1] == 0;
```

A little-endian `width` of 44 is `2c 00`, so `data[1] == 0`. A big-endian `width`
of 44 is `00 2c`, so `data[1] == 0x2c`. Because no real prop is more than 255
pixels wide, byte 1 is enough to disambiguate. **All 227,874 corpus props are
little-endian**, so the big-endian path is exercised only by synthetic tests; the
reference clients implement it identically, which is why it is implemented here.

> Note this is genuinely per prop. Unlike the session byte order in `palace-wire`,
> there is no negotiated value to inherit.

### 2.4 Flags

| Bit | Constant | Meaning | Confidence |
|---|---|---|---|
| `0x0002` | `HEAD` | worn on the head | **Certain** |
| `0x0004` | `GHOST` | ghosting effect | **High** (documented identically in every reference) |
| `0x0008` | `RARE` | marked rare in the prop bag | **High** |
| `0x0010` | `ANIMATE` | animated prop | **High** |
| `0x0020` | `BOUNCE` / `PALINDROME` | the animation bounces | **High** |
| `0x0040` | `FORMAT_20BIT` | 6-6-6-2 payload | **Certain** (corpus) |
| `0x0100` | `FORMAT_32BIT` | RGBA payload | **Certain** (corpus) |
| `0x0200` | `FORMAT_S20BIT` | 5-5-5-5 payload | **Certain** (corpus; the format the modern client writes) |
| `0xFF80` | (pattern) | see next section | **Certain** (corpus: 10 records in a rebuilt roster) |

Observed `flags` values in the corpus:
`0x0000`, `0x0002`, `0x0004`, `0x0006`, `0x000a`, `0x0010`, `0x0012`, `0x0016`,
`0x0032`, `0x0036`, `0x0042` (20-bit + HEAD), `0x0102` (32-bit + HEAD),
`0x0200` (S20).

### 2.5 Format selection — the order is load-bearing

```text
if (flags & 0xFFC1) == 0xFF80  ->  16-bit     // first!
else if flags & 0x0200         ->  S20-bit
else if flags & 0x0100         ->  32-bit
else if flags & 0x0040         ->  20-bit
else                           ->  8-bit
```

A 16-bit prop sets **all three** format bits (`0xFF80 & 0x0340 == 0x0300`, and the
pattern leaves bits `0x40`, `0x100`, `0x200` set), so testing 16-bit anywhere but
first misclassifies it as S20 or 32-bit. `0xFFC1` is the union of every
"original palace prop flag" bit; masking with it discards the format bits and
keeps the rest, which is why the test tolerates any combination of
HEAD/GHOST/RARE/ANIMATE/BOUNCE on a 16-bit prop.

`0x0200` must be tested before `0x0100` and `0x0040`; the reference does the same.

### 2.6 Payload: 8-bit — nibble RLE, palette-indexed, **uncompressed**

The payload starts immediately at offset 12. It is **not** zlib-wrapped. This is
the single most surprising fact about the format, and it is verified: all 227,358
8-bit props in the corpus (which include decompressed blobs from six different
rosters) decode as raw RLE.

Each control byte is two nibbles:

```text
bit  7 6 5 4 | 3 2 1 0
     skip      count
```

- `count` palette bytes follow in the stream.
- `skip` columns are left untouched. **Untouched is not palette index 0** — the
  buffer is pre-zeroed, i.e. `0x00000000`, fully transparent. Palette index 0 is
  opaque white, so a skipped column is genuinely "leave whatever is behind".
- **Scanline order is top-down.** The reference loops `y` from `height - 1` down to
  `0` while writing to a buffer whose cursor starts at offset `width`; the two
  cancel out, and the first row in the stream is image row 0. Both independent
  references agree (`pixData[44..]` in OpenPalace, `px[xi, height - 1 - y]` in
  `prop_decoder.py`), and `fixtures.rs` pins it with a prop whose first row is
  entirely skipped.
- A row ends when exactly `width` columns have been accounted for. If control bytes
  overshoot (`x < 0`) the reference marks the prop bad; we return
  `PropError::RleRowOverflow`.
- A `0x00` control byte advances nothing. The reference keeps consuming bytes
  past the end of the payload (a read past the end yields `0` and still advances
  the cursor) until its **6000-control-byte** runaway guard fires and marks the
  prop bad. **We mirror that**: `codec::eight` uses the same 6000-control-byte
  budget (`MAX_CONTROL_BYTES`) and returns `PropError::RleRunaway` with the same
  row the reference reports.
- If the payload ends inside the *final* row's pixel run, the reference's
  `if (data.length > n)` guard leaves those pixels untouched and the prop still
  renders. **We reproduce that tolerance**, because five props in `pserver.prp`
  depend on it (see §7).

Confidence: **high**. 227,351 props byte-identical to two independent oracles.

### 2.7 Payload: 16-bit — 5-5-5-1, zlib

Two bytes per pixel, read **big-endian** regardless of header endianness:

```text
bit  15..11  10..6  5..1  0
     red     green  blue  alpha
```

`R = b0 >> 3`, `G = (word >> 6) & 31`, `B = (word >> 1) & 31`, `A = word & 1`.
Colour fields scale by `255 / 31`; the single alpha bit is 0 or 255.

Confidence: **medium**. The bit layout is confirmed by two references and is
self-consistent (all 16 bits claimed exactly once), but **the corpus contains no
genuine 16-bit prop** — see §8. This decoder is therefore validated only against
synthetic fixtures and the reference source.

### 2.8 Payload: 20-bit — 6-6-6-2, zlib, two pixels per five bytes

```text
byte  bits 7..0
b0    [7..2] red0[5..0]      [1..0] green0[5..4]
b1    [7..4] green0[3..0]    [3..0] blue0[5..2]
b2    [7..6] blue0[1..0]     [5..4] alpha0[1..0]   [3..0] red1[5..2]
b3    [7..6] red1[1..0]      [5..0] green1[5..0]
b4    [7..2] blue1[5..0]     [1..0] alpha1[1..0]
```

Colours scale by `255 / 63`; the two-bit alpha scales by 85 (so 0, 85, 170, 255).
Every one of the 40 bits is claimed exactly once, and the two pixels tile the
group without aliasing — `codec::twenty`'s tests prove that by setting each
individual bit and asserting exactly one output channel changes.

Confidence: **medium-high**. Faithful port, self-consistent tiling, all 127 corpus
props decode to sensible images — but there is **no 20-bit encoder anywhere** to
invert, so unlike S20 there is no round-trip proof of the bit *order*.

### 2.9 Payload: S20-bit — 5-5-5-5, zlib, two pixels per five bytes

```text
b0  [7..3] red0            [2..0] green0 (high 3)
b1  [7..6] green0 (low 2)  [5..1] blue0        [0] alpha0 bit 4
b2  [7..4] alpha0 (low 4)  [3..0] red1 (low 4)
b3  [7]    red1 bit 4      [6..2] green1       [1..0] blue1 (high 2)
b4  [7..5] blue1 (low 3)   [4..0] alpha1
```

Everything scales by `255 / 31`. This is the format a modern client **writes**, and
the encoder and decoder in the reference client are exact inverses:

- the encoder packs `int_comp = R<<19 | G<<14 | B<<9 | A<<4`, emits bytes
  `[23..16]` then `[15..8]`;
- it then masks `int_comp & 0xF0` — **dropping pixel 0's alpha bit 4 into byte 2
  bit 0** and carrying alpha bits 0..3 into byte 3 bits 4..7 of the *next* group —
  and ORs pixel 1 as `R<<15 | G<<10 | B<<5 | A`, emitting bytes `[23..16]`,
  `[15..8]`, `[7..0]`.

Working through that gives exactly the table above, and `codec::s20` +
`encode::encode_s20_payload` reproduce each other bit for bit.

Confidence: **high**. Encoder/decoder round-trip proof, plus 387 corpus props
byte-identical to the reference oracle.

### 2.10 Payload: 32-bit — RGBA, zlib

Four bytes per pixel, `R, G, B, A`, row-major, top-down. Alpha is a full byte, so
this is the only format that can express partial transparency.

Confidence in the layout: **high** (trivially self-evident, two references agree).
Only **2** corpus props use it, both 991 bytes.

### 2.11 Palette

8-bit props index a 256-entry table, exported as `palette::PALETTE` (packed ARGB
`0xAARRGGBB`). It is `clutARGB` from `PalacePalette.as`, and matches the table
hard-coded in Taj's `PalaceProp.cs` entry for entry. Anchors:

```text
index   0 = 0xffffffff (opaque white)   index  54 = 0xffeeeeee
index   6 = 0xffffdfff                  index 229..255 = 0xff000000 (black run)
```

Every entry is opaque, which is asserted by a test — a non-opaque entry would mean
the wrong array had been transcribed. Regenerate with
`tools/gen_palette.py <PalacePalette.as>`; `--check src/palette.rs` verifies the
committed file.

### 2.12 Asset CRC

`.prp` records store a CRC over the blob **excluding** its 12-byte header, and the
server validates it at startup. The algorithm is a seeded rotate-left-then-XOR
byte fold:

```text
seed 0xD9216290
for each byte: crc = rotate_left(crc, 1) ^ byte
```

`crc::payload_crc(&blob) == Some(record.crc)` holds for all 180,661 props in
`pserver.prp`. See `$CORPUS/PRP-FORMAT.md` §4 for the container context.

### 2.13 The prop bag — `PropBag.bundle/`

`src/bag.rs` reads the modern client's own prop collection, a `PropBag.bundle/`
directory holding two flat files:

| File | Meaning |
|---|---|
| `*.pids` | the index |
| `*.props` | the blobs, concatenated |

The catalog's default location is platform-aware (`src/catalog.rs`,
`default_dir`): `PALACE_PROP_BAG` wins when set; on Windows the search is the
first `PropBag.bundle` inside a `PalaceChat*` directory under `%APPDATA%`, then
under `%LOCALAPPDATA%` (the bare `PalaceChat` name first, versioned names such
as `PalaceChat 4` — the older 4.x client — after); on Unix it is
`~/.local/share/PalaceChat/PropBag.bundle`. A missing bag yields an empty
catalog (`None`), never an error.

**Index — confirmed.** `.pids` is a flat array of 16-byte **big-endian** records
`(a: u32, b: u32, offset: u32, size: u32)`. On a live snapshot frozen
2026-09-17 (`.pids` 61,536 B, `.props` 9,361,440 B):

- 61,536 / 16 = **3,846 records, remainder 0**; every `offset + size <= 9,361,440`
  (**0** out-of-bounds);
- offsets are non-decreasing and the blobs **tile the file: 3,845/3,845 adjacent
  pairs satisfy `offset[i] + size[i] == offset[i+1]`**, so `.props` is a pure
  concatenation.

**The 32-byte prefix — verified, and *not* a zero pad.** Every blob is a fixed
32-byte metadata prefix followed by a standard prop, so the prop header begins at
`blob[32]`. The proof is uniform: **3,846/3,846 blobs begin with `00 2c 00 2c` at
offset 32** (a big-endian 44×44 header), and no other offset is uniformly a
header. The prefix bytes are *not* all zero — the early "zero prefix" reading came
from the first two blobs only. On the original 3,844-blob snapshot, only **223**
prefixes were all zero; the rest carry length-prefixed ASCII metadata, e.g.
`07 4e 65 77 50 72 6f 70 00 00 16 54 68 65 20 43 6f 6c 6f 73 73 65 75 6d …`
(`\x07NewProp\x00\x00\x16The Colosseum …`). **The interior layout of the 32 bytes
is not decoded and its meaning is undetermined**; the reader relies only on the
fixed length. Treat the prefix as opaque.

**`(a, b)` identity.** The pair is the same key the client uses for
`BagThumbCache/<a:08X>_<b:08X>.png`.

- `a` is the **asset id**: a real entry has `a = 0x3a3ad1f7 = 976933367`, an id
  that also exists in `pserver.prp`.
- `b` is the **payload CRC** — `asset_crc` over `blob[44..]`, i.e. the prop
  payload after its 12-byte header. It matched **3,569 of 3,846** records.

**Decode rate.** `blob[32..]` decodes with `palace_prop::decode` for **3,569 of
3,846** entries; **277 are rejected**. The rejected set is *exactly* the set where
`b` is not the payload CRC — the two sets are identical, which is a strong
consistency check on the whole reading. The 277 are two shapes:

| Count | Shape | `blob[32]` header | Failure |
|---|---|---|---|
| 209 | built-ins, `a = 0x80000000 + n`, `b = n + 1` | `flags = 0x0100` (32-bit) | `zlib stream rejected` — the payload is not zlib and `b` is a sequence number, not a CRC |
| 68 | large records, `flags` `0x0400`/`0x0800` set | `flags = 0x402`, `0x412`, `0xc02`, … | `RleRowOverflow`/`RleRunaway` — the payload is not a single 8-bit prop; almost certainly a multi-frame/"big prop" container. Structure undetermined. |

The two exceptions do **not** weaken the ordinary reading: for every prop the
decoder accepts, `b` is its payload CRC, and vice versa.

**Independent oracle: the client's own thumbnails.** `BagThumbCache/*.png` is keyed
by the same `(a, b)` pair. On the snapshot, **11/11** cache keys were present in
`.pids`; **10/11 decoded to 44×44 images whose PNGs are pixel-for-pixel identical
to the cached thumbnail** (0 differing pixels over 1,936 each). The 11th
(`A8CD4152_A8CFD339`, 29,236 B, `flags = 0x402`) is one of the 68 multi-frame
records and is rejected by the single-prop decoder; its cached thumbnail is
176×176, exactly 4× the 44×44 grid size.

> The bag is live data. It grew from 3,844 to 3,846 records *during* this analysis;
> all numbers above are for the frozen `/tmp` snapshot named in §9. Never write to
> the client's data directory (`~/.local/share/PalaceChat/` on Linux).

Confidence:

| Aspect | Confidence | Evidence |
|---|---|---|
| 16-byte big-endian index | **Certain** | 3,846 records, 0 out-of-bounds, exact tiling |
| 32-byte fixed prefix | **High** | 3,846/3,846 blobs have `00 2c 00 2c` at +32 |
| `a` = asset id | **High** | matches a `.prp` record id |
| `b` = payload CRC | **High** for ordinary props; undetermined for the 277 exceptions | 3,569 exact matches |
| prefix interior | **Low** | opaque; not decoded |

---

## 3. Corpus validation

`prop-tool inventory` decodes every blob and tallies by format. Sources, all under
`$CORPUS/`:

| Source | Kind | Blobs | Decoded | Rejected |
|---|---|---|---|---|
| `pserver.prp` (= `pserver_merged.prp`) | roster, 180,661 Prop + 1 Fave | 180,661 | 180,654 | 7 |
| `props_harvested/` | loose dumps | 41,478 | 41,478 | 0 |
| `props_all/`, `props_from_live/`, `props_from_capture/`, `props_harvest_final/`, `props_recovered2/3/4/6/`, `props_test/` | loose dumps | 5,735 | 5,735 | 0 |
| **Total** | | **227,874** | **227,867** | **7** |

Format distribution over the whole corpus:

| Format | Decoded | Notes |
|---|---|---|
| 8-bit | 227,351 | 227,358 blobs; 7 rejected |
| S20-bit | 387 | all in `pserver.prp` |
| 20-bit | 127 | all in `pserver.prp` |
| 32-bit | 2 | both in `pserver.prp` |
| 16-bit | **0** | **no real sample exists locally** |

Every blob in `pserver.prp` also passes CRC verification against its roster
record, which independently confirms the blob boundaries and that payloads were
read whole.

### Other rosters in the corpus

| File | Blobs | Decoded | Rejected | Note |
|---|---|---|---|---|
| `pserver_full2.prp` | 178,839 | 178,832 | 7 | same 7 bad props |
| `pserver_src_frozen.prp` | 178,857 | 178,850 | 7 | same 7 |
| `prp_now.prp` | 42,052 | 42,052 | 0 | 8-bit only, older snapshot |
| `pserver_new.prp` | 42,325 | 42,325 | 0 | 8-bit only |
| `BACKUPS_20260912_102742/server_pserver.prp` | 435 | 435 | 0 | the 317 KB worked example |
| `reference/prp-variants/pserver_full.prp` | 178,838 | 42,325 | **136,513** | **corrupt, see §7** |

---

## 4. Differential testing

`tools/diff_corpus.py` decodes every prop with three independent Python
implementations and compares byte-for-byte (as FNV-1a digests over the decoded
RGBA, which is exact):

1. **`tools/oracle_prop.py --style openpalace`** — a line-by-line port of
   `PalaceProp.as`, the ActionScript client.
2. **`$CORPUS/tools/prop_decoder.py`** — the pre-existing, independently
   written 8-bit decoder. **Not modified.**
3. **`tools/oracle_prop.py --style taj`** — a port of Taj's `PalaceProp.cs`,
   reported for information because it is known to diverge.

Results over the full 227,874-prop corpus:

| Comparison | Compared | Match | Mismatch |
|---|---|---|---|
| Rust vs OpenPalace oracle, 8-bit | 227,351 | **227,351** | 0 |
| Rust vs OpenPalace oracle, S20-bit | 387 | **387** | 0 |
| Rust vs OpenPalace oracle, 20-bit | 127 | **127** | 0 |
| Rust vs OpenPalace oracle, 32-bit | 2 | **2** | 0 |
| Rust vs `prop_decoder.py`, 8-bit | 227,346 | **227,346** | 0 |
| Both sides reject | 7 | — | — |

**Zero disagreements.** The 7 rejections are agreement, not failure: the reference
port raises on exactly the same props (§5). The 5-prop difference in the
`prop_decoder.py` row is that oracle's own limitation, explained below.

### The twelve odd props in the corpus

Twelve of the 227,874 blobs do not decode cleanly, and the differential covers
them as a set of their own. All twelve agree with the reference oracle:

| Props | Rust | ActionScript oracle | Result |
|---|---|---|---|
| 5 | decodes, final pixels transparent | decodes, identical pixels | **pixel-identical** |
| 6 | `RleRunaway` (reference guard) at row 43 | `runaway (reference guard)` | same diagnosis, same row |
| 1 | `RleRowOverflow` at row 39 | `row overflow` | same diagnosis, same row |

The six runaways are payloads that end one *control byte* early; the reference
reads zeroes past the end, makes no progress and blows its own 6000-control-byte
guard. The one overflow genuinely walks a scanline off its end. None of the twelve
is silently repaired, and none crashes.

### A fourth check: pre-existing renders

`$CORPUS/sprites_anim/` contains 44×44 RGBA PNGs of props, produced by a
*different* pipeline before this crate existed. Searching the loose dumps for the
source blob of each render finds 15 of them, and **14 match pixel-for-pixel with 0
differing** (`tools/diff_prerendered.py`; the count depends only on which dump
directories are searched). A different decoder, a different palette loader and
a different rendering path all agree, which also independently confirms the
top-down orientation argued for in §2.6 — a flipped decode would have to have been
made the same wrong way by two unrelated implementations.

### Where the oracles disagree with each other, and who is right

| Disagreement | Which is right | Why |
|---|---|---|
| `prop_decoder.py` raises `IndexError` on **5** props that both Rust and the ActionScript reference decode | **Rust / the reference.** | `prop_decoder.py` indexes `data[n]` with no bounds check, so it cannot express the reference's `if (data.length > n)` end-of-payload tolerance. The 5 props end one byte into their final row. |
| Taj's port differs on **126 of 127** 20-bit props and **381 of 387** S20 props (2,782,959 channels total) | **OpenPalace / Rust.** | Taj folds the dither into C# integer division: `const int dither20bit = 255 / 63` is **4**, and `255 / 31` is **8**. That maps a full-scale 5-bit 31 to 248 instead of 255. The ActionScript uses `Number` arithmetic (`255/31` = 8.2258…), which is exactly integer `v * 255 / 31` — and the S20 *encoder* in the same file is the inverse of that, which settles it. |
| Taj's 20-bit decoder loops 967 groups instead of 968 | **OpenPalace / Rust.** | 968 groups × 2 pixels = 1936 = 44×44. Taj's 967 leaves the last two pixels of every 20-bit prop unwritten. |
| Taj has no 16-bit decoder at all | **OpenPalace / Rust.** | `decode16BitProp()` throws `NotImplementedException`. |

The 8-bit and 32-bit paths agree between Taj and OpenPalace, and both agree with
Rust.

---

## 5. Deliberate divergences from the reference

The reference is ActionScript running on a Flash VM, where an out-of-range read
yields `0` rather than an error and an infinite loop is a plausible outcome. This
crate must instead return `Result`. Each divergence is listed here, and every one
of them is covered by a test.

| Case | Reference behaviour | This crate | Why |
|---|---|---|---|
| 8-bit payload ends during the final row's pixel run | writes the pixels it has, leaves the rest untouched, renders the prop | **same** (tolerant) | 5 real props depend on it; erroring would make us *less* capable than the client we are replicating |
| 8-bit payload exhausted, or a `0x00` control byte makes no progress | reads `0` past the end and keeps consuming bytes, hits the 6000-control-byte guard, marks the prop bad | `PropError::RleRunaway` | **same diagnosis and same outcome.** We mirror the past-the-end-as-zero read so the row accounting, the guard and the reported reason all match the client rather than inventing a distinct "truncated" error |
| 8-bit row overruns its width (`x < 0`) | marks the prop bad | `PropError::RleRowOverflow` | same outcome, explicit error |
| 32-bit loop bound | hard-coded `for X in 0..=1935` (1936 = 44×44) | `width * height` | **byte-identical for every 44×44 prop** (all 227,874). The hard-coded bound over-reads a prop that declares fewer pixels and silently truncates one that declares more; deriving it is strictly safer. See §7. |
| 16/20/S20 pixel count | hard-coded 1936 / 968 groups | `width * height`, `ceil(pixels/2)` groups | same reasoning; identical on all real data |
| Decompressed payload longer than needed | reads the pixels it wants, ignores the rest | tolerates up to 4 KiB of surplus, then `DecompressedTooLarge` | keeps trailing padding harmless while bounding a decompression bomb |
| Decompressed payload shorter than needed | reads `undefined` (0) for the missing pixels | `DecompressedTooShort` | a short payload is corruption, not black pixels |
| Header with `width`/`height` ≤ 0 or > 4096 | allocates and then fails inside the VM | `ImplausibleDimensions` | a corrupt header must not drive an allocation |

---

## 6. Quirks and discoveries

- **The 8-bit payload is not compressed.** The task brief and `PRP-FORMAT.md`
  describe the payload as zlib; that is true for 16/20/S20/32-bit only. Evidence:
  227,358 raw-RLE decodes with zero errors, and a zlib header byte check finds no
  `0x78` first byte on any of them.
- **The 16-bit marker is a pattern, not a flag.** `(flags & 0xFFC1) == 0xFF80`
  means "all three format bits set, and nothing else outside the six legacy flag
  bits". Any 16-bit test that uses `flags & 0x0340` or a plain equality against
  `0xFF80` will misclassify the prop.
- **The 8-bit "extra row".** The reference allocates `width * (height + 1)` words
  and starts its cursor at `width`, then copies from index 44. That is a
  one-row-larger buffer plus a hard-coded 44, and it only works because every prop
  is 44 wide. The net effect is the top-down scanline order documented in §2.6.
- **S20 splits a pixel's channels across bytes.** Pixel 0's alpha has one bit in
  byte 2 and four bits in byte 3; pixel 1's red has four bits in byte 2 and one in
  byte 3. It is not a byte-aligned format and no amount of tidying makes it one.
- **The 32-bit 1936 loop count.** 1936 = 44×44 is hard-coded in both references
  rather than derived from the header. We derive it, which is identical on every
  prop in existence (§5).
- **`pserver_full.prp` is corrupt, in a specific and reproducible way.** 136,513
  of its 178,838 records hold a blob whose first 16 bytes are foreign data and
  whose last 16 bytes are missing: for 136,507 of them,
  `blob[16 .. 16+size-16]` is the first `size-16` bytes of the corresponding
  `pserver.prp` prop, and `0` records match the whole prop. In other words the
  writer inserted 16 bytes at the front of the data region and truncated each blob
  to compensate. The remaining 42,325 records are clean 8-bit props (they came from
  `pserver_new.prp`). The file is therefore a rebuilt artifact, not a server's real
  roster, and it is excluded from the corpus totals. It is, however, excellent
  malformed-input data: all 136,513 are rejected cleanly with
  `ImplausibleDimensions` or `Zlib` errors and no panics.
- **Deduplication is not possible in general.** The corpus has props with the same
  `id` and different CRCs, and the same pixels under different ids; props are
  addressed by `(id, crc)`, so nothing here collapses them.

---

## 7. What could not be validated

- **16-bit has no real data.** Zero of the 227,874 corpus props are 16-bit, and
  no other local roster, ark or dump contains one. The 16 positive hits in the
  corpus search were all misaligned reads of `pserver_full.prp`'s corrupt records
  (their flag word came from a neighbouring prop's payload). **The 16-bit decoder
  is therefore implemented from the reference source and covered by synthetic
  fixtures only**, and is tagged `medium` confidence.
- **Big-endian props have no real data.** All 227,874 are little-endian. The
  big-endian path is covered by unit tests against hand-built headers.
- **Non-44×44 props have no real data.** Every prop in every roster is 44×44, so
  the general `width * height` handling is exercised only by tests.
- **`scriptOffset` is always 0.** Its semantics are unverified.
- **GHOST / ANIMATE / BOUNCE rendering.** The bits are decoded and preserved, but
  their *effects* are a renderer's concern, not this crate's.

---

## 8. Crate layout

```text
src/lib.rs           decode(), Prop, re-exports, crate-level format summary
src/bag.rs           PropBag.bundle reader (.pids index + .props, 32-byte prefix)
src/header.rs        12-byte header, endian sniff, flags, format selection
src/palette.rs       256-entry M&M palette (generated, verified by gen_palette.py)
src/crc.rs           asset CRC
src/error.rs         PropError / Result; MAX_DIMENSION, MAX_PIXELS
src/image.rs         PropImage (RGBA8) + PNG export
src/codec/mod.rs     dispatch + the shared zlib step + bit-field scaling
src/codec/eight.rs   8-bit nibble RLE
src/codec/sixteen.rs 16-bit 5-5-5-1
src/codec/twenty.rs  20-bit 6-6-6-2
src/codec/s20.rs     S20-bit 5-5-5-5
src/codec/thirtytwo.rs 32-bit RGBA
src/encode.rs        S20 encoder + quantisation
src/bin/prop-tool.rs corpus tooling (inventory / rgba / rgba-batch / digests / extract / bag)
tests/fixtures.rs    real corpus blobs decoded to pinned pixels
tests/roundtrip.rs   decode -> encode -> decode, including exact quantisation
tests/malformed.rs   fuzz, truncation, mutation, bombs: no panics, always Result
tests/corpus.rs      whole-corpus run, gated on PALACE_PROP_CORPUS
tests/bag.rs         bag reader on synthetic fixtures + a gated real-bag run
fixtures/            real prop blobs + provenance and SHA-256
```

`tools/` (repo root) holds the Python side: `oracle_prop.py` (the two oracle
styles), `diff_corpus.py` (the differential runner), `diff_prerendered.py` (the
pre-rendered comparison) and `gen_palette.py` (the palette generator/verifier).

## 9. Reproducing the numbers in this document

```bash
cd $REPO

cargo test -p palace-prop

cargo run -p palace-prop --release --bin prop-tool -- \
    inventory $CORPUS/pserver.prp \
              $CORPUS/props_harvested $CORPUS/props_all \
              $CORPUS/props_from_live $CORPUS/props_recovered2 \
              $CORPUS/props_recovered3 $CORPUS/props_recovered4 \
              $CORPUS/props_recovered6 $CORPUS/props_from_capture \
              $CORPUS/props_harvest_final $CORPUS/props_test

PALACE_PROP_CORPUS="$CORPUS/pserver.prp:$CORPUS/props_harvested" \
  cargo test -p palace-prop --test corpus -- --ignored --nocapture

# Differential (needs a manifest of prop paths; see tools/diff_corpus.py --help)
python3 tools/gen_palette.py $CORPUS/reference/PalacePalette.as \
    --check crates/palace-prop/src/palette.rs

python3 tools/diff_prerendered.py $CORPUS/reference/PalacePalette.as \
    $CORPUS/sprites_anim $CORPUS/props_harvested $CORPUS/props_all \
    $CORPUS/props_from_live $CORPUS/props_recovered2 $CORPUS/props_recovered3 \
    $CORPUS/props_recovered4 $CORPUS/props_recovered6 \
    $CORPUS/props_from_capture $CORPUS/props_harvest_final $CORPUS/props_test

# Bag section (§2.13). Copy the live bundle to /tmp first: never analyse the live
# directory, and never write into it. Snapshot used above (frozen 2026-09-17):
#   PropBag.bundle/PalaceChat.pids  md5 8540b5db339689e18c8704f1ff892f6d
#   PropBag.bundle/PalaceChat.props md5 f3a8bc31142ea38a8546dbf399ceeb10
cp -a ~/.local/share/PalaceChat/PropBag.bundle /tmp/propbag_frozen

cargo run -p palace-prop --release --bin prop-tool -- \
    bag list /tmp/propbag_frozen            # index metrics + one line per entry

cargo run -p palace-prop --release --bin prop-tool -- \
    bag extract /tmp/propbag_frozen /tmp/prop-bag-verify --limit 10

PALACE_PROP_BAG=/tmp/palacechat_frozen/PropBag.bundle \
PALACE_PROP_BAG_OUT=/tmp/prop-bag-verify \
  cargo test -p palace-prop --test bag -- --ignored --nocapture
```

The gated run needs the bundle and `BagThumbCache` side by side, so the frozen
layout is `/tmp/palacechat_frozen/{PropBag.bundle,BagThumbCache}`:
`cp -a ~/.local/share/PalaceChat/BagThumbCache /tmp/palacechat_frozen/`.

## 10. References

| Source | Used for |
|---|---|
| `$CORPUS/reference/repos/OpenPalace/PalaceClient/src/net/codecomposer/palace/model/PalaceProp.as` | **Primary.** All five decoders and the S20 encoder. |
| `…/PalacePalette.as` | `clutARGB`, the 8-bit palette |
| `…/PalacePropFormat.as` | the format-name enum |
| `$CORPUS/reference/repos/Taj/Palace/PalaceProp.cs` + `Messages/Flags/PropFormatFlags.cs` | Independent C# port; source of the dither/loop-count divergences in §4 |
| `$CORPUS/tools/prop_decoder.py` | Independent 8-bit differential oracle (read-only) |
| `$CORPUS/PRP-FORMAT.md` | The `.prp` roster container, CRC seed and slice rules |
| `$CORPUS/reference/repos/QPalace/`, `$CORPUS/reference/repos/pserver/` | Checked for a competing prop decoder; neither has one (QPalace's matches are IPC-unrelated, pserver only handles the container) |
| `$CORPUS/reference/prp-variants/` | Six roster variants used to widen the 32-bit/20-bit/S20 sample and to characterise the corrupt file |
