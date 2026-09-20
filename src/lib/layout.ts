/**
 * Grid reflow maths for the main shell.
 *
 * Detaching a panel does not just hide it: its grid slot has to give its space
 * back to the room. That decision is pure arithmetic, so it lives here rather
 * than in `App.svelte` and can be tested without a DOM.
 *
 * The docked column/row sizes are the ones the shell has always used (232px
 * Rooms, 208px Users, 236px Chat). A detached panel leaves a slim *rail* behind
 * so the re-attach affordance stays visible instead of the slot vanishing.
 *
 * `App.svelte` feeds `trackStyle()` into the grid as inline custom properties,
 * which keeps these numbers as the single source of truth: the stylesheet names
 * the variables, never the pixel values.
 */

import { PANEL_IDS, type PanelId } from "./panels";

/** Docked width of the Rooms list column. */
export const ROOMS_WIDTH = 232;
/** Docked width of the Users list column. */
export const USERS_WIDTH = 208;
/** Docked height of the Chat/log row. */
export const CHAT_HEIGHT = 236;
/** Width/height a collapsed slot keeps for the re-attach rail. */
export const RAIL = 44;

/** The resolved grid tracks for the workspace and its centre column. */
export interface WorkspaceTracks {
  /** Left column width in CSS pixels. */
  rooms: number;
  /** Right column width in CSS pixels. */
  users: number;
  /** Bottom row of the centre column, in CSS pixels. */
  chat: number;
}

/**
 * Resolve every track size for the set of currently detached panels.
 *
 * The Room view owns no track of its own (it fills `1fr`), so detaching it only
 * swaps the component for its placeholder; the room area still grows whenever a
 * neighbouring sidebar or the chat row collapses.
 */
export function workspaceTracks(detached: readonly PanelId[]): WorkspaceTracks {
  const isDetached = (panel: PanelId): boolean => detached.includes(panel);
  return {
    rooms: isDetached("rooms") ? RAIL : ROOMS_WIDTH,
    users: isDetached("users") ? RAIL : USERS_WIDTH,
    chat: isDetached("chat") ? RAIL : CHAT_HEIGHT,
  };
}

/**
 * The inline `style` string `App.svelte` puts on `.workspace`.
 *
 * The centre column inherits `--row-chat` from the workspace, so one style
 * attribute drives both grids.
 */
export function trackStyle(detached: readonly PanelId[]): string {
  const tracks = workspaceTracks(detached);
  return `--col-rooms:${tracks.rooms}px;--col-users:${tracks.users}px;--row-chat:${tracks.chat}px`;
}

/**
 * The value of the `data-detached` attribute, e.g. `"users chat"`.
 *
 * Always ordered like `PANEL_IDS`, so the markup is stable and the CSS can
 * match a single id with `[data-detached~="users"]`.
 */
export function detachedAttr(detached: readonly PanelId[]): string {
  return PANEL_IDS.filter((panel) => detached.includes(panel)).join(" ");
}
