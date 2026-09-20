<script lang="ts">
  /**
   * Size panel: image resize with the reference's size presets.
   *
   * "Constrain aspect" fits the source inside the target box instead of
   * stretching, and the resample choice is the pixel-art switch (nearest keeps
   * hard edges, bilinear smooths).
   */
  import * as api from "../../../api";
  import type { PanelContract } from "./contract";
  import { runOutcome } from "./actions";
  import { MESSAGES, PANELS, TOOLTIPS } from "../strings";
  import "./panel.css";

  let { state: doc, busy, active, notify, refresh }: PanelContract = $props();

  const PRESETS: ReadonlyArray<readonly [number, number]> = [
    [16, 16],
    [32, 32],
    [44, 44],
    [64, 64],
    [88, 88],
    [128, 128],
    [176, 176],
    [256, 256],
  ];

  let width = $state(64);
  let height = $state(64);
  let syncedFrom = $state("");
  let constrainAspect = $state(true);
  let resample = $state<api.EditorResample>("bilinear");
  let allFrames = $state(false);
  let working = $state(false);

  const disabled = $derived(busy || working || !active || !doc);

  $effect(() => {
    const marker = doc ? `${doc.width}x${doc.height}` : "";
    if (marker !== syncedFrom) {
      syncedFrom = marker;
      if (doc) {
        width = doc.width;
        height = doc.height;
      }
    }
  });

  function usePreset(preset: readonly [number, number]): void {
    width = preset[0];
    height = preset[1];
  }

  async function resize(): Promise<void> {
    if (disabled) {
      return;
    }
    working = true;
    try {
      await runOutcome(
        () =>
          api.editorResize({
            width: Math.max(1, Math.min(4096, Math.round(width))),
            height: Math.max(1, Math.min(4096, Math.round(height))),
            constrainAspect,
            resample,
            allFrames,
          }),
        notify,
        refresh,
        MESSAGES.resized(width, height),
      );
    } finally {
      working = false;
    }
  }
</script>

<div class="panel">
  <h3>{PANELS.header.size}</h3>

  <dl class="panel-facts">
    <div><dt>{PANELS.size.canvas}</dt><dd>{doc ? `${doc.width} × ${doc.height}` : "—"}</dd></div>
    <div><dt>{PANELS.size.frames}</dt><dd>{doc ? doc.frame_count : "—"}</dd></div>
  </dl>

  <div class="panel-section">
    <h4>{PANELS.size.imageSize}</h4>
    <div class="panel-grid">
      <label class="panel-field">{PANELS.size.width}<input type="number" min="1" max="4096" step="1" bind:value={width} disabled={disabled} /></label>
      <label class="panel-field">{PANELS.size.height}<input type="number" min="1" max="4096" step="1" bind:value={height} disabled={disabled} /></label>
    </div>
    <label class="panel-check">
      <input type="checkbox" bind:checked={constrainAspect} disabled={disabled} />
      {PANELS.size.constrainAspect}
    </label>
    <div class="panel-row">
      <label for="size-resample">{PANELS.size.resample}</label>
      <select id="size-resample" bind:value={resample} disabled={disabled}>
        <option value="bilinear">{PANELS.size.bilinear}</option>
        <option value="nearest">{PANELS.size.nearest}</option>
      </select>
    </div>
    <label class="panel-check">
      <input type="checkbox" bind:checked={allFrames} disabled={disabled} />
      {PANELS.common.applyAllFrames}
    </label>
  </div>

  <div class="panel-section">
    <h4>{PANELS.size.presets}</h4>
    <div class="panel-buttons">
      {#each PRESETS as preset (preset.join("x"))}
        <button
          class="btn"
          class:on={width === preset[0] && height === preset[1]}
          type="button"
          disabled={disabled}
          aria-pressed={width === preset[0] && height === preset[1]}
          aria-label={`Set size to ${preset[0]} by ${preset[1]}`}
          title={`${preset[0]} × ${preset[1]}`}
          onclick={() => usePreset(preset)}
        >
          {preset[0]}
        </button>
      {/each}
    </div>
  </div>

  <div class="panel-buttons">
    <button class="btn primary" type="button" title={TOOLTIPS.size.resize} disabled={disabled} onclick={() => void resize()}>
      {PANELS.size.resize}
    </button>
  </div>

  {#if !doc}
    <p class="panel-note warn">{PANELS.size.openDocument}</p>
  {/if}
</div>
