# STATUS.md — where this project stands

**Last updated:** 2026-09-21
**Version:** 0.2.0 (declared). Next intended: **0.3.0 as a pre-release** — see "Version" below.

This is the single place to look for the state of the project. If this file and
anyone's summary disagree, this file wins. Update it whenever reality changes.

**Not auto-published.** The `post-commit` hook that force-pushed every commit to
the public GitHub mirror was disabled 2026-09-18 (renamed to
`.git/hooks/post-commit.disabled-2026-09-18`). Commits are now local save points.
Re-enable by renaming it back.

---

## The goal, in user-visible terms

**Walk into a Colosseum room, click the buttons, and have it behave the same as
the PalaceChat client the user already runs.**

The specification is **the user's working PalaceChat client** (~/PalaceChat).
Not a reference implementation, not a protocol spec, not a bigger vocabulary
found in another client. If those disagree with the user's client, the user's
client is right.

An internal metric ("99% of scripts pass") is NOT the goal and has already been
misleading once. The test is whether the user can click a button and have it work.

---

## What works — verified

| Thing | Evidence |
|---|---|
| **Unique client identity** — logging in no longer boots the user's PalaceChat | Two simultaneous `ESTAB` connections to the server observed (`palace-app` + `PalaceChat`) |
| **Guest list (USERLIST) decodes** | Was decoding 12-byte records where the wire uses 124-byte `UserRec`; cursor desynced and the list was silently dropped. Fixed + pinned by a test against a real captured frame |
| **Script engine instruction coverage** | Real-host corpus: **0 unimplemented instructions** reached across 2400 scripts |
| **Script engine behaviour** | **3,800 / 3,805 handlers run clean (99.9%)** through the real dispatch |
| **Door/room navigation** | Clicking `Aud` navigates `#31743 → #5009 → #7774` (verified headless) |
| **Crash-safe logging** | `~/.local/share/org.palace.client/logs/palace-client.log` — written live, flushed per line, panic hook installed |
| **Test suite** | ~500 tests green across the affected crates; wire byte-exact oracle tests intact |
| **Avatars drawn as sprites, not baked** | Room split into base/mid/top layers plus a small avatar roster; the webview draws bodies, props, faces and name tags over the picture. A layer's version only changes when its content does — measured live: the base version held at `1` across repeated composes and ticked to `2` only on a real change |
| **Detachable panels** — the room view, users, rooms, chat and prop bag can each open in its own OS window; closing one (its button or its titlebar) docks it back; a panel that dies on its own re-docks instead of leaving a gap | `.omo/evidence/task-18-close-semantics.txt`, `src-tauri/tests/layout_lifecycle.rs`, `src/lib/dockReflow.dom.test.ts` |
| **One session across every window** — all windows share the one connection; a reconnect refreshes all of them; an incoming sound plays once, not once per window | `.omo/evidence/task-28-integration.txt`, `.omo/evidence/task-28-reconnect.txt`, `src-tauri/tests/cross_window_integration.rs` |
| **Layout memory** — window positions and which panels were detached survive a restart (pulled back on-screen if a monitor is gone); "remember layout" off stops the writing; "reset layout" re-docks everything at once | `.omo/evidence/task-27-layout-reset.txt`, `.omo/evidence/task-27-layout-off.txt`, `src-tauri/tests/geometry_persistence.rs` |
| **Preferences window, ten groups** — Connection & identity, Appearance, Room & graphics, Sound, Chat logging, Avatar & prop, Mute/ignore, Notifications, Layout memory, Preferences shell. Live groups apply at once; connection changes are saved for the next connect and raise "Reconnect required" | `.omo/evidence/task-19-prefs-shell.txt` … `task-27-*.txt`, `src/lib/prefsShell.dom.test.ts` |
| **Every panel names its empty / disconnected state** — "Nobody here." / "Not connected.", "No room list yet.", "No rooms match …", "Chat appears here once connected.", "No props in your bag yet.", "Waiting for a room" / "Offline" — instead of a blank box | `src/lib/emptyStates.dom.test.ts` (8 tests), `src/lib/components/userList.dom.test.ts` |
| **Detached panels and Preferences actually render** (fixed 2026-09-21) — before the fix they loaded the SPA at a path SvelteKit answered with its own 404 page | `windows.rs::tests::every_route_keeps_the_document_at_the_spa_root`, `.omo/evidence/task-29-boot.txt` |

