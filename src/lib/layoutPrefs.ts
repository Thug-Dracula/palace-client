/**
 * The Layout memory preference group's rules, independent of the window.
 *
 * This group is the **UI over the layout store** Tasks 7 and 18 built. There is
 * no second persistence mechanism here and no `prefs.*` key: `PREFERENCES.md`
 * puts the whole group in its own file (`window-layout.json`, never the shared
 * `settings.json`), and the Rust `geometry` module owns it:
 *
 * | Stored field                  | What it does                                        |
 * |-------------------------------|-----------------------------------------------------|
 * | `remember`                    | yes — off stops the store writing at all            |
 * | `windows.<label>.x/y/w/h`     | yes — applied by the startup restore                |
 * | `windows.<label>.monitor`     | yes — the screen the rectangle is clamped onto      |
 * | `windows.<label>.detached`    | yes — a detached panel is reopened on the next launch |
 * | `windows.prefs.*`             | yes — the Preferences window's own rectangle        |
 * | `order`                       | yes — the order detached panels are reopened in     |
 *
 * The one action here that is not a stored field is "reset layout to default":
 * it re-docks every panel immediately and forgets the saved windows, so the
 * next launch is the default single-window layout.
 */
import type { LayoutMemory } from "./api";
import { PANEL_IDS, type PanelId } from "./panels";

/** The `remember` default: a fresh install remembers where its windows were. */
export const DEFAULT_REMEMBER_LAYOUT = true;

/** The event Rust broadcasts when layout memory changes. */
export const LAYOUT_CHANGED_EVENT = "palace://layout";

/** The display name of each panel, matching the window titles in `windows.rs`. */
export const PANEL_TITLES: Record<PanelId, string> = {
  room: "Room",
  users: "Users",
  rooms: "Rooms",
  chat: "Chat",
  props: "Prop bag",
};

/** Whether `value` names one of the five panels. */
export function isPanelId(value: unknown): value is PanelId {
  return typeof value === "string" && (PANEL_IDS as readonly string[]).includes(value);
}

/** The layout state with the detached ids checked against the panel registry. */
export interface LayoutMemoryView {
  /** Whether positions and sizes are being remembered. */
  remember: boolean;
  /** The panels in their own window right now, in the registry's fixed order. */
  detached: PanelId[];
  /** The file the layout is written to, when the platform names one. */
  path: string | null;
}

/**
 * Resolve the state Rust reported, defensively.
 *
 * A malformed payload must not leave the group claiming something it cannot
 * know: an unknown panel id is dropped, a duplicate is collapsed by the
 * registry's order, and a non-boolean `remember` falls back to the default.
 */
export function layoutMemoryFrom(raw: unknown): LayoutMemoryView {
  const record =
    typeof raw === "object" && raw !== null ? (raw as Record<string, unknown>) : {};
  const reported = Array.isArray(record.detached) ? record.detached.filter(isPanelId) : [];
  return {
    remember:
      typeof record.remember === "boolean" ? record.remember : DEFAULT_REMEMBER_LAYOUT,
    detached: PANEL_IDS.filter((panel) => reported.includes(panel)),
    path: typeof record.path === "string" && record.path !== "" ? record.path : null,
  };
}

/** The panel names a detached list should show, in the registry's order. */
export function detachedNames(panels: readonly PanelId[]): string[] {
  return panels.map((panel) => PANEL_TITLES[panel]);
}
