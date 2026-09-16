# `palace-asset`

The Palace asset layer: the `qAst` / `sAst` / `rAst` state machines that move
binary assets between a client and its server (and, for props, between clients),
the request scheduler that paces those requests, and the HTTP fetch for room
backgrounds and hotspot images.

It produces **complete, CRC-verified asset blobs**. Decoding a prop into pixels
is `palace-prop`'s job; compositing those pixels is `palace-render`'s. This crate
knows exactly one thing about prop bytes: which slice the CRC covers.

**Status:** built and validated. 138 tests pass offline in ~1 s, plus one opt-in
live test that has been run successfully against `localhost:9998`.

---

## 1. Where this fits

```
palace-wire   framing [type u32][len u32][ref s32], ByteOrder, opcode table
     │
palace-asset  qAst/sAst/rAst codecs, block assembly + CRC, scheduler,
     │        media HTTP fetch, caches, pipeline coordinator
     │
palace-prop   prop codec — consumes the blobs this crate hands over
```

`palace-asset` depends on `palace-wire` (framing, `Reader`/`Writer`,
`ByteOrder`, the opcode table) and on `palace-prop` for **one** function: the
asset CRC routine and the 12-byte prop header length. Re-deriving the CRC here
would be a second implementation of `ASSET_CRC_MAGIC` that could silently
disagree with the decoder that consumes the result.

---

## 2. The asset protocol as implemented

### 2.1 Message framing

Already in `palace-wire`, reused unchanged:

```
uint32 eventType;    // 4-character ASCII mnemonic, in session byte order
uint32 length;       // body length
sint32 refNum;
uint8  msg[length];
```

The three opcodes this crate owns:

| Opcode | Value | Direction | Meaning |
|---|---|---|---|
| `qAst` | `0x71417374` | both | "send me asset (type, id, crc)" |
| `sAst` | `0x73417374` | server → client | here is the asset |
| `rAst` | `0x72417374` | client → server | here is a prop I already hold |

`HTTP` (`0x48545450`) is handled here as well because the media base URL is
asset-layer state. Room descriptions arrive in `room` and are `palace-room`'s
problem; this crate never parses one.

### 2.2 `qAst` — the request

```text
AssetType  type   uint32     'Prop' / 'User' / 'IUsr'
AssetSpec  id     sint32
           crc    uint32     0 = "don't care"
```

Twelve bytes, `refNum` unused (0). A CRC of 0 means the sender does not care
which revision the receiver has.

**One frame per request.** The reference client's comment — "assets are
requested in packets of up to 20 requests, separated by 500ms to prevent
flooding the server and getting killed" — invites the reading that one frame
carries twenty 12-byte records behind a single 240-byte body. It does not:
OpenPalace writes the frame header *inside* its per-request loop, so a "packet
of 20" is twenty complete 24-byte frames flushed together.

The captures settle it. In `$CORPUS/pcap_extract/animanic_pass2_dump.txt`,
a client packet with a 24-byte TCP payload contains exactly one `tsAq`, a 48-byte
payload exactly two, and a 72-byte payload exactly three — 24 bytes per request,
never a shared header. `encode_batch` therefore returns a `Vec<Frame>`, and the
scheduler paces *flushes*, not frames.

### 2.3 `sAst` / `rAst` — the transfer

Identical bodies; the protocol reference says so outright: *"These two messages
are identical except for the message type."* One codec, parameterised by the
opcode at encode time.

```text
AssetType       type         uint32   'Prop' / 'User' / 'IUsr'
AssetSpec       id           sint32
                crc          uint32
                blockSize    sint32   bytes of data in THIS message
                blockOffset  sint32   where this block starts in the asset
                blockNbr     sint16   0-based
                nbrBlocks    sint16   1-based       (24-byte header so far)
--- present ONLY when blockNbr == 0 ---
AssetDescriptor flags        uint32
                size         uint32   size of the whole asset
                name         Str31    32 bytes: length byte + 31 name bytes
                                        (40 more bytes, 64 total)
data            uint8[blockSize]
padding         uint8[…]
```