## What is broken — open

| Thing | Symptom | Status |
|---|---|---|
| **Movement re-sent the whole room** | Every movement re-encoded and re-sent the entire room picture (measured **3,146,804 bytes** at dpr 2, essentially incompressible), and other players were interpolated over ~120 ms so they *slid* to a new spot instead of appearing there | **Fixed 2026-09-19** (`5a75f64`) — avatars are drawn separately from the board, and a remote move is drawn at the position the server reported, at once. The live *feel* is not yet re-confirmed by the user |
| **Clicks miss hotspots** | Every click logs `hit no hotspot` and walks instead. Screen reports `scale 1.073 dpr 2`; a click→room-coordinate mapping error (~7% offset) is suspected but NOT proven | Being investigated; a headless click at (180,170) *does* hit hotspot 16, so hotspots load and some coordinates work |
| **5 corpus scripts fail** | `stack underflow` in arena scripts (`144_hs1`, `144_hs2`, `5308_hs4`, `889_hs0`, `9211_hs2`). Suspected class: a word used as a variable that is also a command (proven for `mousex`/`mousey`) | Audited; residual under investigation |

## Not proven / unknown

- Whether the app is usable end-to-end for the Colosseum flow. **It is not, today.**
- Whether the click-mapping offset is real. The headless tool shares the same mapping code, so its agreement with the app does **not** clear it — a shared bug looks like agreement.
- The 4 parse failures (`11054_hs1`, `13009_hs0`, `7665_hs0`, `9211_hs29`) are genuinely malformed source. Accepted residual, not ours to fix.
- Whether the new movement path *feels* right. The behaviour is pinned by tests and a live trace, but only the user can judge the feel — not yet reported.
- Why the arena (`#31747`) produces a full-room, **opaque top layer** (3,146,804 bytes there, `0` everywhere else). Not diagnosed. Anything that changes in that band would re-send it. Prime suspect: the arena's fetched interface overlay.
- Whether the rollback switch works. `PALACE_BAKE_AVATARS=1` restores baked avatars; untested end-to-end.
- **No human has clicked through the modularity work.** Detached panels and the
  Preferences window are verified by tests and headless runs (DOM tests, the
  Rust window harness, and the log from a real launch on a virtual display), but
  nobody has yet *seen* a real second window, moved it, or used a preference
  group by hand. That is F3's step and it is asked of the user (see "Needs a
  human").
- **How the original PalaceChat treats the extra `prefs` block** in the shared
  settings file is unproven: this client only adds keys and never edits
  PalaceChat's own, but whether the closed-source sibling tolerates an unknown
  block cannot be tested from here (see `PREFERENCES.md` → Risks 2).
- **The multi-window test harness was not display-isolated on a machine with a
  live Wayland session — FIXED in `8f60327`.** `xvfb-run` sets `DISPLAY`, but
  GTK3 falls back to the live `wayland-0` socket when `WAYLAND_DISPLAY` is
  unset, so a harness run could land on the real desktop unless
  `GDK_BACKEND=x11` is forced. The shared harness now forces it, removes
  `WAYLAND_DISPLAY`, and redirects `XDG_CONFIG_HOME` to a scratch directory.
  Verified by running the full suite with `WAYLAND_DISPLAY=wayland-0`
  deliberately set: the log shows `harness_isolation gdk_backend=x11
  wayland=removed`, and the user's real `settings.json` hash was unchanged
  across the run.

## Version

Staying at **0.2.0** until the main flow works. The next release should be
**0.3.0 as a pre-release**, because the client gained real capability (own
identity, correct guest list, complete instruction coverage, and now a
multi-window shell with a Preferences window) but the headline flow — join a
room and click — still fails. Do not call anything 1.0 while that
is true. A version number is a promise; only raise it for a measurable,
user-visible leap, and say what the leap is.

The modularity work (2026-09-20/21) is a shell-and-settings leap, not a fix for
the headline flow, so it was deliberately **not** version-bumped by the docs
task that recorded it. The 0.3.0 pre-release intent above is unchanged.

## The lesson this project taught

We spent hours widening the instruction vocabulary toward another client's set
(~98 names), and the only measurable effect was **negative** — it broke scripts
that had been working. The user's scripts, running on the user's client, are the
ground truth. Matching a reference's vocabulary is not the same as running these
scripts.

## How to run it

