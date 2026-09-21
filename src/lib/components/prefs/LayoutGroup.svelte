<script lang="ts">
  import { onMount } from "svelte";
  import { listen } from "@tauri-apps/api/event";

  import * as api from "../../api";
  import {
    DEFAULT_REMEMBER_LAYOUT,
    LAYOUT_CHANGED_EVENT,
    PANEL_TITLES,
    layoutMemoryFrom,
    type LayoutMemoryView,
  } from "../../layoutPrefs";
  import type { PanelId } from "../../panels";

  // The last state Rust reported. The controls are mirrors of it, so a failed
  // call can put them back to what the file actually says.
  let remember = $state(DEFAULT_REMEMBER_LAYOUT);
  let detached = $state<PanelId[]>([]);
  let path = $state<string | null>(null);
  let loaded = $state(false);
  let busy = $state(false);
  let error = $state<string | null>(null);
  let notice = $state<string | null>(null);

  function apply(state: api.LayoutMemory): void {
    const view: LayoutMemoryView = layoutMemoryFrom(state);
    remember = view.remember;
    detached = view.detached;
    path = view.path;
    loaded = true;
  }

  async function refresh(): Promise<void> {
    try {
      apply(await api.getLayoutMemory());
      error = null;
    } catch (cause) {
      error = `Could not read the layout state: ${String(cause)}`;
    }
  }

  onMount(() => {
    void refresh();
    // Detaching a panel in another window changes what this list must show.
    // Rust broadcasts the new state, so there is no polling here.
    let stop: (() => void) | undefined;
    let cancelled = false;
    void listen<api.LayoutMemory>(LAYOUT_CHANGED_EVENT, (event) => apply(event.payload))
      .then((unlisten) => {
        if (cancelled) {
          unlisten();
        } else {
          stop = unlisten;
        }
      })
      .catch(() => {
        // No event bridge in this host; the list still loads once.
      });
    return () => {
      cancelled = true;
      stop?.();
    };
  });

  async function onRemember(event: Event): Promise<void> {
    const box = event.currentTarget as HTMLInputElement;
    const wanted = box.checked;
    if (busy) {
      return;
    }
    busy = true;
    error = null;
    try {
      apply(await api.setLayoutRemember(wanted));
      notice = wanted
        ? "Layout memory on: window positions are saved again."
        : "Layout memory off: nothing more is saved, and the next launch starts from the default single-window layout.";
    } catch (cause) {
      error = `Could not change layout memory: ${String(cause)}`;
      box.checked = remember;
    } finally {
      busy = false;
    }
  }

  async function onReset(): Promise<void> {
    if (busy) {
      return;
    }
    busy = true;
    error = null;
    try {
      apply(await api.resetLayout());
      notice = "Layout reset: every panel is docked again and the saved layout is forgotten.";
    } catch (cause) {
      error = `Could not reset the layout: ${String(cause)}`;
    } finally {
      busy = false;
    }
  }
</script>

<div class="prefs-group">
  {#if error}
    <p class="prefs-error" role="alert">{error}</p>
  {/if}
  {#if notice}
    <p class="prefs-notice" role="status" data-testid="layout-notice">{notice}</p>
  {/if}

  <section class="prefs-section" aria-labelledby="prefs-layout-memory">
    <h2 id="prefs-layout-memory">Window positions</h2>
    <p class="prefs-note">
      Where every window was, and which panels were detached, is remembered for the next launch.
      Turning this off stops the saving for good until it is turned back on.
    </p>

    <label class="prefs-check">
      <input
        type="checkbox"
        checked={remember}
        disabled={busy}
        data-testid="layout-remember"
        onchange={onRemember}
      />
      <span>Remember layout</span>
    </label>

    {#if !remember}
      <p class="prefs-note" data-testid="layout-remember-off-note">
        Off: nothing is written, so the next launch starts from the default single-window layout.
        The layout saved before you turned this off is kept but no longer used.
      </p>
    {/if}
  </section>

  <section class="prefs-section" aria-labelledby="prefs-layout-detached">
    <h2 id="prefs-layout-detached">Detached panels</h2>
    <p class="prefs-note">
      These panels are in their own window right now. Closing one re-docks it in the main window.
    </p>

    {#if !loaded}
      <p class="prefs-note" data-testid="layout-detached-loading">Reading the current layout…</p>
    {:else if detached.length === 0}
      <p class="prefs-note" data-testid="layout-detached-none">
        All panels are docked in the main window.
      </p>
    {:else}
      <ul class="prefs-detached" data-testid="layout-detached-list">
        {#each detached as panel (panel)}
          <li data-testid="layout-detached-{panel}">{PANEL_TITLES[panel]}</li>
        {/each}
      </ul>
    {/if}
  </section>

  <section class="prefs-section" aria-labelledby="prefs-layout-reset">
    <h2 id="prefs-layout-reset">Reset</h2>
    <p class="prefs-note">
      Re-docks every panel now and forgets the saved positions, so the next launch is the default
      single-window layout. The main window is left where it is.
    </p>
    <button
      class="btn"
      type="button"
      data-testid="layout-reset"
      disabled={busy}
      onclick={onReset}
    >
      Reset layout to default
    </button>
  </section>

  {#if path}
    <p class="prefs-note prefs-path" data-testid="layout-path">
      Saved in {path} — separate from the shared settings file.
    </p>
  {/if}
</div>

<style>
  .prefs-detached {
    margin: var(--sp-2) 0;
    padding-left: var(--sp-5);
  }

  .prefs-path {
    font-size: var(--fs-xs);
    word-break: break-all;
  }
</style>