Two details are worth calling out because they are where an implementation
breaks:

**(a) The descriptor is conditional.** The first block is 40 bytes longer than
every other block. Getting this wrong desynchronises every multi-block read.

**(b) The padding term.** OpenPalace computes `padding = size - (blockSize + 64)`
using 64 for *every* block. 64 is correct only for block 0. This crate derives
the header length from `blockNbr` (64 or 24) and treats a negative remainder as a
truncation error instead of skipping zero bytes and carrying on.

### 2.4 Multi-block assembly

The format is designed for blocks. The Unix server does not use them: the
protocol reference and Taj both state that in practice every asset arrives whole,
with `blockNbr == 0`, `nbrBlocks == 1` and `blockOffset == 0`. This crate
implements the general case anyway — a fork or a peer-propagation path may use
it, and "assume one block" is how you end up with half an avatar.

`AssetAssembler` stores blocks by index in a `BTreeMap`, so concatenation is in
index order regardless of arrival order. It tracks per-block offsets and rejects
a repeated index that disagrees about where it belongs. A complete asset is
checked against the descriptor's `size` and then against the CRC; failures drop
the partial state and are reported, never retried in place — retrying is the
scheduler's job.

### 2.5 The CRC rule

```
crc = 0xD9216290
for byte in data:
    crc = rotate_left(crc, 1) ^ byte
```

For a **prop**, `data` is the bytes *after* the 12-byte prop header — never the
whole blob. A spec CRC of `0` means "don't care" and is never treated as a
mismatch.

`tests/captured_sast.rs` proves the distinction is meaningful: for every captured
prop the payload CRC differs from the whole-blob CRC, so an implementation that
hashes the wrong slice would fail rather than accidentally pass.

### 2.6 Peer propagation (`rAst`)

The server may ask a *client* for a prop (server → client `qAst`). The pipeline
answers with `rAst` if it holds the asset, and remembers the request if it does
not, serving it as soon as it arrives. `PipelineEvent::Serve` is the seam where
the caller applies the reference client's size gate — it refuses to serve a prop
that is not 44×44, because a larger prop is a "web service prop" the server does
not want. This crate cannot enforce that rule without decoding props, so it
exposes the decision instead of hiding it.

Request matching is deliberately lenient about the CRC: a client that asks with
`crc == 0` gets an answer carrying the server's real CRC, and an exact-key match
would then fail and re-request forever. `note_received_any_crc` and
`AssetCache::peek_by_id` both match on (type, id).

This is not theoretical. The live test asked for prop `1020559380` with `crc == 0`
and the server answered with `0x20051eef`; the pipeline satisfied the request and
cached the asset under the real CRC.

### 2.7 The media server

`HTTP` delivers one NUL-terminated base URL. The capture in
`fixtures/logon-run1/` is a real one, from Balamb Garden:

```
HTTP  https://media.palace.example.info/palace/media
```

Note the **absent trailing slash**. OpenPalace concatenates base and file name
directly, which would yield `…/palace/mediasqoom23.gif`; FreePalace inserts a
separator when one is needed. This crate follows FreePalace: `media_url` is total
and never produces a double slash or a missing one.

---

## 3. The `.png → .jpg → original` fallback chain

A room that names `sqoom23.gif` is asking for whatever the server has — the
author may have converted the background and left the room description alone.
Both reference implementations chase this identically:

| Source | Behaviour |
|---|---|
| OpenPalace `PalaceRoomView.mxml` | on a name matching `/^(.*)\.gif$/i`: `tryPngBG` → on IO error `tryJpegBG` → on IO error `tryRegularBG` |
| FreePalace `Media/Loader.hs` | `isSuffixOf ".gif"` → try `[stem.png, stem.jpg, name]` in order, first success wins |

So the chain fires **only for a `.gif` name** and is `.png` → `.jpg` → original.
Any other name is fetched exactly as written.

That last point matters for hotspot and overlay images: `PalaceHotspotView.mxml`
has no fallback at all, so `fetch_single` requests one URL and no more.

`fallback_chain` is a public, unit-tested function rather than logic buried in a
retry loop, so the rule can be read and asserted directly.

