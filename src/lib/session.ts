import * as api from "./api";
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
 */
export function startSession(): { stop: () => void } {
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
  })();

  return {
    stop(): void {
      disposed = true;
      unlisten?.();
    },
  };
}
