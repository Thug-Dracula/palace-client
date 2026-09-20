/**
 * Which kind of window this webview is.
 *
 * Detaching is a `main`-only action: the button that sends a panel to its own
 * OS window belongs in the docked shell, not inside the panel window itself
 * (where the way back is closing the window). Both run the same components, so
 * the control needs a way to tell which window it is in. The SPA hash is that
 * signal — the Rust registry builds `#/panel/<id>` for a detached window and
 * `main` loads the bare page.
 */

import { viewForHash } from "./panels";

/**
 * True when this webview is a `panel-*` window rather than the `main` shell.
 *
 * The hash is read once at call time. During SSR there is no `window`, so the
 * empty hash resolves to `main`; a component only consults this while mounted.
 */
export function isPanelWindow(
  hash: string = typeof window === "undefined" ? "" : window.location.hash,
): boolean {
  return viewForHash(hash).kind === "panel";
}