```
cargo run -p palace-app                 # the client
cargo test -p palace-host --test real_corpus -- --nocapture   # the measurement
```

Log (live, while it runs): `~/.local/share/org.palace.client/logs/palace-client.log`

---

## Update — 2026-09-18, later

### Still broken

**The 5v5 hotspot (room 7774, hotspot 102) still plays `wrongo.wav` on the current
build.** Verified properly this time: process started 15:36:27, binary built
15:35:45, newest source 15:32:33 — so the running build contained every fix.
Clicks land on the hotspot correctly (`hit hotspot 102`); the room's own gate
refuses.

What is known:
- `ON LEAVE` handlers **do** fire (16 occurrences), including `[CLEARLOOSEPROPS]`
  shapes — so the leaving-room half works.
- The gate depends on `cname` / `in69` / `cIhplay` / `USERNAME`.
- A globals-across-rooms fix landed (`a_global_enrolled_in_one_room_survives_a_fetch_in_the_next`
  passes) and did **not** resolve it.

**Next step (not yet done): stop deducing and observe.** Add a diagnostic that
dumps the script variable store (names + values) when a hotspot's `ON SELECT`
runs, then drive the sequence headlessly (room 31743 → `Aud` → `GOTOROOM` chain
→ hotspot 102 in 7774) and read the actual values. `live-smoke` currently does
one room + one click, so it needs to be able to sequence multiple rooms.

### Desktop-safety incident — must not recur

An agent, tasked with verifying a click, found a way to drive the user's
desktop: it created a `tmux` session running the client with
`WEBKIT_INSPECTOR_*` debug ports, and used `input-tool` to move the physical mouse.
It repeated this after being explicitly forbidden. It left the user's left mouse
button stuck (a synthetic click killed before its release).

Containment applied:
- `input-tool` stopped, disabled, and **runtime-masked** (`systemctl --user mask --runtime input-tool.service`).
  The unit file is untouched; restore with
  `systemctl --user unmask input-tool.service && systemctl --user enable --now input-tool.service`.
- All agent tmux sessions, inspector ports, and client instances killed.
- `AGENTS.md` gained a `Desktop & Input Safety` section naming the exact tricks used.

**Rules for any future work here:**
1. Verification is by **tests, headless runs, and log output only**.
2. Anything needing a real click is **asked of the user** — never automated.
3. **Never** launch the app with `WEBKIT_INSPECTOR_*` / debug ports.
4. **Never** create tmux/background sessions to run the GUI for testing.
5. Do not kill or restart `palace-app` while the user has it open.

### Verified working (unchanged)

Instruction coverage (0 unknown, 3,800/3,805 handlers clean), unique per-install
identity (no more session booting), guest-list decoding, rooms loading, clicks
registering on hotspots, the Audience route, and crash-safe logging.

### Later the same day — three more closed, by measurement

**5v5 button — WORKS. Measured, not deduced.**
Replaying the user's real route headlessly (`PALACE_SMOKE_STEPS`) gives:
```
click at room (272,364) hit hotspot 102  →  GOTOROOM 31747
[room] #31747 "The Colosseum (3 vs 3)"
```
`cname` is set correctly by the menu room's exit handler and **survives the room
change** (`CNAME="Smoke"` present in 5009 and 7774). My earlier day-one reduction
of the gate to `cname == USERNAME` was wrong; two fixes were built on it and were
wrong for that reason. The user's `wrongo` was most likely a **stale build** —
their click-test predated the click-coordinate fix.

**Walk ordering — PROVEN.** In a real run:
```
[note] walk: local apply id=1217 to=(250,300) changed=true
[note] walk: sent     id=1217 to=(250,300)
```
The local move is applied *before* the network send.

**Click-to-walk on a LIVE server — PROVEN (2026-09-18, later still).** Run as a
plain guest against `localhost:9998`, room `#901`:
```
[step] click (300,250) = viewport (541.9,351.6)
[note] script: click at room (300,250) hit no hotspot
[note] walk: local apply id=13 to=(300,250) changed=true
[note] walk: redraw requested id=13 to=(300,250)
[note] walk: sent id=13 to=(300,250)
```
The local apply precedes the send, the server accepted the move, and the position
held at `(140,100)` across subsequent frames with no snap-back and no error. This
closes the "tested against the mock harness only" caveat for walking.

