# UI visual check (headless)

A dev-only harness that lets an agent (or a human) *see* the frontend without
ever touching the user's desktop. It launches **its own headless Chromium**
via Playwright, points it at the **local Vite dev server**, replaces the Tauri
IPC layer with a small stub, and writes screenshots to an output directory.

This is the sanctioned way to do visual checking on this machine: `AGENTS.md`
forbids screen captures and input automation against the user's machine, and
`STATUS.md` records an incident where that rule was broken. This tool exists
so nobody has to improvise.

## What it does

1. Headless Chromium loads `http://localhost:1420/` (the dev server).
2. Before any app code runs, an init script installs a stub
   `window.__TAURI_INTERNALS__` that answers `invoke()` with canned editor and
   prop-bag state, plus a generated 44x44 test image. So the browser-only dev
   build behaves like it has the Tauri backend.
3. It screenshots, in order (best effort — a failed step is recorded in
   `console.log`, not fatal):
   - `01-shell.png` — app shell after load
   - `02-roommenu.png` — room right-click menu (right-click in the viewport)
   - `03-bag.png` — prop bag dialog opened from that menu
   - `04-editor.png` — the prop editor opened
   - with `--tabs`: one `tab-<name>.png` per editor tool tab (the tab names
     are read from the page; the built-in fallback list is
     `Paint,Transform,Adjust,Size,Frames,Text,Guides`)

   Order matters: the editor is a full-window overlay, so the room menu and
   bag are captured *before* it opens.
4. It writes `console.log` (browser console + page errors) next to the shots.

## What it does NOT touch (hard rule — do not change)

- **Never** captures the user's screen.
- **Never** moves, clicks, or types into the user's desktop or input devices.
  All synthetic clicks happen inside our own headless browser process, which
  has no window on the user's desktop.
- **Never** attaches to, inspects, or launches the running Tauri app.
- **Never** opens a debug/inspector port on the user's app (no
  `--remote-debugging-port`, no WebKit inspector socket, nothing).
- It touches the filesystem only through the chosen output directory.

If a change makes the harness need any of the above, that change is wrong —
stop and ask the user instead.

## How to run it

Prerequisites (already on this machine, intentionally **not** vendored as a
project dependency):

- `python3` with the `playwright` and `Pillow` packages
  (`python3 -c "from playwright.sync_api import sync_playwright"`)
- a Playwright Chromium build (`playwright install chromium`)

The dev server must be running on port **1420** (this repo's fixed Vite port):

```bash
bun run dev            # serves http://localhost:1420
```

Then, from the repo root:

```bash
bun run ui:visual                                  # shell + room menu + bag + editor
bun run ui:visual -- --tabs                        # ...plus every editor tab
python3 tools/ui-visual-check/capture.py --tabs    # same thing directly
python3 tools/ui-visual-check/tabs.py              # same as ... --tabs
tools/ui-visual-check/capture.sh --port 1420       # shell wrapper
```

Useful flags: `--out DIR` (or env `UI_VISUAL_OUT`), `--url URL` / `--port N`
(or env `UI_VISUAL_URL`), `--tab-names "Transform,Adjust,Frames"`.

If the dev server is not up, the script exits `1` with a "start the dev
server first" hint. It does not start or stop the server itself.

## Where screenshots go

Default: `tools/ui-visual-check/out/` (git-ignored). Override with `--out` or
`UI_VISUAL_OUT`, e.g. into a scratch dir for evidence:

```bash
python3 tools/ui-visual-check/capture.py --tabs --out /tmp/work/palace-uicheck
```
