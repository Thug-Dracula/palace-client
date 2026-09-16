# Status / handoff — Palace Tauri client

**Last updated:** 2026-09-16
**Scope, decisions and design:** `~/palace-corpus/TAURI-CLIENT-SCOPE.md` ← read that first, not this file's context.

This file exists so a **fresh session** can resume without carrying a long conversation.

---

## Where the code is

`~/ProgramFiles/palace-client` — git repo, branch `master`.

| Crate | State | Evidence |
|---|---|---|
| `palace-wire` | ✅ done | framing, `ByteOrder`, 75-opcode table, codecs |
| `palace-probe` | ✅ done | live logon; **81 rooms / 2 users**; matched `palace_walker.py` exactly |
| `palace-room` | ✅ done | **804/804** corpus records parse clean, 0 warnings, 0 panics |
| `palace-prop` | ✅ done | 5 decoders + S20 encoder; **222k props**; zero differential disagreements |
| `palace-asset` | ⚠️ **in worktree, NOT merged** | `qAst`/`sAst`/`rAst`, paced scheduler, media HTTP; validated against 327 real captured `sAst` frames |
| `palace-render` | ⚠️ **in worktree, NOT merged** | composites rooms → PNG; 8 renders produced |

`master` currently: **183 tests passing**, clippy clean, working tree clean.

### FIRST TASK FOR THE NEXT SESSION — merge the two pending worktrees

```bash
cd ~/ProgramFiles/palace-client
git merge --no-ff feat/palace-asset -m "merge: palace-asset"
git merge --no-ff feat/palace-render -m "merge: palace-render"
```

Expected conflicts — resolve as noted, then `cargo test --workspace` must stay green:

| File | Why | Resolution |
|---|---|---|
| `Cargo.toml` | both add a workspace member; **asset also bumps `rust-version` 1.75 → 1.85** (needed by `ureq` 3.x) | union — keep both members **and** the higher `rust-version` |
| `Cargo.lock` | machine-generated | take either side, then `cargo update -w` / let cargo refresh |
| `README.md` | both append a section | union of both sections |

Then clean up:
```bash
git worktree remove --force ~/ProgramFiles/palace-client-asset
git worktree remove --force ~/ProgramFiles/palace-client-render
git worktree prune && git branch -d feat/palace-asset feat/palace-render
```

---

## You can look at it right now

`~/ProgramFiles/palace-client-render/renders/` — 8 real rooms composited from the corpus, each with a `.report.txt`:

- `901-balamb-garden.png` — Balamb Garden entrance (real backdrop + overlay)
- `1672-ludo-ii-props-avatars.png` — real loose props + real avatars
- `167-avatar-editor.png` — a **1280×720** room (room size is *not* fixed at 512×384)
- `1672-ludo-ii-dpr2.png` — 2048×1280 buffer at DPR 2
- `1021-rpg-loading.png` (exact 512×384 floor), `90-template-room.png`, `7022-okas-magic-bar.png`

`regenerate.sh` rebuilds them all.

---

## Next milestones

1. Merge the two worktrees (above), then wire them together into `palace-client`
2. **Tauri shell** — present composited frames via `register_asynchronous_uri_scheme_protocol`; HTML overlay for labels/hotspots. This is the first *interactive* artifact.
3. **IPTSCRAE** — a fresh Rust VM. Port the pure core; the command layer is Angular-coupled in the reference, so implement it against a capability trait (see scope doc).
4. Interaction completeness, then compatibility hardening.

---

## Honest gaps

- **Multi-block asset transfer is reference-derived only** — no real multi-block capture exists anywhere in the corpus.
- **16-bit props have zero real samples** — implemented from reference + synthetic tests, medium confidence.
- **All 155 loose-prop IDs** in the 799-room corpus are missing from local stores, so corpus rooms only ever draw placeholders. Real prop art was demonstrated with IDs taken from `pserver.prp`.
- `pserver_full.prp` is systematically corrupt (documented in the scope doc's Verified findings).
- Web panes with transparency/layering remain the highest-risk unbuilt feature — possibly infeasible; PalaceChat itself hasn't achieved it on Linux.

---

## Operational rules that have been earning their keep

- **Delegate one milestone per agent, in its own git worktree.** Parallel tracks only when they touch disjoint crates.
- **ALWAYS re-run the agent's own verification command before merging.** This has caught 4 real errors so far (a wrong padding formula, a bad dither in a reference implementation, a wrong compression assumption, and a double-counted avatar anchor).
- **Never read the research dumps** in `~/.local/share/opencode/tool-output/` — they are distilled into the scope doc. Reference file *paths*, never paste contents.
- **`~/palace-corpus/` is read-only reference.** Do not edit it. Beware Python tools creating `__pycache__` there.
- Update `~/palace-corpus/TAURI-CLIENT-SCOPE.md` at the end of each milestone.