Per-candidate outcomes are all treated as "try the next one", and a total failure
returns the list of URLs tried with the reason for each:

```
AllAttemptsFailed { attempts: [
  "http://media/bg.png (HTTP 404)",
  "http://media/bg.jpg (connection reset)",
  "http://media/bg.gif (200 but empty)",
]}
```

A zero-byte 200 is not a usable image and does not stop the chain.

---

## 4. The scheduler: timing rationale

### 4.1 What the numbers are

| Constant | Value | Source |
|---|---|---|
| requests per flush | **20** | reference comment: "packets of up to 20 requests" |
| flush interval | **500 ms** | reference comment: "separated by 500ms to prevent flooding the server and getting killed" |
| first-flush debounce | 50 ms | reference client debounces so the several props one room asks for travel together |
| attempt timeout | 15 s | not from a reference; chosen so a dead server is detected in a bounded time |
| attempts before giving up | 4 | |
| retry backoff | 500 ms, doubling, capped at 30 s | |

### 4.2 Why it is clamped, not merely defaulted

`pserver` counts message rate and drops connections it judges to be flooding. The
failure mode of over-requesting is not "slower", it is "disconnected". So
`SchedulerConfig::clamped()` — applied by both constructors — forces:

* `batch_size <= 20`
* `batch_interval_ms >= 500`

A caller can make the scheduler **more patient**. It cannot make it faster.
`the_interval_cannot_be_tuned_below_the_compatibility_floor` and
`a_flood_is_impossible_to_configure` assert that, including the degenerate
`batch_size: 0` / `batch_interval_ms: 0` case.

### 4.3 Time is an argument, never a `sleep`

`AssetScheduler::poll(now_ms)` takes the time and returns what to do. The
scheduler never sleeps, never spawns, and never reads a clock. `next_deadline()`
tells a driver when to wake.

That is what makes the pacing exactly assertable. `tests/pacing.rs` drives 45
requests through a `VirtualClock` and asserts

```rust
assert_eq!(flushes, vec![(50, 20), (550, 20), (1050, 5)]);
```

— 20 at t=50, 20 at t=550, the last 5 at t=1050, and every consecutive pair
exactly 500 ms apart. The whole pacing suite runs in under 10 ms of wall time and
cannot go flaky on a loaded machine. The same file asserts no flush ever exceeds
20, that 500 requests produce exactly 500 frames and no more, and that a dead
server produces 4 sends and then one `Failed` — never a hang.

### 4.4 Dedup, cancellation, backoff

* **Dedup** — keyed on `(assetType, id, crc)`; a repeat `request` returns
  `AlreadyPending` and changes nothing.
* **Satisfaction** — an incoming `sAst` cancels the matching request by exact key
  *and* by (type, id) ignoring CRC (see §2.6).
* **Cancellation** — `cancel`, `cancel_if`, `cancel_all`, and
  `AssetPipeline::reset` for a room change or disconnect.
* **Backoff** — a timeout re-queues the request with `min(500 ms · 2ⁿ, 30 s)`, so
  it stays inside the same 20/500 pacing.
* **Bounded** — the queue is capped (`max_queue`, default 4096) and refuses
  rather than growing without limit, emitting a `Dropped` event.

A fresh request is never starved by an unrelated backed-off retry: `request`
clamps the flush timer to `min(existing, now + 50 ms)`.

---

## 5. Cache layout

### 5.1 In-memory asset cache

Keyed by `(assetType, id, crc)` — the wire identity of an asset — with an LRU
eviction order and a default capacity of 512 assets. There is a secondary
`(assetType, id)` index so a `crc == 0` lookup can find an asset whose real CRC
is known.

The reference client bounds the same structure
(`PalaceConfig.numberPropsToCacheInRAM`) for the same reason: a busy room can
reference thousands of props and each one is a full sprite.

### 5.2 On-disk media cache

```
<root>/
  media/
    <8 hex of media-server-URL hash>/
      <sanitised relative name>
```

Concretely:

