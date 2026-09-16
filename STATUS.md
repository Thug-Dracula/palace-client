# Status / handoff — Palace Tauri client

**Last updated:** 2026-09-16
**Scope, decisions and design:** `$CORPUS/TAURI-CLIENT-SCOPE.md` ← read that too. It is the design authority; this file is only "where things stand right now".

This file exists so a **fresh session** can resume without carrying a long conversation.

---

## Where the code is

`$REPO` — git repo, branch `master`, working tree clean.
**372 tests passing, clippy clean.**

All six crates are built, merged and independently verified:

| Crate | State | Evidence |
|---|---|---|
| `palace-wire` | ✅ done | framing, `ByteOrder`, 75-opcode table, codecs |
| `palace-probe` | ✅ done | live logon; **81 rooms / 2 users**; matched `palace_walker.py` exactly |
| `palace-room` | ✅ done | **804/804** corpus records parse clean, 0 warnings, 0 panics |
| `palace-prop` | ✅ done | 5 decoders + S20 encoder; **3,000-prop differential = 0 disagreements**; 8-bit parity 2989/2989 vs `prop_decoder.py` |
| `palace-asset` | ✅ merged | `qAst`/`sAst`/`rAst`, paced scheduler, media HTTP; validated against **327 real captured `sAst` frames** |
| `palace-render` | ✅ merged | composites rooms → PNG; coordinate mapping round-trip tested across 5 rooms × 10 viewports × 6 zooms × 5 DPRs |

The protocol layer, both binary codecs, the asset layer and the compositor all exist and are validated against real corpus data. **The hardest technical risk is behind us.**

---

## You can look at it right now

`$REPO/renders/` — 8 real rooms composited from the corpus, each with a `.report.txt`:

- `901-balamb-garden.png` — Balamb Garden entrance (real backdrop + overlay)
- `1672-ludo-ii-props-avatars.png` — real loose props + real avatars
- `167-avatar-editor.png` — a **1280×720** room (room size is *not* fixed at 512×384)
- `1672-ludo-ii-dpr2.png` — 2048×1280 buffer at DPR 2
- `1021-rpg-loading.png` (exact 512×384 floor), `90-template-room.png`, `7022-okas-magic-bar.png`

`renders/regenerate.sh` rebuilds them all.

**These have not been checked by a human yet.** Visual correctness — does a Palace room *look* right? — is the one thing automated verification cannot confirm. Worth eyeballing before building on top of it.

---

## NEXT TASK — wire it together, then put a window on it

1. **Integrate.** These crates are proven independently but are not yet composed into one runtime. Build a `palace-client` crate that owns the connection (`palace-wire` FSM + `palace-asset`) and drives `palace-room` → `palace-prop` → `palace-render`.
2. **Tauri shell.** New binary crate. Present composited frames via `register_asynchronous_uri_scheme_protocol`; HTML overlay for labels/hotspots/selection. Base the project shape on `$HOME/ProgramFiles/chiptune-player` (Tauri 2 + SvelteKit 5 + Vite).
   - **Never return `Vec<u8>` from a Tauri command** — it arrives as a JSON number array (measured 22.5 MB for a 6.3 MB image). All blobs go through the custom protocol.
   - WebGL on WebKitGTK can silently fall back to a software rasterizer with no detectable signal. The Rust compositor already avoids this — keep it that way, and test on real hardware early.
3. Then: **IPTSCRAE** (fresh Rust VM — see scope doc; the reference interpreter is Angular-coupled and cannot be embedded), interaction completeness, hardening.

---

## Honest gaps

- **Multi-block asset transfer is reference-derived only** — no real multi-block capture exists anywhere in the corpus.
- **16-bit props have zero real samples** — implemented from reference + synthetic tests, medium confidence.
- **All 155 loose-prop IDs** in the 799-room corpus are missing from local stores, so corpus rooms only ever draw placeholders. Real prop art was demonstrated with IDs taken from `pserver.prp`.
- `pserver_full.prp` is systematically corrupt (see scope doc's Verified findings).
- **Draw commands, name tags and chat text are not rasterized** (0 of 799 rooms use draw commands).
- Avatar anchor offset is `x−22, y−22`; one reference uses −21. Flagged as a 1 px ambiguity.
- **Web panes with transparency/layering remain the highest-risk unbuilt feature** — possibly infeasible. PalaceChat itself hasn't achieved it on Linux. Plan to cut it.

---

## Operational rules that have been earning their keep

- **Delegate one milestone per agent, in its own git worktree.** Parallel tracks only when they touch disjoint crates. Merge conflicts land in `Cargo.toml` / `Cargo.lock` / `README.md` — resolve as union, and **never blanket `git add -A` during a conflicted merge** (that once committed conflict markers into `README.md`).
- **ALWAYS re-run the agent's own verification command before merging.** This has caught or confirmed five real findings: a wrong padding formula, Taj's bad dither, an incorrect "8-bit is zlib" assumption (also wrong in `PRP-FORMAT.md`), a double-counted avatar anchor, and a stale todo.
- **Never read the research dumps** in `~/.local/share/agent-runner/tool-output/` — they're distilled into the scope doc. Reference file *paths*, never paste contents.
- **`$CORPUS/` is read-only reference.** Do not edit it; beware Python tools writing `__pycache__` there.
- Update `$CORPUS/TAURI-CLIENT-SCOPE.md` at the end of each milestone.
