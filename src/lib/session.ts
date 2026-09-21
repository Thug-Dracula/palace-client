import * as api from "./api";
import { graphicsPrefsFrom } from "./graphicsPrefs";
import { store } from "./store.svelte";

/**
 * The one way a window seeds itself.
 *
 * Every webview — the `main` shell and each `panel-*` window — is a peer view:
 * it subscribes to the single `palace://event` channel and then asks the
 * backend for a replay (`refresh`). Extracting the pattern here keeps that
 * behaviour identical everywhere instead of letting a detached window grow a
 * divergent copy. `stop()` unsubscribes; the subscription resolving after
 * unmount is handled so a disposed window never leaks a listener.
 *
 * A webview has exactly one session. A second `startSession()` in the same
 * module instance (a remount, a hot reload) returns the live session instead of
 * opening a second listener and a second `refresh`, so one window can never
 * double-apply the broadcast or issue a duplicate seed.
 */
let active: { stop: () => void } | undefined;

export function startSession(): { stop: () => void } {
  if (active) {
    return active;
  }
  let unlisten: (() => void) | undefined;
  let disposed = false;

  void (async () => {
    const stopListening = await api.onEvent((event) => store.apply(event));
    if (disposed) {
      stopListening();
      return;
    }
    unlisten = stopListening;
    // The backend auto-connects before this listener exists; ask it to replay
    // the current status, banner, lists and frame so the view starts in sync.
    await api.refresh().catch(() => {});
    // Name/avatar visibility is process-global backend state, so every window
    // seeds its mirrors from the saved choice — otherwise a launch would show
    // names until someone visited the Preferences group. `applyGraphics` skips a
    // value it already holds, so this never re-sends needlessly.
    const saved = await api.getPrefs().catch(() => null);
    if (saved !== null) {
      store.applyGraphics(graphicsPrefsFrom(saved));
    }
  })();

  const session = {
    stop(): void {
      disposed = true;
      unlisten?.();
      if (active === session) {
        active = undefined;
      }
    },
  };
  active = session;
  return session;
}