```
~/.cache/palace/media/1a2b3c4d/bg.gif
~/.cache/palace/media/1a2b3c4d/animated-backgrounds/rainy-day.gif
~/.cache/palace/media/9f8e7d6c/bg.gif        ← a different media server
```

The URL hash is what keeps two servers that both serve `bg.gif` from overwriting
each other; the reference implementations that cache to a flat directory get this
wrong. The hash is FNV-1a over the base URL with a trailing slash trimmed, so
`…/media` and `…/media/` are the same namespace.

Writes go to a sibling `.part` file and are then renamed, so a crash or a
concurrent reader never sees a half-written image.

### 5.3 Name sanitising

Real servers reference nested names — `animated-backgrounds/rainy-day.gif`,
`avatar-editor/avatar-edit-raum.jpg` are both live Balamb paths — so
subdirectories must survive. Everything dangerous does not:

| Input | Result |
|---|---|
| `animated-backgrounds/rainy-day.gif` | accepted, nested path preserved |
| `../etc/passwd` | refused |
| `/etc/passwd`, `\\windows\\system32` | refused (leading separator) |
| `a/./b`, `a//b`, `a/../../b` | refused |
| component > 200 bytes, path > 8 deep | refused |
| `a b:c.png`, non-ASCII | percent-escaped to `a%20b%3Ac.png` |
| embedded NUL | refused |

`sanitise_media_name` is public and `tests/adversarial.rs` throws 20,000
generated hostile names at it, asserting that every accepted result is a relative
path with no `ParentDir`/`RootDir`/`Prefix` component and that every resulting
cache path stays under the cache root.

---

## 6. Bounds and failure behaviour

### 6.1 Allocation ceilings

| Constant | Default | Guards |
|---|---|---|
| `MAX_BLOCK_SIZE` | 8 MiB | one `sAst`/`rAst` block |
| `MAX_ASSET_SIZE` | 64 MiB | a whole assembled asset (descriptor `size`) |
| `MAX_BLOCK_COUNT` | 4096 | blocks per asset |
| `DEFAULT_MAX_PENDING_ASSETS` | 512 | simultaneously incomplete assets |
| `DEFAULT_MAX_PENDING_BYTES` | 32 MiB | total held by incomplete assets |
| `DEFAULT_MAX_MEDIA_BYTES` | 64 MiB | one media response body |

Every one of these is checked **before** a buffer is sized, and `blockSize` is
additionally checked against the bytes that actually arrived, so a frame cannot
declare its way into a large allocation.

That claim is measured rather than asserted by inspection. `tests/adversarial.rs`
installs a counting `GlobalAlloc` and decodes a 64-byte frame that claims a 2 GiB
block, asserting live memory grows by less than 4 MiB:

```rust
assert!(outcome.is_err(), "a 2 GiB block must be refused, not returned");
assert!(grew < 4 * 1024 * 1024, "…; grew {grew} bytes");
```

### 6.2 Timeouts

The media transport carries a connect timeout and a total timeout (`ureq`), and
reads the body through a hard byte limit. A server that accepts the connection
and then says nothing is a `TimedOut` → `AllAttemptsFailed`, not a hang;
`a_server_that_never_answers_times_out_instead_of_hanging` proves it against a
local server that stalls on purpose.

An asset request to a dead server is bounded by the scheduler's attempt timeout
and attempt count: a `Failed` event after four sends, and an idle scheduler.

### 6.3 What is dropped, not crashed

A malformed `sAst`, a CRC mismatch, a size mismatch, an incomplete asset, an
impossible block header, a media file that fails all three candidates, an unsafe
media name, a truncated prop — all are values. There is no `unwrap`, `expect` or
`panic` in the library's non-test code (enforced by
`#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used, clippy::panic))]`
and by `cargo clippy --all-targets -- -D warnings`).

---

## 7. What is validated, and how

### 7.1 Against real captured data

`fixtures/captured/` holds five prop blobs captured from
`chat.animanic.de:9977` and the exact TCP payload length of the packet that
carried each. The blobs were extracted by
`$CORPUS/tools/palace_walker.py::_store_asset` — the known-good Python
parser — from `$CORPUS/props_from_capture/`; the packet lengths came from
`$CORPUS/pcap_extract/animanic_pass2_dump.txt` and `…pass3_dump.txt`.
`capture.json` records the provenance.

