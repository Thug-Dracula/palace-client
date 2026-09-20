#!/usr/bin/env python3
"""Headless visual check of the palace-client UI.

Loads the local Vite dev server (default http://localhost:1420) in a
Playwright-controlled *headless* Chromium with a stubbed Tauri bridge
(`window.__TAURI_INTERNALS__`), then screenshots the shell, the prop editor,
the room menu and the prop bag. With --tabs it also screenshots each editor
tool tab.

Safety: this drives ONLY its own headless browser pointed at its own dev
server. It never screenshots the user's screen, never drives the user's
input devices, never attaches to the running Tauri app, and never opens a
debug/inspector port on the user's app. Do not change that.

Requires (already present on this machine, intentionally NOT vendored):
  - python3 with the `playwright` and `Pillow` packages
  - `playwright install chromium` browser build

Usage:
  bun run dev                       # serve on http://localhost:1420
  python3 tools/ui-visual-check/capture.py [--tabs] [--out DIR] [--port PORT]

Exit status: 0 on success, 1 if the dev server could not be reached.
"""
from __future__ import annotations

import argparse
import base64
import json
import os
import pathlib
import sys

from playwright.sync_api import sync_playwright
from PIL import Image, ImageDraw

HERE = pathlib.Path(__file__).resolve().parent
DEFAULT_OUT = HERE / "out"
DEFAULT_PORT = 1420
DEFAULT_TABS = "Paint,Transform,Adjust,Size,Frames,Text,Guides"


