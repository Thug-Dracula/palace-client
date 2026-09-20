<script lang="ts">
  /**
   * Adjust panel: brightness/contrast, colour (with saturation), and chroma-key
   * background removal.
   *
   * Sliders are -100..100 and are divided by 100 for Rust's -1.0..1.0 parameters;
   * 0 is the identity, so Reset simply zeroes the fields. Background removal
   * samples a key colour from the layer, then cuts it with a tolerance and a
   * soft feather band.
   */
  import * as api from "../../../api";
  import type { PanelContract } from "./contract";
  import { causeMessage, runOutcome } from "./actions";
  import { MESSAGES, PANELS, REFERENCE_TOOLTIPS, TOOLTIPS } from "../strings";
  import "./panel.css";

  let { state: doc, busy, active, notify, refresh }: PanelContract = $props();

  let allFrames = $state(false);
  let brightness = $state(0);
  let contrast = $state(0);
  let saturation = $state(0);
  let keyHex = $state("#00ff00");
  let sampleX = $state(0);
  let sampleY = $state(0);
  let tolerance = $state(20);
  let feather = $state(10);
  let working = $state(false);

  const disabled = $derived(busy || working || !active || !doc);

  function hexToRgb(hex: string): number[] {
    const value = hex.replace("#", "");
    return [
      parseInt(value.slice(0, 2), 16),
      parseInt(value.slice(2, 4), 16),
      parseInt(value.slice(4, 6), 16),
    ];
  }

  async function run(action: () => Promise<api.EditorOutcome>, success: string): Promise<void> {
    if (disabled) {
      return;
    }
    working = true;
    try {
      await runOutcome(action, notify, refresh, success);
    } finally {
      working = false;
    }
  }

  const applyBrightnessContrast = (): Promise<void> =>
    run(
      () =>
        api.editorAdjustBrightnessContrast({
          brightness: brightness / 100,
          contrast: contrast / 100,
          allFrames,
        }),
      MESSAGES.appliedTone,
    );

  const applyColour = (): Promise<void> =>
    run(
      () =>
        api.editorAdjustColour({
          brightness: brightness / 100,
          contrast: contrast / 100,
          saturation: saturation / 100,
          allFrames,
        }),
      MESSAGES.appliedColour,
    );

  async function sample(): Promise<void> {
    if (disabled) {
      return;
    }
    working = true;
    try {
      const color = await api.editorSampleKeyColor(
        0,
        Math.max(0, Math.round(sampleX)),
        Math.max(0, Math.round(sampleY)),
      );
      if (color) {
        keyHex = `#${color.map((channel) => channel.toString(16).padStart(2, "0")).join("")}`;
        notify(MESSAGES.sampledKey(keyHex));
      } else {
        notify(MESSAGES.transparentPixel, "warn");
      }
    } catch (cause) {
      notify(causeMessage(cause), "error");
    } finally {
      working = false;
    }
  }

  const removeBackground = (): Promise<void> =>
    run(
      () =>
        api.editorRemoveBackground({
          key: hexToRgb(keyHex),
          tolerance: tolerance / 100,
          feather: feather / 100,
          allFrames,
        }),
      MESSAGES.removedBackground,
    );

  function resetTone(): void {
    brightness = 0;
    contrast = 0;
  }

  function resetColour(): void {
    brightness = 0;
    contrast = 0;
    saturation = 0;
  }
</script>

<div class="panel">
  <h3>{PANELS.header.adjust}</h3>

  <label class="panel-check">
    <input type="checkbox" bind:checked={allFrames} disabled={disabled} />
    {PANELS.common.applyAllFrames}
  </label>

  <div class="panel-section">
    <h4>{PANELS.adjust.tone}</h4>
    <div class="panel-row">
      <label for="adj-brightness">{PANELS.adjust.brightness}</label>
      <input id="adj-brightness" type="range" min="-100" max="100" step="1" bind:value={brightness} disabled={disabled} />
      <span class="panel-value">{brightness}</span>
    </div>
    <div class="panel-row">
      <label for="adj-contrast">{PANELS.adjust.contrast}</label>
      <input id="adj-contrast" type="range" min="-100" max="100" step="1" bind:value={contrast} disabled={disabled} />
      <span class="panel-value">{contrast}</span>
    </div>
    <div class="panel-buttons">
      <button class="btn primary" type="button" title={TOOLTIPS.adjust.applyTone} disabled={disabled} onclick={() => void applyBrightnessContrast()}>
        {PANELS.adjust.apply}
      </button>
      <button class="btn ghost" type="button" title={TOOLTIPS.adjust.resetTone} disabled={disabled} onclick={resetTone}>{PANELS.adjust.reset}</button>
    </div>
  </div>

  <div class="panel-section">
    <h4>{PANELS.adjust.colour}</h4>
    <div class="panel-row">
      <label for="adj-saturation">{PANELS.adjust.saturation}</label>
      <input id="adj-saturation" type="range" min="-100" max="100" step="1" bind:value={saturation} disabled={disabled} />
      <span class="panel-value">{saturation}</span>
    </div>
    <div class="panel-buttons">
      <button class="btn primary" type="button" title={TOOLTIPS.adjust.applyColour} disabled={disabled} onclick={() => void applyColour()}>
        {PANELS.adjust.applyColour}
      </button>
      <button class="btn ghost" type="button" title={TOOLTIPS.adjust.resetColour} disabled={disabled} onclick={resetColour}>{PANELS.adjust.reset}</button>
    </div>
  </div>

  <div class="panel-section">
    <h4>{PANELS.adjust.backgroundRemoval}</h4>
    <div class="panel-row">
      <label>
        {PANELS.adjust.key}
        <input type="color" bind:value={keyHex} disabled={disabled} aria-label={PANELS.adjust.keyColour} />
      </label>
      <span class="panel-tag">{keyHex}</span>
    </div>
    <div class="panel-grid">
      <label class="panel-field">{PANELS.adjust.sampleX}<input type="number" step="1" bind:value={sampleX} disabled={disabled} /></label>
      <label class="panel-field">{PANELS.adjust.sampleY}<input type="number" step="1" bind:value={sampleY} disabled={disabled} /></label>
    </div>
    <div class="panel-buttons">
      <button class="btn ghost" type="button" disabled={disabled} title={REFERENCE_TOOLTIPS.sampleKey} onclick={() => void sample()}>
        {PANELS.adjust.samplePixel}
      </button>
    </div>
    <div class="panel-row">
      <label for="adj-tolerance">{PANELS.adjust.tolerance}</label>
      <input id="adj-tolerance" type="range" min="0" max="100" step="1" bind:value={tolerance} disabled={disabled} />
      <span class="panel-value">{tolerance}%</span>
    </div>
    <div class="panel-row">
      <label for="adj-feather">{PANELS.adjust.feather}</label>
      <input id="adj-feather" type="range" min="0" max="100" step="1" bind:value={feather} disabled={disabled} />
      <span class="panel-value">{feather}%</span>
    </div>
    <div class="panel-buttons">
      <button class="btn" type="button" title={TOOLTIPS.adjust.removeBackground} disabled={disabled} onclick={() => void removeBackground()}>
        {PANELS.adjust.removeBackground}
      </button>
    </div>
  </div>
</div>