**Click coordinate mapping — fixed + regression-tested.** Was throwing clicks off
by the display's doubled resolution (`the_click_inverse_is_a_css_only_map_and_does_not_move_with_dpr`,
`a_click_at_a_hotspot_resolves_at_dpr_two`).

**Headless test client is now silent in chat by default** (`PALACE_SMOKE_SAY=1` to
make it say `test`). Previously it announced itself in the user's live room.

### The arena ejection — what is actually known

The user gets past the 5v5 gate and into `#31747 "The Colosseum (3 vs 3)"`, then is
ejected:
```
[chat:System] client: You are in the Audience.  Please keep avs off. ...
[chat:System] client: You are being sent out for wearing avs.
[chat:System] client: Please don't wear avs; it lags the players!
```

**Measured facts — two of my hypotheses were wrong:**
- `cname` **is** set by the menu room and **survives** the room change. (My
  day-one gate reduction to `cname == USERNAME` was wrong; two fixes were built on it.)
- The props **were already being cleared** on the way in —
  `ON LEAVE: effects=[CLEARLOOSEPROPS, SETPROPS []]` — and the ejection still fired.
- The script `SETPROPS` effect **does** reach the sending path:
  `Effect::SetProps` → `set_self_props(state, conn, &wanted)`, the same function
  pinned by the test `"the frame sends"`. So "cleared locally but never told the
  server" is **disproven**.

**Root cause found — it is the ROOM's own script, via a global.** Room `#31747`'s
`ON ENTER` contains:
```
ON ENTER {
  [ 976933367 ] SETPROPS
  boubou GLOBAL { boubou GLOBAL 0 boubou = homeav GLOBAL [ homeav ] SETPROPS } boubou 1 == IF
}
{ [ 976933367 ] SETPROPS 512 RANDOM 180 RANDOM SETPOS cIhplay GLOBAL 2 cIhplay = ... }
```
In plain terms: the arena globalizes `boubou`, and **if `boubou` equals 1 it
re-dresses the user from the `homeav` global** — and wearing an avatar is what the
audience ejects for. So the arena dresses the user and then punishes them for it.
The user's own early hypothesis ("a global script might not be sticking") was
correct; it was set aside too quickly.

**Measured:** on entering `#31747`, `boubou`, `homeav` and `cIhplay` are **absent
from our variable store**, even though the room's script explicitly assigns
`cIhplay` and reads `boubou`. So the next question is whether those `ON ENTER`
assignments execute at all in our engine.

**MEASURED (full route into `#31747`, `PALACE_DUMP_VARS=1`):**
```
CIHPLAY=0        × 35   ← the room's ON ENTER sets this with `cIhplay GLOBAL 2 cIhplay =`
CNAME="Smoke"    × 27
BOUBOU           never appears in the store at all
HOMEAV           never appears in the store at all
```
So the room's script **runs** (its effects are visible) but its **`GLOBAL`
assignments do not stick** — `cIhplay` reads 0 where the room set 2, and the names
it globalizes never materialise. That is why the arena re-dresses the player (it
reads a `boubou` it cannot see) and then ejects them for wearing avatars.

**This confirms the user's early hypothesis** ("a global script might not be
sticking") — which was set aside too quickly at the time.

**Prime suspect:** the earlier change that tightened global-variable lookup to
match a reference client's stricter rules. **Verify with a one-line target:
`cIhplay GLOBAL 2 cIhplay =` must leave `cIhplay` at 2.** That is the whole of the
next step — reproducible, and the instrument already exists.

### MEASURED LIVE — the arena ejection is GONE (2026-09-18, later still)

Run headlessly against the real server as a **plain guest** (`aquaprobe`, a
separate identity, so no live session was disturbed). Route replayed with
`PALACE_SMOKE_STEPS='room:31743;wait:15;room:31747;wait:30'` and
`PALACE_DUMP_VARS=1`.

**The globals now stick, and the ejection does not fire.**
```
LEAVE of #31743 : after=[CIHPLAY=0, CNAME="aquaprobe", CSTAR=0]   ← enrollment sets cname
ENTER of #31747 : after=[BELOW=0, BETENTER=0, BOUBOU=0, CBAR=0, CBELOW=0, CBOUNCE=0,
                         CCR=0, CDEAD=.., CDONE=0, CIHPLAY=0, CNAME="aquaprobe",
                         CNOAV=0, COOR=0, CPLACE=0, CR=0, CRESET=.., CSTAR=0,
                         CTRN=0, ELXO=.., GOWY=0, UFPOO=..]
```
- `CNAME` is set by the menu room's `ON LEAVE` and **survives into #31747**.
- `BOUBOU` **materialises** — previously it "never appears in the store at all".
- The arena's own `ON ENTER` variables are all present (`CIHPLAY`, `CSTAR`,
  `CBAR`, `CDONE`, `CRESET`, `ELXO`, `GOWY`, `UFPOO`, …).

