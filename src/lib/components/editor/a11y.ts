import { PANEL_TABS, type PanelId } from "../../editorView";

/**
 * Keyboard and focus helpers for the editor and bag dialogs.
 *
 * Everything here is either pure math or a tiny DOM query, so it can be tested
 * headlessly (jsdom) without a Tauri runtime. The dialogs own their own
 * keydown handlers; these helpers decide *where focus goes* and *what a
 * shortcut means*, which is exactly the part that is easy to get subtly wrong.
 */

/**
 * The elements a dialog is allowed to move focus to. Disabled controls and
 * `tabindex="-1"` elements are excluded so a trapped Tab never lands on a
 * dead control.
 */
export const FOCUSABLE_SELECTOR = [
  "button:not([disabled])",
  "input:not([disabled])",
  "select:not([disabled])",
  "textarea:not([disabled])",
  "a[href]",
  '[tabindex]:not([tabindex="-1"])',
].join(", ");

/** Every focusable element under `root`, in DOM (tab) order. */
export function focusables(root: ParentNode): HTMLElement[] {
  return Array.from(root.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR));
}

/**
 * Whether an element is actually rendered. `getClientRects()` is used rather
 * than `offsetParent` because it also reports hidden-by-collapse dialogs.
 */
export function isVisible(element: HTMLElement): boolean {
  return element.getClientRects().length > 0;
}

/** Focusable elements under `root` that are currently rendered. */
export function visibleFocusables(root: ParentNode): HTMLElement[] {
  return focusables(root).filter(isVisible);
}

/**
 * The focusable index a Tab press should land on, wrapping at both ends so
 * focus can never escape the dialog.
 *
 * `current < 0` means focus is outside the list (or on the dialog itself):
 * forward Tab starts at the first control, backward Tab at the last.
 */
export function nextFocusIndex(count: number, current: number, shift: boolean): number {
  if (count <= 0) {
    return -1;
  }
  if (current < 0) {
    return shift ? count - 1 : 0;
  }
  return shift ? (current - 1 + count) % count : (current + 1) % count;
}

/**
 * Trap a Tab key inside `root`. Returns true when the event was handled (the
 * caller should stop propagating it). Non-Tab keys and empty roots are left
 * alone so the dialog can still close on Escape.
 */
export function trapTabKey(event: KeyboardEvent, root: HTMLElement | null | undefined): boolean {
  if (event.key !== "Tab" || !root) {
    return false;
  }
  const items = visibleFocusables(root);
  if (items.length === 0) {
    return false;
  }
  const active = document.activeElement;
  const index = active instanceof HTMLElement ? items.indexOf(active) : -1;
  const next = nextFocusIndex(items.length, index, event.shiftKey);
  if (next < 0) {
    return false;
  }
  event.preventDefault();
  items[next]?.focus();
  return true;
}

/**
 * Focus the first rendered control of `root`, or `root` itself when it holds
 * none. Used when an overlay opens so keyboard users are not left behind it.
 */
export function focusFirst(root: HTMLElement | null | undefined): void {
  if (!root) {
    return;
  }
  const first = visibleFocusables(root).at(0);
  (first ?? root).focus();
}

/** A command the editor shell knows how to run. `null` means "not a shortcut". */
export type EditorCommand =
  | { kind: "undo" }
  | { kind: "redo" }
  | { kind: "zoom-in" }
  | { kind: "zoom-out" }
  | { kind: "fit" }
  | { kind: "actual-size" }
  | { kind: "save" }
  | { kind: "panel-cycle"; delta: number }
  | { kind: "panel-select"; id: PanelId };

/**
 * The subset of a keyboard event the shortcut map cares about. Keeping it
 * structural (not a real `KeyboardEvent`) makes the map testable in node.
 */
export interface ShortcutEvent {
  key: string;
  ctrlKey?: boolean;
  metaKey?: boolean;
  shiftKey?: boolean;
  altKey?: boolean;
}

/**
 * Map a key press to an editor command.
 *
 * Modifier choices are deliberate:
 * - Ctrl/Cmd is required for undo/redo/zoom/save so plain typing is never
 *   swallowed by the dialog while a field has focus.
 * - `Ctrl+Tab` (and `Ctrl+Shift+Tab`) cycles the tool panels; `Alt+Left/Right`
 *   does the same for keyboards/WebViews that reserve Ctrl+Tab; `Alt+1..7`
 *   jumps straight to a panel.
 */
export function editorCommand(event: ShortcutEvent): EditorCommand | null {
  const primary = Boolean(event.ctrlKey || event.metaKey);
  const key = event.key.toLowerCase();

  if (event.altKey && !primary) {
    if (key === "arrowright") {
      return { kind: "panel-cycle", delta: 1 };
    }
    if (key === "arrowleft") {
      return { kind: "panel-cycle", delta: -1 };
    }
    const position = Number(key);
    if (Number.isInteger(position) && position >= 1 && position <= PANEL_TABS.length) {
      return { kind: "panel-select", id: PANEL_TABS[position - 1].id };
    }
    return null;
  }

  if (!primary) {
    return null;
  }

  switch (key) {
    case "z":
      return event.shiftKey ? { kind: "redo" } : { kind: "undo" };
    case "y":
      return { kind: "redo" };
    case "=":
    case "+":
      return { kind: "zoom-in" };
    case "-":
    case "_":
      return { kind: "zoom-out" };
    case "0":
      return { kind: "fit" };
    case "1":
      return { kind: "actual-size" };
    case "s":
      return { kind: "save" };
    case "tab":
      return { kind: "panel-cycle", delta: event.shiftKey ? -1 : 1 };
    default:
      return null;
  }
}
