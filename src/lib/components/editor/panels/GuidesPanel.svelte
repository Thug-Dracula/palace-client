<script lang="ts">
  /**
   * Guides panel: the view-only guide and onion-skin switches.
   *
   * Every switch is presentation state that lives in the Rust session, so
   * enabling a guide can never change a pixel or push an undo step. The panel
   * reads the switches from the session and writes only the switches back; the
   * canvas draws the overlay from the same state. "Centre guides" is the
   * reference client's single centre switch: it draws the centre cross and
   * snaps placed points to the middle.
   */
  import * as api from "../../../api";
  import type { PanelContract } from "./contract";
  import { causeMessage } from "./actions";
  import { MESSAGES, PANELS } from "../strings";
  import "./panel.css";

  let {
    state: doc,
    busy,
    active,
    notify,
    guideRefresh,
  }: PanelContract & { guideRefresh: () => Promise<void> } = $props();

  const LABELS: Record<keyof api.EditorGuideToggles, string> = {
    onion: PANELS.guides.onionSkin,
    grid: PANELS.guides.pixelGrid,
    snap_grid: PANELS.guides.snapGrid,
    snap_centre: PANELS.guides.centreGuides,
    safe_area: PANELS.guides.safeArea,
    thirds: PANELS.guides.thirds,
  };

  let guides = $state<api.EditorGuideState | null>(null);
  let working = $state(false);
  let radius = $state(1);

  const disabled = $derived(busy || working || !active || !doc || !guides);

  // Load once per open document. The key is a plain (non-reactive) variable so
  // refreshing the document after a switch does not re-trigger this effect.
  let loadedKey = "";
  $effect(() => {
    const key = doc ? `${doc.frame_index}/${doc.frame_count}/${doc.width}/${doc.height}` : "";
    if (!active || key === "") {
      guides = null;
      return;
    }
    if (key === loadedKey) {
      return;
    }
    loadedKey = key;
    void load();
  });

  async function load(): Promise<void> {
    try {
      const next = await api.editorGuideState();
      guides = next;
      radius = next.onion_radius;
    } catch (cause) {
      notify(causeMessage(cause), "error");
    }
  }

  async function apply(next: Promise<api.EditorGuideState>, message: string): Promise<void> {
    if (disabled) {
      return;
    }
    working = true;
    try {
      guides = await next;
      radius = guides.onion_radius;
      notify(message);
      await guideRefresh();
    } catch (cause) {
      notify(causeMessage(cause), "error");
    } finally {
      working = false;
    }
  }

  function toggle(key: keyof api.EditorGuideToggles, value: boolean): Promise<void> {
    if (!guides) {
      return Promise.resolve();
    }
    const next = { ...guides.toggles, [key]: value };
    return apply(
      api.editorSetGuideToggles(next),
      MESSAGES.guideToggled(LABELS[key], value),
    );
  }

  function commitRadius(): Promise<void> {
    return apply(api.editorSetOnionRadius(radius), MESSAGES.onionRadius(radius));
  }
</script>

<div class="panel">
  <h3>{PANELS.header.guides}</h3>

  {#if doc}
    {#if guides}
      <div class="panel-section">
        <h4>{PANELS.guides.overlay}</h4>
        <label class="panel-check">
          <input
            type="checkbox"
            checked={guides.toggles.grid}
            disabled={disabled}
            onchange={(event) => void toggle("grid", (event.currentTarget as HTMLInputElement).checked)}
          />
          {PANELS.guides.pixelGrid} · {guides.grid_spacing}px
        </label>
        <label class="panel-check">
          <input
            type="checkbox"
            checked={guides.toggles.snap_centre}
            disabled={disabled}
            onchange={(event) =>
              void toggle("snap_centre", (event.currentTarget as HTMLInputElement).checked)}
          />
          {PANELS.guides.centreGuides}
        </label>
        <label class="panel-check">
          <input
            type="checkbox"
            checked={guides.toggles.safe_area}
            disabled={disabled}
            onchange={(event) =>
              void toggle("safe_area", (event.currentTarget as HTMLInputElement).checked)}
          />
          {PANELS.guides.safeArea} · {guides.safe_area_inset}px
        </label>
        <label class="panel-check">
          <input
            type="checkbox"
            checked={guides.toggles.thirds}
            disabled={disabled}
            onchange={(event) => void toggle("thirds", (event.currentTarget as HTMLInputElement).checked)}
          />
          {PANELS.guides.thirds}
        </label>
        <p class="panel-note">{PANELS.guides.centreHint}</p>
      </div>

      <div class="panel-section">
        <h4>{PANELS.guides.snapping}</h4>
        <label class="panel-check">
          <input
            type="checkbox"
            checked={guides.toggles.snap_grid}
            disabled={disabled}
            onchange={(event) =>
              void toggle("snap_grid", (event.currentTarget as HTMLInputElement).checked)}
          />
          {PANELS.guides.snapGrid}
        </label>
      </div>

      <div class="panel-section">
        <h4>{PANELS.guides.onionSkin}</h4>
        <label class="panel-check">
          <input
            type="checkbox"
            checked={guides.toggles.onion}
            disabled={disabled}
            onchange={(event) => void toggle("onion", (event.currentTarget as HTMLInputElement).checked)}
          />
          {PANELS.guides.onionSkin} · {PANELS.guides.onionPrevious}/{PANELS.guides.onionNext}
        </label>
        <div class="panel-row">
          <label for="guides-onion-radius">{PANELS.guides.onionRadius}</label>
          <input
            id="guides-onion-radius"
            type="range"
            min="1"
            max="8"
            step="1"
            bind:value={radius}
            disabled={disabled}
            onchange={() => void commitRadius()}
          />
          <span class="panel-value">{guides.onion_radius}</span>
        </div>
        <p class="panel-note">{PANELS.guides.onionHint}</p>
      </div>

      <p class="panel-note">{PANELS.guides.note}</p>
    {:else}
      <p class="panel-note ui-loading" role="status">{PANELS.common.noDocument}</p>
    {/if}
  {:else}
    <p class="panel-note">{PANELS.common.noDocument}</p>
  {/if}
</div>