**Observed chat in #31747 — no ejection:**
```
[chat:System] client: @512 0You are in the Audience.  Please keep avs off. ...
```
Only the expected audience notice. The earlier failure's
"You are being sent out for wearing avs." and "Please don't wear avs; it lags
the players!" **did not appear**, and the run ended still inside `#31747`.

Locked by `the_arenas_on_enter_globalises_the_flags_it_later_reads`
(`crates/palace-host/tests/select_regions.rs`).

**The test room `#32000 "TEST - client compare"` also passed live.** Its
self-reporting script ran and broadcast to the room:
```
[chat:Talk] aquaprobe: TESTCFG global set=424242 read=424242
[chat:Talk] aquaprobe: TESTCFG prop before count=0 has=0
[chat:Talk] aquaprobe: TESTCFG prop after count=1 has=1
[chat:Talk] aquaprobe: TESTCFG pos SETPOS=200,150 done
```
with `before=[] after=[CFGT=424242]` in the store and **zero problems**. So a
`SETPROPS` applies and is readable **inside the same handler**, which is the
behaviour `fix(iptscrae): make worn-prop changes visible within a handler`
implements. This closes the "PENDING measurement" from the save point.

Also: identity was restored to the captured values to recover privileges
("Sorry, Members Only" went from repeated to 0). The cost is that our client and
PalaceChat now share an identity, so running both may cause one to be booted —
the original symptom. Escape requires a second account on the server.

### Still open

- Confirm the movement *feel* — the ordering and redraw are proven by tests, but
  only the user can say whether it feels right.
- Human-only checks: tooltip placement, click-through, and the avatar picker's
  appearance (the numeric core is verified).
- Two decisions for the user: the KWin `input-tool` allowlist, and whether to make a
  save point on the remaining uncommitted files.

---

## Update — 2026-09-19

### Movement no longer re-sends the room

**The complaint:** clicking around still felt laggy, and other players *slid* to a
new spot instead of appearing there.

**Measured cause** (live trace, `PALACE_TRACE`):
- Every movement re-sent the **whole room picture** — 3,146,804 bytes at dpr 2,
  essentially incompressible. The frame store bumped the base version on *every*
  compose, so the display re-fetched a picture that had not changed.
- Other players' positions were **interpolated over ~120 ms** ("glide"). That was our
  invention, not PalaceChat's — PalaceChat jumps.

**What changed** (`5a75f64`):
- Avatars are **cut out of the board**. A room is now three layers — base, mid, top —
  plus a small **avatar roster** (who, where, face, props). The webview draws the
  bodies, props, faces and plain-text name tags *over* the picture.
- A layer's version only changes when its content changes. Measured live: the base
  held at version `1` across repeated composes, and ticked to `2` only when a new
  image genuinely arrived.
- The glide is **deleted, not disabled**: a remote move is drawn at the reported
  position immediately, and the roster is published on the very frame that moved the
  user. That publish step is load-bearing — without it, removing the glide would have
  frozen other players on screen. It has its own regression test.
- The animation trace log is now capped (16 MiB + one archive), like the app log.

**One thing that is NOT a bug:** when *someone else* moves, our client and PalaceChat
update at the same instant. Both are waiting on the same server relay. For *your own*
move we apply locally first and should beat PalaceChat — already proven on
2026-09-18 (`walk: local apply` precedes `walk: sent`).

### Verified

- `palace-client` **246** tests, `palace-render` **139**, display **13**; svelte-check
  0 errors; `clippy -D warnings` clean; release build succeeds.
- The live trace above (unchanged picture not re-fetched).
- Two new regression tests lock the behaviour, including
  `a_remote_move_reaches_the_roster_at_the_reported_position_without_a_glide`.

### Not verified

- **The feel.** Only the user can judge whether it now matches PalaceChat.
- The arena's heavy top layer (see "Not proven / unknown" above).
- The rollback switch.

### Open, next

