# COLOSSEUM — session record (TABLED 2026-09-19)

**Status: parked, not abandoned.** The benching bug is **fixed and verified**; one unrelated
rendering bug and a couple of small gaps remain. Nobody is actively working on this.
Resume from "How to resume" below.

*Written for the next engineer/LLM. The user reads plain English and does not read code.*

---

## The goal
Make the Rust Palace client at `$REPO` behave like the user's working
**PalaceChat**, so they can play in **The Colosseum** on `localhost:9998`.
PalaceChat is the specification — never Sparky/OpenPalace when they disagree.

---

## FIXED AND VERIFIED THIS SESSION

### 1. The benching ("Team is full!" → Audience) — root cause found and fixed

**Plain terms.** The arena marks a team slot as "taken" with a command that is defined as
*room-wide* (everyone in the room sees it). The arena's own "on leave" cleanup resets those
slots. A normal room change ran that cleanup, but **signing off did not** — so a fighter who
disconnected left the slot marked taken, and the next arrival (them, or anyone) was told
"Team is full!" and benched to the Audience. The slot only re-opened when a benched player got
ejected. That produced a perfect alternating cycle.

**Measured before the fix.** Four consecutive headless entries into room 31741:
Success / Benched / Success / Benched. A failing entry had **zero other users** in the room,
so this was not real occupancy.

**The fix.** The client now runs the current room's "on leave" cleanup — and sends the slot
resets it produces — when it signs off, before closing the connection. Because the GUI's
window-close path already asks the runtime to sign off, closing the app gets the fix too.

- `crates/palace-client/src/runtime.rs` — new `run_room_leave(...)` helper; the `Disconnect`
  command arm calls it before the LOGOFF frame. A normal room change uses the same helper.
- `crates/palace-client/tests/mock_runtime.rs` — `signing_off_runs_the_rooms_on_leave_handlers`.

**Evidence.** 221 tests green across `palace-client`; the new test proven to **fail** without
the wiring (only login chatter and the goodbye frame went out) and pass with it; two
consecutive live headless entries **both** placed a fighter (0 "Team is full", 0 Audience).

### 2. Also confirmed this session
- Room 31741's gate scripts ship **inside the room payload** (they are not attached at runtime).
- Conformance rooms **32000, 32001, 32009 match PalaceChat exactly**.
- The previous session's **dispatch-order fix** is what first made fighter placement land.

---

## DISPROVEN — do not re-chase
- **Alarm-timing hypothesis** (from `HANDOFF-2026-09-19.md`): false. The user's own PalaceChat
  reference proves alarms read values when they **fire**, not when scheduled (`dgv_alarm15=99`),
  and our client matches.
- **Another user occupying the slot**: false. The failing run had zero other users.
- **"Defect B" as the cause**: false. The stuck slot is server-side and survives across
  separate processes; the local-apply issue was not the mechanism.

---

## OPEN / TABLED

1. **Self-avatar is drawn in the wrong place in the Tauri client** (the newest issue, live).
   Tauri draws its **own** avatar ("Shadow") too low — near/past the bottom of the room — while
   PalaceChat draws it correctly, and the *other* avatar looks fine. The script had legitimately
   moved the self avatar to room **(282,362)** in a 512×384 room (y = 362 is the floor line, so
   most of the sprite is off the bottom).
   - Evidence: `$MEDIA/Screenshots/Screenshot_20260919_103157.png` and
     `Screenshot_20260919_103352.png`; app log
     `~/.local/share/org.palace.client/logs/palace-client.log` around 17:33:16Z.
   - Leads to test: in `crates/palace-render/src/build.rs` the built-in **face** part is placed
     at offset (0,0) while **worn props** use `(-22, +propOffset)`, and the anchor is the sprite
     centre (clamp is `[22, width-22]`); and the self-position path in
     `crates/palace-client/src/runtime.rs` (`avatar_specs`, the `MoveUserAbs`/`MoveUserRel` arm).
   - A deep-diagnosis task was started and then stopped at the user's request. **No changes were
     made for this item.**
2. **Whispers are not implemented** in the client, so the Colosseum's **basic attack does not
   work** (the ability prompts arrive as whispers).
3. **Reconnect still skips the leave-cleanup.** The sign-off fix covers `Disconnect`, not the
   `Reconnect` command. Reconnecting from inside the arena can leave the slot stuck. Small
   follow-up: run the same helper in the `Reconnect` arm.
4. **Window-close is best-effort** — the app waits ~150 ms after asking to sign off. That is the
   same reliability as the existing goodbye frame; don't over-claim it.
5. **"Defect B"** — applying a room-wide spot change locally before the server echoes it.
   Analysis (explore + Oracle) says: **not the cause, and not worth fixing now.** The one real
   dependency is untested: the local write currently makes the server's echo a no-op, so our own
   change does **not** fire an `ON STATECHANGE`; removing the local write would start firing it.
   The script-engine layer already matches PalaceChat here. If ever changed: keep
   `SETSPOTSTATELOCAL` local, expect `ON STATECHANGE` on our own echo, and add a test.

---

## UNCOMMITTED (no save-point made)
Modified but **not committed** (the repo does not auto-publish):
- `crates/palace-client/src/runtime.rs`
- `crates/palace-client/tests/mock_runtime.rs`

Plus untracked `HANDOFF-2026-09-19.md` and this file. Last committed save point: **`3836d5f`**.
**Ask the user before committing.**

---

### (appendix) Drafted reproduction for the self-avatar bug — saved, then reverted
A diagnosis pass was started and stopped mid-work. Before stopping it had added two **failing**
tests to `crates/palace-render/src/build.rs` (no fix). They were removed so the tree stays green
and the Colosseum fix stays a clean, verified save-point. The exact patch is saved at
`/tmp/work/avatar-repro.patch` (scratch — may be cleared), and the substance is:

- `a_face_and_a_worn_prop_are_placed_from_the_same_anchor_box` — builds a prop-less avatar (draws
  the built-in face) and a HEAD-prop avatar at the same `(x,y)`; both parts should land at the
  same `(dx,dy)`. Today the face is `(0,0)` and a prop is `(-22,+offset)`, so it fails.
- `a_face_at_the_bottom_clamp_stays_inside_the_room` — places a face-only avatar at `(282,362)`
  (the reported position) and asserts the face's bottom edge is ≤ 384; today it reaches 406.

Fix direction (UNVERIFIED): give the built-in face the same anchor-box offset as worn props
(`-AVATAR_HALF` on both axes) in `build_avatar`, check `smiley_cell` trimming, and update the
existing test `a_prop_less_avatar_renders_its_face_cell_at_the_anchor`, which currently pins the
old placement. Confirm against PalaceChat before/after.

## How to resume
- Scoped tests: `cargo test -p palace-client` (run from the repo root; **never** `--workspace`).
- Headless live check: see `HANDOFF-2026-09-19.md` for the `PALACE_SMOKE_STEPS` chain into 31741.
- If the arena ever says "Team is full!" again: leave the arena and re-enter once — that runs the
  arena's own reset. A slot left by an abrupt disconnect clears that way.
- Start the self-avatar item from the two screenshots + the app log.

## Hard rules (still in force)
- Never drive the user's mouse/keyboard/windows; no tmux-GUI, no inspector/debug ports, no
  screenshots of their desktop.
- `localhost:9998` is production and the user plays on it — never leave it down, never disturb
  their session.
- Never commit or publish without the user's explicit request.
- Never `--workspace` cargo; scope with `-p`.
- Don't edit `STATUS.md`, `NOTES.local.md`, `palace-todo.md`, `IPTSCRAE-GAP.md`, `AGENTS.md`.
