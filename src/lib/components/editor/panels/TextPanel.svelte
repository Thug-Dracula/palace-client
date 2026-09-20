<script lang="ts">
  /**
   * Text panel: the text tool, external overlay import, and the layer list.
   *
   * Everything rasterizes in Rust (`editor_add_text` lays the string out with an
   * embedded face and bakes shadow/outline into a layer; `editor_import_overlay`
   * decodes an image into a layer). This panel only edits parameters. The layer
   * list is the R3 addition beyond PalaceChat and lives in `LayersList.svelte`.
   */
  import * as api from "../../../api";
  import type { PanelContract } from "./contract";
  import { causeMessage } from "./actions";
  import LayersList from "../LayersList.svelte";
  import { MESSAGES, PANELS, REFERENCE_TOOLTIPS, TOOLTIPS } from "../strings";
  import "./panel.css";

  let { state: doc, busy, active, notify, refresh }: PanelContract = $props();

  let text = $state("");
  let hex = $state("#ffffff");
  let alpha = $state(255);
  let size = $state(16);
  let x = $state(0);
  let y = $state(0);
  let rotation = $state(0);
  let opacity = $state(1);
  let shadow = $state(true);
  let outline = $state(false);
  let wrap = $state(true);
  let working = $state(false);

  let overlayName = $state("");
  let overlayInput: HTMLInputElement | undefined = $state();

  const disabled = $derived(busy || working || !doc);

  function hexToRgb(value: string): [number, number, number] {
    const clean = value.trim().replace(/^#/, "");
    if (/^[0-9a-fA-F]{6}$/.test(clean)) {
      return [
        Number.parseInt(clean.slice(0, 2), 16),
        Number.parseInt(clean.slice(2, 4), 16),
        Number.parseInt(clean.slice(4, 6), 16),
      ];
    }
    return [255, 255, 255];
  }

  async function addText(): Promise<void> {
    if (disabled) {
      return;
    }
    if (text.trim() === "") {
      notify(MESSAGES.textNeeded, "warn");
      return;
    }
    working = true;
    try {
      await api.editorAddText({
        text,
        color: [...hexToRgb(hex), alpha],
        size,
        x,
        y,
        rotation,
        opacity,
        shadow,
        outline,
        wrap,
      });
      notify(MESSAGES.textAdded);
      await refresh();
    } catch (cause) {
      notify(causeMessage(cause), "error");
    } finally {
      working = false;
    }
  }

  function chooseOverlay(): void {
    overlayInput?.click();
  }

  async function onOverlayChosen(event: Event): Promise<void> {
    const input = event.currentTarget as HTMLInputElement;
    const file = input.files?.[0];
    input.value = "";
    if (!file || disabled) {
      return;
    }
    working = true;
    try {
      const buffer = await file.arrayBuffer();
      await api.editorImportOverlay(
        Array.from(new Uint8Array(buffer)),
        overlayName.trim() === "" ? null : overlayName.trim(),
      );
      notify(MESSAGES.overlayAdded(overlayName.trim() || file.name));
      overlayName = "";
      await refresh();
    } catch (cause) {
      notify(causeMessage(cause), "error");
    } finally {
      working = false;
    }
  }
</script>

<div class="panel">
  <h3>{PANELS.header.text}</h3>

  <div class="panel-section">
    <h4>{PANELS.text.textLayer}</h4>
    <textarea
      class="text-input"
      rows="2"
      placeholder={PANELS.text.placeholder}
      aria-label={PANELS.text.placeholder}
      disabled={disabled}
      bind:value={text}
    ></textarea>
    <div class="panel-grid">
      <label class="panel-field">
        {PANELS.text.colour}
        <input type="color" bind:value={hex} disabled={disabled} />
      </label>
      <label class="panel-field">
        {PANELS.text.alpha}
        <input type="range" min="0" max="255" step="1" bind:value={alpha} disabled={disabled} />
      </label>
      <label class="panel-field">
        {PANELS.text.size}
        <input type="number" min="4" max="256" step="1" bind:value={size} disabled={disabled} />
      </label>
      <label class="panel-field">
        {PANELS.text.rotation}
        <input type="number" step="1" bind:value={rotation} disabled={disabled} />
      </label>
      <label class="panel-field">
        {PANELS.text.x}
        <input type="number" step="1" bind:value={x} disabled={disabled} />
      </label>
      <label class="panel-field">
        {PANELS.text.y}
        <input type="number" step="1" bind:value={y} disabled={disabled} />
      </label>
    </div>
    <div class="panel-row">
      <label class="panel-check">
        {PANELS.text.opacity}
        <input type="range" min="0" max="1" step="0.05" bind:value={opacity} disabled={disabled} />
      </label>
      <span class="panel-value">{Math.round(opacity * 100)}%</span>
    </div>
    <div class="panel-row">
      <label class="panel-check">
        <input type="checkbox" bind:checked={shadow} disabled={disabled} /> {PANELS.text.shadow}
      </label>
      <label class="panel-check">
        <input type="checkbox" bind:checked={outline} disabled={disabled} /> {PANELS.text.outline}
      </label>
      <label class="panel-check">
        <input type="checkbox" bind:checked={wrap} disabled={disabled} /> {PANELS.text.wrap}
      </label>
    </div>
    <div class="panel-buttons">
      <button class="btn primary" type="button" title={TOOLTIPS.text.addText} disabled={disabled} onclick={() => void addText()}>
        {PANELS.text.addText}
      </button>
    </div>
  </div>

  <div class="panel-section">
    <h4>{PANELS.text.overlay}</h4>
    <label class="panel-field">
      {PANELS.text.nameOptional}
      <input type="text" placeholder={PANELS.text.namePlaceholder} bind:value={overlayName} disabled={disabled} />
    </label>
    <div class="panel-buttons">
      <button class="btn" type="button" title={REFERENCE_TOOLTIPS.importOverlay} disabled={disabled} onclick={chooseOverlay}>
        {PANELS.text.importOverlay}
      </button>
    </div>
    <p class="panel-note">{PANELS.text.importOverlayHint}</p>
    <input
      class="hidden-file"
      type="file"
      accept="image/png,image/jpeg,image/bmp,image/gif,image/webp"
      aria-label={PANELS.text.importOverlay}
      bind:this={overlayInput}
      onchange={onOverlayChosen}
    />
  </div>

  {#if active}
    <LayersList state={doc} busy={busy || working} active={true} notify={notify} refresh={refresh} />
  {/if}
</div>

<style>
  .text-input {
    width: 100%;
    box-sizing: border-box;
    resize: vertical;
    background: var(--surface-1);
    border: 1px solid var(--line-strong);
    border-radius: var(--r-sm);
    padding: var(--sp-1);
    color: var(--text-0);
    font-size: var(--fs-sm);
    font-family: inherit;
  }

  .text-input:focus {
    border-color: var(--amber-dim);
    box-shadow: 0 0 0 2px var(--amber-soft);
  }

  .hidden-file {
    display: none;
  }
</style>
