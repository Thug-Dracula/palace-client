/**
 * The frontend half of the panel registry contract.
 *
 * `src-tauri/src/windows.rs` owns the closed enum and builds
 * `index.html#/panel/<id>` for every detached window. This module mirrors that
 * set and parses the hash back into a view, so App can mount either the full
 * shell (`main`) or exactly one panel.
 *
 * The parser is deliberately as lenient as the Rust `Panel::from_id`: trimmed,
 * case-folded, and both `users` and `panel-users` are accepted. Anything
 * outside the set is rejected here; the caller decides the fallback.
 */

/** The five panels that can live in their own OS window, in plan order. */
export const PANEL_IDS = ["room", "users", "rooms", "chat", "props"] as const;

/** A detachable panel's stable id, as passed to `open_panel`/`close_panel`. */
export type PanelId = (typeof PANEL_IDS)[number];

const PANEL_ALIASES: Record<string, PanelId> = {
  room: "room",
  users: "users",
  rooms: "rooms",
  chat: "chat",
  props: "props",
  propbag: "props",
  "prop-bag": "props",
};

/** Map a panel id or window label onto the enum; null when it is not a panel. */
export function panelFromId(id: string): PanelId | null {
  const lowered = id.trim().toLowerCase();
  const name = lowered.startsWith("panel-") ? lowered.slice("panel-".length) : lowered;
  return PANEL_ALIASES[name] ?? null;
}

/** Which view a window should mount: the full shell, or one detached panel. */
export type AppView = { kind: "main" } | { kind: "panel"; panel: PanelId };

/**
 * Resolve `window.location.hash` into the view for this window.
 *
 * The only panel routes are the ones the Rust registry builds
 * (`#/panel/<id>`). An unknown or malformed hash falls back to the full
 * layout: that is also what the SPA asset fallback serves, so a stray URL can
 * never leave a window blank.
 */
export function viewForHash(hash: string): AppView {
  const path = hash.replace(/^#/, "").replace(/^\//, "");
  if (!path.startsWith("panel/")) {
    return { kind: "main" };
  }
  const panel = panelFromId(path.slice("panel/".length));
  return panel ? { kind: "panel", panel } : { kind: "main" };
}