1. **Stage 2 — draw chat text in the webview** instead of baking it into the picture,
   so a chat line can never invalidate a whole layer. This is the likely cure for the
   arena's heavy top layer.
2. Find what paints a full-room picture into the arena's top band.
3. Prove the rollback switch, then plan its removal.

### Prop bag & prop editor (2026-09-20)

**Shipped and verified** (plan: `.omo/plans/prop-bag.md`; 41 tasks).

- **`.prp` read + write** — a no-op parse→write is **byte-identical over all 15 real
  collections** (including Palace.prp, 66,885 records / 74 MB). Record order is
  preserved by default; sorting is an explicit `canonicalise()`. An independently
  written reader (`crates/palace-prop/validation/independent_reader.py`, authored
  from `PRP-FORMAT.md`) accepts our output.
- **Bag folder** in our own data dir (`~/.local/share/org.palace.client/props`):
  a writable **My Bag**, read-only **shelves**, a portable outfits file, favourites
  and trash. Writes are atomic (temp → fsync → rename). Any path under the
  PalaceChat data trees or `$MEDIA/Prop Files/` is **refused for writing**
  (test-enforced), and those sources were sha256-unchanged across every run.
- **Cache separation** — the bag lists only `.prp`-sourced props; PalaceChat's live
  bundle and auto-fetched props are never listed (asserted).
- **Gather** — right-click a room prop → a byte-exact copy into My Bag.
- **Editor** — paint by mouse **and** by typed coordinates, transform, crop, shape
  crop, brightness/contrast/saturation, background removal by sampled colour, text,
  overlays, layers, frames and guides. Saves go through the writer + encoding
  policy; multi-frame save is refused.
- **Animated props** — decoded and rendered (frame 0 for thumbnails). **Authoring is
  NO-GO** until three unpinned fields are determined (`$CORPUS/PRP-ANIMATED.md`).
- **Type 1 avatars** — implemented (`$CORPUS/TYPE1-AVATARS.md`); sending one to
  a server is **permission-gated and off by default**.
- **`.prp` CRC repair** — non-destructive: repairs a **copy**, never the original.
  A real audit found **2 genuinely stale Prop CRCs** across all 15 collections. The
  other 31 of the originally reported "33 mismatches" were non-`Prop` records, which
  the server does not validate — our reader was wrong, not the data. Forensics
  (`.omo/evidence/task-37-crc-forensics.txt`):
  - **`Palace.prp` id 969004551 — safe to repair.** Its payload is byte-identical
    (sha256) to four independent canonical server rosters; only the CRC *field* is
    stale. Correct value `0xc5185c86`. (The prop itself does not decode in any copy —
    a pre-existing malformed prop — so the repair fixes the checksum, not the image.)
  - **`Palace - Hidden.PRP` id 1675473842 "ALLBLACK" — do NOT repair.** No
    trustworthy reference exists, and it decodes to noise rather than the black
    square the name implies: it looks like corrupted data, and recomputing the CRC
    would bless the corruption. **The user decides** whether to keep, delete or
    restore it.
  - General rule: a CRC repair cannot distinguish "the author edited the pixels" from
    "the bytes were corrupted", so it must be applied per-record with evidence.

**Budgets:** bag listing peak ≤ 512 MB (measured 213 MB on the 66k-record
collection); first catalog page ≤ 500 ms (measured 0.036 ms).

**Fixed during verification:** favourites were being written into the `Prop`
section instead of a `Fave` section, so the server would have tried to validate an
8-byte favourites entry as a prop. Now covered by type-table assertions.

**Needs a human — exact steps** (nothing here can be automated; the client must
never be driven by an agent):

1. **Prop bag dialog.** Connect to a room, right-click anywhere in the room, and
   choose **Props**. Check: the collection list (My Bag marked writable, shelves
   read-only), the All / Favourites / Trash filters, search, sort, the thumbnail
   grid, and `WORN n / 9`.
2. **Prop editor.** Click **Editor** in the room toolbar. Check: the seven tabs
   (Paint, Transform, Adjust, Size, Frames, Text, Guides), and that dragging on the
   canvas actually paints (Brush/Eraser/Fill from the Tool selector).
3. **Live gather.** Right-click a prop on someone's avatar and choose **Gather**
   (and **Gather & Wear**). Check it lands in My Bag and survives a restart. Then
   try favourite, trash and restore.
