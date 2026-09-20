<script lang="ts">
  /**
   * Transform panel: flip, rotate, free crop, square crop and shape crop.
   *
   * Every control sends a Rust transform command; the checkbox maps the same
   * operation over every frame. Rotation is an exact right angle (lossless);
   * shape crop adds zoom and arbitrary-angle rotation.
   */
  import * as api from "../../../api";
  import type { PanelContract } from "./contract";
  import { runOutcome } from "./actions";
  import { MESSAGES, PANELS, TOOLTIPS } from "../strings";
  import "./panel.css";

  let { state: doc, busy, active, notify, refresh }: PanelContract = $props();

  let allFrames = $state(false);
  let cropX = $state(0);
  let cropY = $state(0);
  let cropWidth = $state(16);
  let cropHeight = $state(16);
  let shapeKind = $state<"square" | "circle" | "rounded">("circle");
  let radius = $state(6);
  let zoom = $state(1);
  let rotation = $state(0);
  let resample = $state<api.EditorResample>("bilinear");
  let working = $state(false);

  const disabled = $derived(busy || working || !active || !doc);

  $effect(() => {
    if (doc) {
      cropWidth = doc.width;
      cropHeight = doc.height;
    }
  });

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

  const flip = (axis: api.EditorFlipAxis): Promise<void> =>
    run(() => api.editorFlip({ axis, allFrames }), MESSAGES.flipped(axis));
  const rotate = (turn: api.EditorQuarterTurn): Promise<void> =>
    run(() => api.editorRotate({ turn, allFrames }), MESSAGES.rotated(turn));
  const crop = (): Promise<void> =>
    run(
      () =>
        api.editorCrop({
          x: Math.max(0, Math.round(cropX)),
          y: Math.max(0, Math.round(cropY)),
          width: Math.max(1, Math.round(cropWidth)),
          height: Math.max(1, Math.round(cropHeight)),
          allFrames,
        }),
      MESSAGES.cropped,
    );
  const cropSquare = (): Promise<void> =>
    run(() => api.editorCropSquare({ allFrames }), MESSAGES.croppedSquare);

  const shapeCrop = (): Promise<void> =>
    run(
      () =>
        api.editorShapeCrop({
          shape:
            shapeKind === "rounded"
              ? { kind: "rounded", radius: Math.max(0, Math.round(radius)) }
              : { kind: shapeKind },
          zoom,
          rotation,
          resample,
          allFrames,
        }),
      MESSAGES.shapeCropped,
    );
</script>

<div class="panel">
  <h3>{PANELS.header.transform}</h3>

  <label class="panel-check">
    <input type="checkbox" bind:checked={allFrames} disabled={disabled} />
    {PANELS.common.applyAllFrames}
  </label>

  <div class="panel-section">
    <h4>{PANELS.transform.flipRotate}</h4>
    <div class="panel-buttons">
      <button class="btn" type="button" title={TOOLTIPS.transform.flipHorizontal} disabled={disabled} onclick={() => void flip("horizontal")}>{PANELS.transform.flipH}</button>
      <button class="btn" type="button" title={TOOLTIPS.transform.flipVertical} disabled={disabled} onclick={() => void flip("vertical")}>{PANELS.transform.flipV}</button>
    </div>
    <div class="panel-buttons">
      <button class="btn" type="button" title={TOOLTIPS.transform.rotateCW} disabled={disabled} onclick={() => void rotate("clockwise")}>{PANELS.transform.rotateCW}</button>
      <button class="btn" type="button" title={TOOLTIPS.transform.rotateCCW} disabled={disabled} onclick={() => void rotate("counter_clockwise")}>{PANELS.transform.rotateCCW}</button>
      <button class="btn" type="button" title={TOOLTIPS.transform.rotate180} disabled={disabled} onclick={() => void rotate("half")}>{PANELS.transform.rotate180}</button>
    </div>
  </div>

  <div class="panel-section">
    <h4>{PANELS.transform.crop}</h4>
    <div class="panel-grid">
      <label class="panel-field">{PANELS.transform.x}<input type="number" step="1" bind:value={cropX} disabled={disabled} /></label>
      <label class="panel-field">{PANELS.transform.y}<input type="number" step="1" bind:value={cropY} disabled={disabled} /></label>
      <label class="panel-field">{PANELS.transform.width}<input type="number" min="1" step="1" bind:value={cropWidth} disabled={disabled} /></label>
      <label class="panel-field">{PANELS.transform.height}<input type="number" min="1" step="1" bind:value={cropHeight} disabled={disabled} /></label>
    </div>
    <div class="panel-buttons">
      <button class="btn" type="button" title={TOOLTIPS.transform.crop} disabled={disabled} onclick={() => void crop()}>{PANELS.transform.cropButton}</button>
      <button class="btn" type="button" title={TOOLTIPS.transform.cropSquare} disabled={disabled} onclick={() => void cropSquare()}>{PANELS.transform.squareCrop}</button>
    </div>
  </div>

  <div class="panel-section">
    <h4>{PANELS.transform.shapeCrop}</h4>
    <div class="panel-row">
      <label for="shape-kind">{PANELS.transform.shape}</label>
      <select id="shape-kind" bind:value={shapeKind} disabled={disabled}>
        <option value="square">{PANELS.transform.square}</option>
        <option value="circle">{PANELS.transform.circle}</option>
        <option value="rounded">{PANELS.transform.rounded}</option>
      </select>
    </div>
    {#if shapeKind === "rounded"}
      <div class="panel-row">
        <label for="shape-radius">{PANELS.transform.radius}</label>
        <input id="shape-radius" type="range" min="0" max="64" step="1" bind:value={radius} disabled={disabled} />
        <span class="panel-value">{radius}</span>
      </div>
    {/if}
    <div class="panel-row">
      <label for="shape-zoom">{PANELS.transform.zoom}</label>
      <input id="shape-zoom" type="range" min="0.1" max="4" step="0.1" bind:value={zoom} disabled={disabled} />
      <span class="panel-value">{zoom.toFixed(1)}×</span>
    </div>
    <div class="panel-row">
      <label for="shape-rotation">{PANELS.transform.rotation}</label>
      <input id="shape-rotation" type="range" min="-180" max="180" step="1" bind:value={rotation} disabled={disabled} />
      <span class="panel-value">{Math.round(rotation)}°</span>
    </div>
    <div class="panel-row">
      <label for="shape-resample">{PANELS.transform.resample}</label>
      <select id="shape-resample" bind:value={resample} disabled={disabled}>
        <option value="bilinear">{PANELS.transform.bilinear}</option>
        <option value="nearest">{PANELS.transform.nearest}</option>
      </select>
    </div>
    <div class="panel-buttons">
      <button class="btn primary" type="button" title={TOOLTIPS.transform.shapeCrop} disabled={disabled} onclick={() => void shapeCrop()}>
        {PANELS.transform.applyShape}
      </button>
    </div>
  </div>
</div>