The measurement behind those five:

> Of the **332** prop blobs extracted from those captures, **327** satisfied
> `tcp_payload_len == 12 + 64 + blob_size` exactly (179 distinct sizes). That is,
> the padding after a real block is **zero**, and the block-0 header is 64 bytes.
> (`12` is the frame header, `64` is the 24-byte block header plus the 40-byte
> descriptor.) The five that did not match are blobs harvested from a *different*
> session and merged into the same directory, not counterexamples.

That is the padding formula checked against real traffic, as required, rather
than taken on faith from OpenPalace's source. Five of those 327 are the fixtures;
`tests/captured_sast.rs` asserts, for each one:

* `payload_len == 64 + blob_size` and `padding == 0`;
* `blockNbr == 0`, `nbrBlocks == 1`, `blockOffset == 0`, `descriptor.size == blob_size`;
* decode → assemble → CRC verified → cached;
* the assembled blob **decodes as a 44×44 prop under `palace-prop`**, which is the
  actual downstream contract;
* the payload CRC differs from the whole-blob CRC, so the CRC-slice test is
  meaningful.

**Measured:** the `sAst` layout, the 64-byte block-0 header, zero padding, the
`qAst` 24-byte one-frame-per-request shape (24/48/72-byte packets containing 1/2/3
`tsAq` mnemonics), the CRC slice, the media base URL from a real `HTTP` frame.

### 7.2 Against a live server

`tests/live_server.rs` is `#[ignore]`d — the default suite is hermetic — but it
has been run successfully against `localhost:9998`:

```
live media server: https://media.palace.example.info/palace/media
assembled (Prop, 1020559380, 0x20051eef) (434 bytes)
```

The asset it assembled is the *same prop* as the `434`-byte captured fixture, and
the CRC `0x20051eef` was independently computed in Python from the captured bytes
before the test was written. An end-to-end `qAst` → `sAst` → assemble → verify
round trip against a real pserver, including the `crc == 0` → real-CRC matching
in §2.6.

Run it with:

```sh
cargo test -p palace-asset --test live_server -- --ignored --nocapture
```

### 7.3 Against a local HTTP server

`tests/media_http.rs` stands up a real `TcpListener` on an ephemeral port and
drives the **real** `UreqTransport` against it — not a mock, so request
formation, status handling, body reading, redirects and the byte limit are all
genuinely exercised. Ten tests cover: `.png` winning, falling through to `.jpg`,
falling through to the original `.gif`, total failure listing every URL, the disk
cache preventing a second round trip, nested names, a stalling server timing out,
connection-refused, an over-limit body being refused, and `fetch_single` not
chasing extensions.

### 7.4 Synthetic and reference-derived only

Honest gaps:

| Area | Confidence | Why |
|---|---|---|
| Multi-block assembly | **medium** | implemented from the protocol reference; **no capture in the corpus contains a real multi-block asset**. Tested with synthetic blocks (out-of-order arrival, duplicate blocks, offset disagreement, mid-asset restart). |
| `User` / `IUsr` asset types | **low** | no CRC rule is documented for them. `CrcPolicy::VerifyKnown` (the default) accepts them unverified and says so; `Strict` refuses. |
| `sAst`/`rAst` for non-prop types | **low** | the server never sends one in any capture. The codec is type-agnostic, so the risk is confined to the CRC policy. |
| Media fallback for non-`.gif` names | **medium** | both references apply the chain only to `.gif`; verified by reading both, not by a capture of a `.jpg`-named room background going missing. |
| Block `blockOffset` semantics | **medium** | validated only for the single-block case (`offset == 0`); for later blocks the crate rejects a repeated index that disagrees, but does not attempt to prove contiguous coverage. |

### 7.5 Not verified at all

* The live test's `sAst` leg is best-effort: it asserts only that *if* the server
  answers, the assembled asset satisfies its CRC. It cannot assert that the
  server will answer for an arbitrary id.
