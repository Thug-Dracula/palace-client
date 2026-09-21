/**
 * The two Appearance settings that need the host, not the DOM.
 *
 * Colours and typography are applied by writing CSS custom properties, which is
 * pure DOM work (see `appearance.ts`). Interface scale and full screen are
 * window-level and go through the host instead. They are kept in their own
 * module so a component test can mock the host seam without touching the
 * stylesheet seam.
 */
import { invoke } from "@tauri-apps/api/core";

/**
 * Enter or leave full screen on the primary window.
 *
 * The preference is application-wide, so the Rust command applies it to `main`
 * rather than to the Preferences window that raised it. Outside a Tauri host
 * (unit tests, a plain browser) the call rejects; the caller decides what to
 * report rather than pretending it succeeded.
 */
export function setFullscreen(fullscreen: boolean): Promise<boolean> {
  return invoke<boolean>("set_fullscreen", { fullscreen });
}
