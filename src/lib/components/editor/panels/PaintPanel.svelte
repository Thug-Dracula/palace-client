<script lang="ts">
  /**
   * Paint panel: brush, eraser, flood fill, colour picker and clear.
   *
   * The current tool, colour, size, layer and frame scope live in the shell's
   * shared `paint` state, so a drag on the canvas paints with exactly what this
   * panel shows. The typed coordinate form still issues real Rust paint commands
   * (the pixel work and the undo snapshot live there) and remains the
   * keyboard-accessible way to place a stroke. "Apply to all frames" maps a
   * single stroke onto every frame.
   *
   * Tooltips use the exact PalaceChat wording for the paint tools (see
   * `REFERENCE_TOOLTIPS`), including its American "color".
   */
  import * as api from "../../../api";
  import { hexToRgba, type PaintState } from "../../../editorView";
  import type { PanelContract } from "./contract";
  import { causeMessage, rgbaHex, runOutcome } from "./actions";
  import { MESSAGES, PANELS, REFERENCE_TOOLTIPS, TOOLTIPS } from "../strings";
  import "./panel.css";

  let {
    state: doc,
    busy,
    active,
    notify,
    refresh,
    paint = $bindable(),
  }: PanelContract & { paint: PaintState } = $props();

  let tolerance = $state(30);
  let fromX = $state(0);
  let fromY = $state(0);
  let toX = $state(1);
  let toY = $state(1);
  let picked = $state("—");
  let working = $state(false);

  const disabled = $derived(busy || working || !active || !doc);
  const layerCount = $derived(doc?.layers.length ?? 0);

  $effect(() => {
    if (layerCount > 0 && paint.layer >= layerCount) {
      paint.layer = 0;
    }
  });

  function rgba(): number[] {
    return hexToRgba(paint.color);
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

  const stroke = (): api.EditorStrokeOptions => ({
    fromX,
    fromY,
    toX,
    toY,
    color: rgba(),
    size: paint.size,
    layer: paint.layer,
    allFrames: paint.allFrames,
  });

  const paintStroke = (): Promise<void> =>
    run(() => api.editorPaintStroke(stroke()), MESSAGES.painted(paint.size));
  const erase = (): Promise<void> =>
    run(() => api.editorEraseStroke(stroke()), MESSAGES.erased(paint.size));
  const fill = (): Promise<void> =>
    run(
      () =>
        api.editorFloodFill({
          x: Math.max(0, Math.round(fromX)),
          y: Math.max(0, Math.round(fromY)),
          color: rgba(),
          tolerance: tolerance / 100,
          layer: paint.layer,
          allFrames: paint.allFrames,
        }),
      MESSAGES.floodFilled,
    );

  async function pick(): Promise<void> {
    if (disabled) {
      return;
    }
    working = true;
    try {
      const color = await api.editorPickColor(
        paint.layer,
        Math.max(0, Math.round(fromX)),
        Math.max(0, Math.round(fromY)),
      );
      picked = rgbaHex(color);
      notify(color ? MESSAGES.pickedColour(picked) : MESSAGES.noColour);
    } catch (cause) {
      notify(causeMessage(cause), "error");
    } finally {
      working = false;
    }
  }

  const clear = (): Promise<void> =>
    run(() => api.editorClearFrame({ allFrames: paint.allFrames }), MESSAGES.clearedFrame);

  function usePicked(): void {
    if (picked !== "—") {
      paint.color = picked;
    }
  }
</script>

<div class="panel">
  <h3>{PANELS.header.paint}</h3>

  <div class="panel-section">
    <h4>{PANELS.paint.colourBrush}</h4>
    <div class="panel-row">
      <label for="paint-tool">{PANELS.paint.tool}</label>
      <select id="paint-tool" bind:value={paint.tool} disabled={disabled}>
        <option value="brush">{PANELS.paint.brush}</option>
        <option value="eraser">{PANELS.paint.eraser}</option>
      </select>
    </div>
    <div class="panel-row">
      <label>
        {PANELS.paint.colour}
        <input
          type="color"
          bind:value={paint.color}
          disabled={disabled}
          aria-label={PANELS.paint.brushColour}
          title={REFERENCE_TOOLTIPS.pick}
        />
      </label>
      <button class="btn ghost" type="button" disabled={disabled || picked === "—"} title={TOOLTIPS.paint.usePicked} onclick={usePicked}>
        {PANELS.paint.usePicked} {picked}
      </button>
    </div>
    <div class="panel-row">
      <label for="paint-size">{PANELS.paint.size}</label>
      <input id="paint-size" type="range" min="1" max="48" step="1" bind:value={paint.size} disabled={disabled} />
      <span class="panel-value">{paint.size}</span>
    </div>
    <div class="panel-row">
      <label for="paint-tolerance">{PANELS.paint.tolerance}</label>
      <input id="paint-tolerance" type="range" min="0" max="100" step="1" bind:value={tolerance} disabled={disabled} />
      <span class="panel-value">{tolerance}%</span>
    </div>
    <div class="panel-row">
      <label for="paint-layer">{PANELS.paint.layer}</label>
      <select id="paint-layer" bind:value={paint.layer} disabled={disabled}>
        {#each doc?.layers ?? [] as entry, index (entry.id)}
          <option value={index}>{index === 0 ? PANELS.paint.base : entry.name || `Layer ${index}`}</option>
        {/each}
      </select>
      <label class="panel-check">
        <input type="checkbox" bind:checked={paint.allFrames} disabled={disabled} />
        {PANELS.paint.allFrames}
      </label>
    </div>
  </div>

  <div class="panel-section">
    <h4>{PANELS.paint.stroke}</h4>
    <div class="panel-grid">
      <label class="panel-field">{PANELS.paint.fromX}<input type="number" step="1" bind:value={fromX} disabled={disabled} /></label>
      <label class="panel-field">{PANELS.paint.fromY}<input type="number" step="1" bind:value={fromY} disabled={disabled} /></label>
      <label class="panel-field">{PANELS.paint.toX}<input type="number" step="1" bind:value={toX} disabled={disabled} /></label>
      <label class="panel-field">{PANELS.paint.toY}<input type="number" step="1" bind:value={toY} disabled={disabled} /></label>
    </div>
    <div class="panel-buttons">
      <button class="btn primary" type="button" title={REFERENCE_TOOLTIPS.brush} disabled={disabled} onclick={() => void paintStroke()}>{PANELS.paint.brush}</button>
      <button class="btn" type="button" title={REFERENCE_TOOLTIPS.eraser} disabled={disabled} onclick={() => void erase()}>{PANELS.paint.eraser}</button>
      <button class="btn" type="button" title={REFERENCE_TOOLTIPS.fill} disabled={disabled} onclick={() => void fill()}>{PANELS.paint.fill}</button>
      <button class="btn" type="button" title={REFERENCE_TOOLTIPS.pick} disabled={disabled} onclick={() => void pick()}>{PANELS.paint.pick}</button>
      <button class="btn" type="button" title={REFERENCE_TOOLTIPS.clear} disabled={disabled} onclick={() => void clear()}>{PANELS.paint.clear}</button>
    </div>
  </div>

  <p class="panel-note">{PANELS.paint.note}</p>
</div>