* No test drives a real socket through `AssetPipeline` at the level
  `palace-client` will; the pipeline's I/O seam (`Vec<PipelineEvent>`) is
  exercised in-process. That integration belongs to `palace-client`.
* HTTPS certificate verification through `UreqTransport` is only exercised
  against plain HTTP locally; the live test talks to a plain TCP socket, not the
  media server. The rustls path is `ureq`'s default and unmodified.

---

## 8. Usage

```rust
use palace_asset::{AssetKey, AssetPipeline, AssetType, PipelineEvent};
use palace_wire::ByteOrder;
use std::time::Duration;

let mut pipeline = AssetPipeline::new();
pipeline.set_byte_order(ByteOrder::Little);   // from the TIYID banner

// A room description named prop 1234. Ask for it.
pipeline.request(AssetKey::new(AssetType::PROP, 1234, 0), now_ms());

// Drive the clock. Wake at next_deadline(), not on a fixed tick.
while let Some(due) = pipeline.next_deadline() {
    // … sleep until `due` (or wake on socket input) …
    for event in pipeline.poll(due) {
        match event {
            PipelineEvent::Send  { frames } => write_all(frames),   // qAst, ≥500ms apart
            PipelineEvent::Serve { frames } => write_all(frames),   // rAst
            PipelineEvent::RequestFailed { key, attempts } => log(key, attempts),
            _ => {}
        }
    }
}

// Frames off the socket go straight in.
for event in pipeline.on_frame(&frame, order, now_ms()) {
    if let PipelineEvent::AssetReady { key, .. } = event {
        let blob: &[u8] = &pipeline.cache().peek(&key).unwrap().data;
        let prop = palace_prop::decode(blob)?;
    }
}
```

Media, with the fixture-backed local-server test as the reference:

```rust
use palace_asset::{MediaCache, MediaConfig, MediaFetcher, UreqTransport};

let cfg = MediaConfig::default();
let mut fetcher = MediaFetcher::new(
    UreqTransport::from_config(&cfg),
    MediaCache::new(cache_root),   // the root is the caller's choice
    cfg,
);
let image = fetcher.fetch(media_server, &room.picture, now_ms())?;
```

---

## 9. Layout

| File | Contents |
|---|---|
| `src/asset.rs` | `AssetType`, `AssetSpec`, `AssetKey`, `AssetDescriptor`, `AssetTransfer` (the `sAst`/`rAst` codec) |
| `src/query.rs` | `AssetQuery`, `encode_batch`, `REQUESTS_PER_BATCH` (the `qAst` codec) |
| `src/assembler.rs` | `AssetAssembler`, `Asset`, `CrcPolicy`, `AssemblyOutcome` |
| `src/scheduler.rs` | `AssetScheduler`, `SchedulerConfig`, `SchedulerEvent`, `VirtualClock` |
| `src/media.rs` | `HttpTransport`, `UreqTransport`, `MediaFetcher`, `fallback_chain`, `media_url` |
| `src/cache.rs` | `AssetCache`, `MediaCache`, `sanitise_media_name` |
| `src/pipeline.rs` | `AssetPipeline`, `PipelineEvent` — the coordinator |
| `src/error.rs` | `AssetError` and the size ceilings |
| `tests/captured_sast.rs` | the real-capture assertions (§7.1) |
| `tests/pacing.rs` | the exact 20/500 timing (§4.3) |
| `tests/media_http.rs` | the local HTTP server (§7.3) |
| `tests/adversarial.rs` | hostile input, counting-allocator bounds, path escapes |
| `tests/live_server.rs` | opt-in live exchange (§7.2) |

---

## 10. Credits

Protocol behaviour cross-checked against OpenPalace (`PalaceClient.as`,
`PalacePropStore.as`, `PalaceRoomView.mxml`, `PalaceProp.as`), FreePalace
(`Media/Loader.hs`), Taj (`Palace/Messages/Structures/Asset*.cs`), ThePalacev0
(`PalaceProtocolRef.txt`) and `$CORPUS/tools/palace_walker.py`. The captured
fixtures come from a live pserver session recorded on 2026-09-09.