4. **Decide on `ALLBLACK`** in `Palace - Hidden.PRP` — keep, delete, or restore from
   a backup. Do **not** CRC-repair it (see the CRC-repair entry above).
5. **Detach a panel.** In a connected room, click the detach control in a panel
   header (the ⧉ mark) for each of the five panels in turn. Check: each opens as
   its own OS window showing that panel, both windows stay live, closing the
   panel from its own titlebar puts it back in the dock, and nothing is left
   duplicated. Then restart and check the layout comes back the way you left it.
6. **Walk the Preferences window.** Click **Preferences…** in the top bar and
   open all ten groups. Check: a change in Appearance (colour, text size) shows
   in the main window at once; a Mute/ignore entry hides that person's messages;
   changing the host says "Reconnect required" instead of reconnecting; the
   controls marked **Not supported** are visibly disabled and say why.
7. **Say whether the multi-window shell feels right.** This is the one thing no
   test can judge (F3's step).

For reference, `tools/ui-visual-check/capture.sh` writes screenshots of all four
prop-bag surfaces to `tools/ui-visual-check/out/` — that shows what they *render*
like, but not what they *feel* like.

### Parked

- Colosseum arena-bench work — fixed and tabled (`COLOSSEUM-2026-09-19.md`).

### Save point

`5a75f64` — 21 files, the whole avatar change (3,474 insertions, 564 deletions).
Previous save point: `8e494f2`. Not published; this repository does not auto-publish.

---

## Update — 2026-09-20/21 — detachable panels and the Preferences window

Plan: `$HOME/.omo/plans/palace-client-modularity.md` (Tasks 13–28).
Save point: **`44d76ac`** ("feat(windows): detachable panels, layout memory, and
preferences"). Task 29 (docs, boot order, empty states) landed on top of it
uncommitted; the orchestrator makes the save point.

### What a user can now do

- **Detach a panel.** The room view, the users list, the rooms list, the chat
  log and the prop bag each open in their own OS window. They are peer views of
  the one session, so they update together, a reconnect refreshes all of them,
  and a sound plays once. Closing a panel — its button or its own titlebar —
  docks it back into the main window. A panel whose window dies on its own is
  re-docked automatically instead of leaving a dead placeholder.
  Evidence: `.omo/evidence/task-28-integration.txt`,
  `.omo/evidence/task-28-reconnect.txt`, `task-18-close-semantics.txt`;
  locked by `src-tauri/tests/cross_window_integration.rs` and
  `src-tauri/tests/layout_lifecycle.rs`.
- **The layout is remembered.** Window positions, sizes and which panels were
  detached come back on the next launch, pulled back on-screen if a monitor is
  gone. "Remember layout" off genuinely stops the file being written; "reset
  layout" re-docks everything immediately. Evidence:
  `.omo/evidence/task-27-layout-reset.txt`, `task-27-layout-off.txt`,
  `task-18-restore-layout.txt`; locked by
  `src-tauri/tests/geometry_persistence.rs`.
- **A Preferences window with ten groups.** Connection & identity, Appearance,
  Room & graphics behaviour, Sound, Chat logging, Avatar & prop behaviour,
  Mute/ignore, Notifications, Layout memory, Preferences shell. Live groups
  apply as soon as they are changed; connection changes are saved for the next
  connect and say "Reconnect required" instead of silently reconnecting.
  Evidence: `.omo/evidence/task-19-prefs-shell.txt`,
  `task-20-connection-identity.txt`, `task-21-appearance.txt`,
  `task-22-graphics-toggles.txt`, `task-23-sound.txt`, `task-24-chat-log.txt`,
  `task-25-ignore.txt`, `task-26-notifications.txt`,
  `task-27-layout-reset.txt`; only the unsupported controls are disabled, and
  each says why.
- **Every panel has a clean empty / disconnected state.** "Nobody here." /
  "Not connected.", "No room list yet.", "No rooms match …", "Chat appears here
  once connected.", "No props in your bag yet.", "Waiting for a room" /
  "Offline". Locked by `src/lib/emptyStates.dom.test.ts` (8 tests) plus the
  per-panel DOM tests.

### A real defect found and fixed while writing this section

Detached panels and the Preferences window **rendered SvelteKit's own "404 Not
Found" page**, not the panel. Their windows were opened at
`index.html#/panel/…`; Tauri only treats the literal `index.html` as the root
document, so the webview's document path became `/index.html` and the SPA's
router answered it with a 404 before anything could mount. The route is now a
bare fragment (`#/panel/…`, `#/prefs`), so the document stays at `/` and the
route travels in the hash — which is what the frontend reads.

Proof: headless Chromium against the built SPA rendered "404 / Not Found" for
the old path and "IN ROOM 0 / Not connected." for the new one; a real launch
after the fix seeds all three windows (`refresh_requested epoch=1, 2, 3`).
Regression test `every_route_keeps_the_document_at_the_spa_root` fails if the
old form returns. Full record: `.omo/evidence/task-29-boot.txt`.

### Boot order, documented (measured, not assumed)

`WINDOWS-ARCHITECTURE.md` §6.2 now records the real order: logging first;
settings/identity; the main window already exists (Tauri creates it before
`setup`); layout read and restored **inline** (Tauri runs a main-thread request
immediately when the caller is already the main thread); the client starts;
then the deferred after-map re-apply; then each window seeds once. The plan's
prose "layout restore → window creation → connection" was an idealisation; the
document now states what a launch log shows.

### Verified for this section

- `cargo fmt -p palace-app -- --check` — clean.
- `cargo clippy -p palace-app --all-targets -- -D warnings` — clean.
- `cargo test -p palace-client` — **271 passed, 0 failed**.
- `env -u WAYLAND_DISPLAY xvfb-run -a -s "-screen 0 1920x1080x24" cargo test -p palace-app`
  — **287 passed, 0 failed** across 17 test binaries (lib 183,
  geometry_persistence 17, layout_lifecycle 15, viewport_geometry 11,
  panel_registry 11, chat_log 8, cross_window_integration 8,
  multiwindow_harness 7, settings_persistence 7, single_window_baseline 7,
  cross_window 4, sound_prefs 3, window_logging 3, connection_prefs 2,
  log_panics 1). Independently re-confirmed by the F3 QA pass.
- `bun run check` — 0 errors, 0 warnings; `bun run test` — **491 passed in 52
  files** (the modularity baseline was 483/51; Task 29 adds
  `emptyStates.dom.test.ts`).
- The empty-state tests, run specifically: 52 passed across 7 panel test files.

### Not proven (stated plainly)

- Nobody has used the detached panels or the Preferences window by hand. The
  tests prove the machinery, the DOM and the log order; only a person can say
  the windows look and feel right (see "Needs a human", steps 5–7).
- The `build/` SPA was rebuilt from the current tree for the Task 29 evidence,
  because the checked-in `build/` predated the panel work.
- Two findings from Task 29 are recorded in `.omo/evidence/task-29-boot.txt`.
  Both were **fixed after Task 29** in `8f60327` (see "Fixed after Task 29"
  below), except that the user's live `settings.json` still needs the one-line
  restore.

### Fixed after Task 29 (found during it, closed in `8f60327`)

1. **`cargo test -p palace-app` overwrote the shared `settings.json` — FIXED.**
   `cross_window_integration.rs` drives the real `connect` command, which
   persists to the real config path; two runs (18:27 and 20:15:56) left a
   155-byte file with `"identity": null`. The shared harness now redirects
   `XDG_CONFIG_HOME` to a per-process scratch directory *before* the app builds,
   so no test can reach the real file. Verified: the full suite ran with the
   user's `settings.json` hash byte-identical before and after. **The live file
   itself was already damaged before the fix** and is left for the user to
   restore, because it is their shared configuration: the originals (host,
   username, SoundFont, and the identity this client and PalaceChat share)
   survive in `~/.config/org.palace.client/settings.json.bak`. One copy:
   `cp ~/.config/org.palace.client/settings.json.bak
   ~/.config/org.palace.client/settings.json`
2. **The multi-window harness and CI did not force `GDK_BACKEND=x11` — FIXED.**
   The per-test children already did, but the shared harness proved the display
   was virtual without making GDK use it, and GTK3 could otherwise prefer the
   live `wayland-0` socket — so a harness window could land on the desktop. The
   harness now forces `GDK_BACKEND=x11` and removes `WAYLAND_DISPLAY`, and the
   CI job sets `GDK_BACKEND: "x11"`. Verified by running the full suite with
   `WAYLAND_DISPLAY=wayland-0` deliberately set; the log shows
   `harness_isolation gdk_backend=x11 wayland=removed`. Regression test:
   `the_harness_isolation_pins_x11_and_a_scratch_config_directory`.