def make_frame_png(out_dir: pathlib.Path) -> str:
    """Draw a recognisable 44x44 checkerboard + amber cross, return base64."""
    img = Image.new("RGBA", (44, 44), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    for y in range(0, 44, 8):
        for x in range(0, 44, 8):
            if (x // 8 + y // 8) % 2 == 0:
                d.rectangle([x, y, x + 7, y + 7], fill=(60, 75, 95, 255))
    d.rectangle([20, 4, 23, 39], fill=(240, 180, 60, 255))
    d.rectangle([4, 20, 39, 23], fill=(240, 180, 60, 255))
    frame_path = out_dir / "frame.png"
    img.save(frame_path)
    return base64.b64encode(frame_path.read_bytes()).decode()


def build_stub(png_b64: str) -> str:
    """Tauri IPC stub: replay canned editor/bag state to the frontend.

    Field names mirror the real `src/lib/api.ts` interfaces exactly
    (`BagPropEntry` uses `w`/`h`/`fav`; `BagShelfInfo` uses
    `undecodable`/`dropped`; `BagCollectionInfo` uses `path`/`exists`).
    Getting these wrong makes the UI look broken when it is not.
    """
    bag_catalog = [
        {"id": 976933367, "crc": 4292559158, "name": "The Colosseum (1 vs 1)",
         "w": 44, "h": 44, "flags": 512, "fav": True, "trash": False,
         "collection": "My Bag", "source": "bag"},
        {"id": 60065, "crc": 4284005580, "name": "phone on",
         "w": 44, "h": 44, "flags": 0, "fav": False, "trash": False,
         "collection": "My Bag", "source": "bag"},
        {"id": 1021, "crc": 2578878265, "name": "TopHat",
         "w": 44, "h": 44, "flags": 0, "fav": False, "trash": False,
         "collection": "Classic.prp", "source": "bag"},
    ]
    bag_shelves = [
        {"name": "Classic.prp", "path": "/tmp/bag/shelves/Classic.prp",
         "status": "ok", "records": 76, "named": 56,
         "undecodable": 0, "dropped": 0, "error": None},
    ]
    bag_catalog_json = json.dumps(bag_catalog)
    bag_shelves_json = json.dumps(bag_shelves)
    guide_grid = {
        "vertical": [{"from": {"x": x, "y": 0}, "to": {"x": x, "y": 44}}
                     for x in range(0, 44, 4)],
        "horizontal": [{"from": {"x": 0, "y": y}, "to": {"x": 44, "y": y}}
                       for y in range(0, 44, 4)],
    }
    guide_geometry_json = json.dumps({
        "width": 44, "height": 44,
        "grid": guide_grid,
        "centre": {
            "vertical": {"from": {"x": 22, "y": 0}, "to": {"x": 22, "y": 44}},
            "horizontal": {"from": {"x": 0, "y": 22}, "to": {"x": 44, "y": 22}},
        },
        "safe_area": {"x": 4, "y": 4, "width": 36, "height": 36},
        "thirds": {
            "vertical": [{"from": {"x": 15, "y": 0}, "to": {"x": 15, "y": 44}},
                         {"from": {"x": 29, "y": 0}, "to": {"x": 29, "y": 44}}],
            "horizontal": [{"from": {"x": 0, "y": 15}, "to": {"x": 44, "y": 15}},
                           {"from": {"x": 0, "y": 29}, "to": {"x": 44, "y": 29}}],
        },
    })
    guide_state_json = json.dumps({
        "toggles": {"onion": False, "grid": True, "snap_grid": True,
                    "snap_centre": False, "safe_area": True, "thirds": True},
        "grid_spacing": 4, "safe_area_inset": 4, "onion_radius": 1,
        "width": 44, "height": 44, "frame_index": 0, "frame_count": 3,
    })
    return f"""
window.__TAURI_INTERNALS__ = {{
  transformCallback: function () {{ return 1; }},
  convertFileSrc: function (p) {{ return p; }},
  metadata: {{ currentWindow: {{ label: 'main' }}, currentWebview: {{ label: 'main' }} }},
  invoke: async function (cmd, args) {{
    const PNG_B64 = {json.dumps(png_b64)};
    const bytes = Array.from(atob(PNG_B64), c => c.charCodeAt(0));
    const layer = {{ id: 1, name: 'Base', width: 44, height: 44, opacity: 1.0,
                     visible: true, x: 0, y: 0, rotation: 0, scale: 1.0 }};
    const state = {{
      origin: {{ kind: 'blank' }},
      width: 44, height: 44,
      frame_count: 3, frame_index: 0,
      frames: [1, 2, 3].map((n, i) => ({{ index: i, width: 44, height: 44,
                layer_count: 1, selected: i === 0 }})),
      layers: [layer],
      can_undo: true, can_redo: false, can_paste: true,
      undo_depth: 2, redo_depth: 0, undo_bytes: 4096, dirty: true
    }};
    switch (cmd) {{
      case 'editor_state':
      case 'editor_open_blank':
      case 'editor_open_bag_prop':
      case 'editor_open_image':
        return state;
      case 'editor_frame_png':
        return bytes;
      case 'editor_undo': case 'editor_redo': case 'editor_add_frame':
      case 'editor_duplicate_frame': case 'editor_delete_frame':
      case 'editor_move_frame': case 'editor_select_frame':
        return {{ changed: true, state: state }};
      case 'bag_catalog':
        return {bag_catalog_json};
      case 'bag_collections':
        return [
          {{ path: '/tmp/bag/My Bag.prp', name: 'My Bag', writable: true, exists: true }},
          {{ path: '/tmp/bag/shelves/Classic.prp', name: 'Classic.prp', writable: false, exists: true }}
        ];
      case 'bag_shelves':
        return {bag_shelves_json};
      case 'editor_guide_state':
        return {guide_state_json};
      case 'editor_guide_geometry':
        return {guide_geometry_json};
      case 'editor_onion_neighbours':
        return [{{ index: 1, offset: 1 }}];
      case 'editor_set_guide_toggles': {{
        const t = (args && args.toggles) || {{}};
        return Object.assign({{}}, {guide_state_json}, {{ toggles: t }});
      }}
      case 'editor_set_onion_radius': {{
        const r = (args && args.radius) || 1;
        return Object.assign({{}}, {guide_state_json}, {{ onion_radius: r }});
      }}
      case 'editor_snap_point':
        return {{ x: (args && args.x) || 0, y: (args && args.y) || 0 }};
      case 'get_settings':
        return {{ host: 'localhost', port: 9998, username: 'Guest',
                  soundfont: null }};
      default:
        return null;
    }}
  }}
}};
"""


def shot(page, out_dir: pathlib.Path, name: str, log: list[str]) -> None:
    path = out_dir / name
    page.screenshot(path=str(path))
    log.append(f"wrote {path.name} ({path.stat().st_size} bytes)")


def slug(name: str) -> str:
    return "".join(c if c.isalnum() else "-" for c in name.strip().lower()).strip("-")


def close_overlays(page, log: list[str]) -> None:
    """Close whatever dialog is open (bag, pickers) so the next step can act."""
    try:
        page.keyboard.press("Escape")
        page.wait_for_timeout(400)
        dialogs = page.locator("[role=dialog]:visible")
        if dialogs.count():
            dialogs.first.locator(".dialog-x, button[aria-label*='lose']").first.click(timeout=1500)
            page.wait_for_timeout(400)
    except Exception as exc:  # noqa: BLE001
        log.append(f"close overlays: {exc}")


def run(url: str, out_dir: pathlib.Path, tabs: bool, tab_names: list[str]) -> int:
    out_dir.mkdir(parents=True, exist_ok=True)
    png_b64 = make_frame_png(out_dir)
    stub = build_stub(png_b64)

    console: list[str] = []
    log: list[str] = []

    with sync_playwright() as p:
        # The app serves its own assets from `palace://localhost` on Linux, which
        # a plain browser cannot resolve. A Windows user agent makes it use the
        # documented fallback `http://palace.localhost` (see src/lib/internalUrl.ts),
        # which Playwright CAN intercept — so bag thumbnails render for real.
        windows_ua = (
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 "
            "(KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36"
        )
        browser = p.chromium.launch()  # headless by default
        page = browser.new_page(viewport={"width": 1400, "height": 900},
                                user_agent=windows_ua)
        page.on("console", lambda m: console.append(f"[{m.type}] {m.text}"))
        page.on("pageerror", lambda e: console.append(f"[pageerror] {e}"))

        thumb = base64.b64decode(png_b64)

        def serve_internal(route) -> None:
            if "bag-prop" in route.request.url:
                route.fulfill(status=200, content_type="image/png", body=thumb)
            else:
                route.fulfill(status=404, body=b"")

        page.route("http://palace.localhost/**", serve_internal)
        page.add_init_script(stub)
        try:
            page.goto(url, wait_until="networkidle", timeout=60000)
        except Exception as exc:  # noqa: BLE001
            browser.close()
            print(f"error: could not load {url}: {exc}", file=sys.stderr)
            print("hint: start the dev server first: bun run dev", file=sys.stderr)
            return 1
        page.wait_for_timeout(2500)
        shot(page, out_dir, "01-shell.png", log)

        # Room right-click menu, then the prop bag (before the editor opens:
        # the editor is a full-window overlay that covers the room).
        try:
            viewport = page.locator(".viewport, .canvas-box, .room-canvas, canvas").first
            box = viewport.bounding_box()
            if box:
                page.mouse.click(box["x"] + box["width"] / 2,
                                 box["y"] + box["height"] / 2,
                                 button="right")
                page.wait_for_timeout(1200)
                shot(page, out_dir, "02-roommenu.png", log)
                for sel in (".ctx-menu .ctx-item:has-text('Props')",
                            "[role=menuitem]:has-text('Props')",
                            "text=Props"):
                    try:
                        page.locator(sel).first.click(timeout=1500)
                        break
                    except Exception:  # noqa: BLE001
                        continue
                else:
                    raise RuntimeError("no Props entry in the room menu")
                page.wait_for_timeout(2000)
                shot(page, out_dir, "03-bag.png", log)
        except Exception as exc:  # noqa: BLE001
            console.append(f"[bag] {exc}")
        close_overlays(page, log)

        # Open the prop editor (full-window overlay).
        try:
            page.get_by_role("button", name="Editor").first.click(timeout=5000)
            page.wait_for_timeout(2500)
            shot(page, out_dir, "04-editor.png", log)
        except Exception as exc:  # noqa: BLE001
            console.append(f"[editor] {exc}")

        if tabs:
            # Prefer the tabs actually present; fall back to the known list.
            labels = tab_names or []
            if not labels:
                detected = page.locator("button.panel-tab")
                labels = [detected.nth(i).inner_text().strip()
                          for i in range(detected.count())]
                labels = [l for l in labels if l]
            labels = labels or [t.strip() for t in DEFAULT_TABS.split(",")]
            for label in labels:
                for sel in (f"button.panel-tab:has-text('{label}')",
                            f"button:has-text('{label}')"):
                    try:
                        page.locator(sel).first.click(timeout=3000)
                        break
                    except Exception:  # noqa: BLE001
                        continue
                else:
                    console.append(f"[tab {label}] not found")
                    continue
                page.wait_for_timeout(1200)
                shot(page, out_dir, f"tab-{slug(label)}.png", log)

        browser.close()

    (out_dir / "console.log").write_text("\n".join(console))
    print(f"target: {url}")
    print(f"output: {out_dir}")
    for line in log:
        print(" ", line)
    print(f"console lines: {len(console)} (see {out_dir / 'console.log'})")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--url", default=os.environ.get("UI_VISUAL_URL"),
                    help="dev server URL (default http://localhost:<port>/)")
    ap.add_argument("--port", type=int, default=DEFAULT_PORT,
                    help=f"dev server port (default {DEFAULT_PORT})")
    ap.add_argument("--out", default=os.environ.get("UI_VISUAL_OUT", str(DEFAULT_OUT)),
                    help=f"screenshot output dir (default {DEFAULT_OUT})")
    ap.add_argument("--tabs", action="store_true",
                    help="also screenshot each editor tool tab")
    ap.add_argument("--tab-names", default=None,
                    help=f"comma-separated tab names for --tabs "
                         f"(default: detect from the page, else {DEFAULT_TABS})")
    args = ap.parse_args()

    url = args.url or f"http://localhost:{args.port}/"
    out_dir = pathlib.Path(args.out).expanduser().resolve()
    tab_names = [t.strip() for t in args.tab_names.split(",") if t.strip()] \
        if args.tab_names else []
    return run(url, out_dir, args.tabs, tab_names)


if __name__ == "__main__":
    sys.exit(main())
