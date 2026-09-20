/**
 * Which panels are currently detached into their own OS window, and the actions
 * that move a panel between docked and detached.
 *
 * This is the one reactive holder the main shell reads to reflow its grid. It
 * deliberately does not own window geometry or persistence (Task 7/18 own
 * those); it owns only the docked/detached fact and the Rust calls that flip
 * it, so later per-panel tasks bind to one seam instead of each inventing a
 * local flag.
 *
 * A panel is detached optimistically: the grid reflows immediately and is put
 * back if Rust refuses to open the window, so the UI never claims a window
 * exists when it does not.
 */

import { listen } from "@tauri-apps/api/event";

import * as api from "./api";
import { PANEL_IDS, panelFromId, type PanelId } from "./panels";

/** The targeted event Rust emits to `main` when a panel window is closing. */
export const PANEL_CLOSED_EVENT = "palace://panel-closed";

/** The payload of {@link PANEL_CLOSED_EVENT}, mirroring `windows::PanelClosed`. */
export interface PanelClosedPayload {
  /** The closing window's label, e.g. `panel-users`. */
  label: string;
  /** The panel id, when the label named a known panel. */
  panel: string | null;
}

function allDocked(): Record<PanelId, boolean> {
  return { room: false, users: false, rooms: false, chat: false, props: false };
}

class PanelLayout {
  private state = $state<Record<PanelId, boolean>>(allDocked());

  /** Whether `panel` currently lives in its own OS window. */
  isDetached(panel: PanelId): boolean {
    return this.state[panel];
  }

  /** Every detached panel, in the plan's fixed panel order. */
  detachedIds(): PanelId[] {
    return PANEL_IDS.filter((panel) => this.state[panel]);
  }

  /** Set the docked/detached fact directly (persistence restore, tests). */
  setDetached(panel: PanelId, value: boolean): void {
    this.state[panel] = value;
  }

  /** Dock every panel again. */
  reset(): void {
    this.state = allDocked();
  }

  /**
   * Move `panel` into its own window.
   *
   * Resolves once Rust has answered. If the window could not be created the
   * grid slot is restored and the error re-thrown so the caller can report it.
   */
  async detach(panel: PanelId): Promise<void> {
    if (this.state[panel]) {
      return;
    }
    this.state[panel] = true;
    try {
      await api.openPanel(panel);
    } catch (error) {
      this.state[panel] = false;
      throw error;
    }
  }

  /** Bring `panel` back into the main shell, closing its window if it has one. */
  async reattach(panel: PanelId): Promise<void> {
    if (!this.state[panel]) {
      return;
    }
    try {
      await api.closePanel(panel);
    } catch {
      // The window may already be gone; docking again is still the right end state.
    }
    this.state[panel] = false;
  }

  /**
   * React to a panel window closing by docking that panel again.
   *
   * The label is the authority; the optional panel id is a convenience from
   * Rust. An unknown label is ignored rather than treated as an error.
   */
  applyPanelClosed(payload: PanelClosedPayload): void {
    const panel = panelFromId(payload.panel ?? payload.label);
    if (panel) {
      this.state[panel] = false;
    }
  }

  /**
   * Subscribe to Rust's targeted re-attach signal. `main` only.
   *
   * Returns a stop function. Outside a Tauri host (unit tests, a plain
   * browser) the subscribe fails and is ignored, so the shell still mounts.
   */
  startCloseListener(): () => void {
    let unlisten: (() => void) | undefined;
    let stopped = false;
    void listen<PanelClosedPayload>(PANEL_CLOSED_EVENT, (event) =>
      this.applyPanelClosed(event.payload),
    )
      .then((stop) => {
        if (stopped) {
          stop();
        } else {
          unlisten = stop;
        }
      })
      .catch(() => {
        // No Tauri event bridge in this host; there is nothing to listen to.
      });
    return () => {
      stopped = true;
      unlisten?.();
    };
  }
}

/** The shared detach state for this webview. */
export const panelLayout = new PanelLayout();
