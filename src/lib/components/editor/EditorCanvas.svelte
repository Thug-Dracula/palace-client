<script lang="ts">
  /**
   * The editor's canvas: one already-composited PNG frame plus zoom, pan and
   * pointer painting.
   *
   * Presentational and interaction only. The shell owns the [`ViewTransform`],
   * the paint tool state and all document state; this component reports
   * pointer/wheel/keyboard intent back through its callbacks and its measured
   * box size through `onmetrics`. It never imports the api and never mutates the
   * document: zoom and pan are pure screen transforms, and a paint drag is
   * reported as layer-pixel segments for the shell to send to Rust.
   *
   * Gestures (documented choice):
   * - wheel zooms at the cursor, as before;
   * - middle-button drag pans, and a plain left drag pans when `painting` is
   *   false (no Paint panel / no document);
   * - with `painting` true, a plain left drag paints: `pointerdown` begins,
   *   `pointermove` extends, `pointerup`/`pointercancel` commits.
   *
   * Keyboard: the box takes focus and pans with the arrow keys and zooms with
   * `+`, `-` and `0`, so the canvas is usable without a pointer.
   */
  import { onMount } from "svelte";

  import {
    clampPan,
    fitTransform,
    panBy,
    sameLayerPoint,
    screenToLayer,
    zoomStepAt,
    type CanvasGuides,
    type LayerPoint,
    type ViewTransform,
  } from "../../editorView";
  import { CANVAS } from "./strings";

  let {
    /** Object URL (or data URL) of the current frame PNG; null shows the empty state. */
    src,
    /** Natural frame width in document pixels. */
    imageWidth,
    /** Natural frame height in document pixels. */
    imageHeight,
    /** The shell-owned view transform. */
    view,
    /** Called with the next transform after a wheel zoom, drag pan or key press. */
    onchange,
    /** Called with the measured canvas-box size in CSS pixels. */
    onmetrics,
    /** True when a plain left drag should paint; middle-drag still pans. */
    painting = false,
    /** Called once when a paint drag starts, with the layer pixel under the press. */
    onpaintbegin,
    /** Called as the paint drag moves, with the segment from the last pixel to this one. */
    onpaintextend,
    /** Called when a paint drag ends (pointerup or pointercancel). */
    onpaintend,
    /** Guide geometry, switches and onion ghosts; null draws no overlay. */
    guides = null,
  }: {
    src: string | null;
    imageWidth: number;
    imageHeight: number;
    view: ViewTransform;
    onchange: (next: ViewTransform) => void;
    onmetrics?: (width: number, height: number) => void;
    painting?: boolean;
    onpaintbegin?: (point: LayerPoint) => void;
    onpaintextend?: (from: LayerPoint, to: LayerPoint) => void;
    onpaintend?: () => void;
    guides?: CanvasGuides | null;
  } = $props();

  /**
   * What the current pointer interaction is doing:
   * - `none`: idle,
   * - `pan`: a middle-button drag, or a plain left drag while not painting,
   * - `paint`: a plain left drag on the Paint panel.
   */
  type DragMode = "none" | "pan" | "paint";

  let box: HTMLDivElement | undefined = $state();
  let dragMode = $state<DragMode>("none");
  let lastScreenX = 0;
  let lastScreenY = 0;
  let lastLayer: LayerPoint | null = null;
  let measured = { width: 0, height: 0 };

  const PAN_STEP = 32;

  function reportMetrics(): void {
    if (!box) {
      return;
    }
    const rect = box.getBoundingClientRect();
    const width = Math.round(rect.width);
    const height = Math.round(rect.height);
    if (width === measured.width && height === measured.height) {
      return;
    }
    measured = { width, height };
    onmetrics?.(width, height);
  }

  onMount(() => {
    const observer = new ResizeObserver(() => reportMetrics());
    if (box) {
      observer.observe(box);
    }
    reportMetrics();
    return () => observer.disconnect();
  });

  function bounds(): { width: number; height: number } {
    if (!box) {
      return { width: measured.width, height: measured.height };
    }
    const rect = box.getBoundingClientRect();
    return { width: rect.width, height: rect.height };
  }

  function onWheel(event: WheelEvent): void {
    if (!box || !src) {
      return;
    }
    event.preventDefault();
    const rect = box.getBoundingClientRect();
    const anchorX = event.clientX - rect.left;
    const anchorY = event.clientY - rect.top;
    const direction = event.deltaY < 0 ? 1 : -1;
    const zoomed = zoomStepAt(view, direction, anchorX, anchorY);
    const { width, height } = bounds();
    onchange(clampPan(zoomed, imageWidth, imageHeight, width, height));
  }

  /** The pointer position relative to the canvas box, in CSS pixels. */
  function localPoint(event: PointerEvent): { x: number; y: number } {
    if (!box) {
      return { x: 0, y: 0 };
    }
    const rect = box.getBoundingClientRect();
    return { x: event.clientX - rect.left, y: event.clientY - rect.top };
  }

  function onPointerDown(event: PointerEvent): void {
    if (!src) {
      return;
    }
    // Middle button always pans; a plain left drag paints when the Paint panel
    // is open and otherwise pans, exactly as it did before painting existed.
    const wantsPaint = painting && event.button === 0;
    const wantsPan = event.button === 1 || (!painting && event.button === 0);
    if (!wantsPaint && !wantsPan) {
      return;
    }
    event.preventDefault();
    (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);

    if (wantsPaint) {
      dragMode = "paint";
      const local = localPoint(event);
      const point = screenToLayer(view, local.x, local.y);
      lastLayer = point;
      onpaintbegin?.(point);
      return;
    }
    dragMode = "pan";
    lastScreenX = event.clientX;
    lastScreenY = event.clientY;
  }

  function onPointerMove(event: PointerEvent): void {
    if (dragMode === "pan") {
      const dx = event.clientX - lastScreenX;
      const dy = event.clientY - lastScreenY;
      lastScreenX = event.clientX;
      lastScreenY = event.clientY;
      const { width, height } = bounds();
      onchange(clampPan(panBy(view, dx, dy), imageWidth, imageHeight, width, height));
      return;
    }
    if (dragMode !== "paint") {
      return;
    }
    const local = localPoint(event);
    const point = screenToLayer(view, local.x, local.y);
    if (lastLayer && sameLayerPoint(lastLayer, point)) {
      return;
    }
    const from = lastLayer ?? point;
    lastLayer = point;
    onpaintextend?.(from, point);
  }

  function endPointer(event: PointerEvent): void {
    if (dragMode === "none") {
      return;
    }
    if (dragMode === "paint" && lastLayer) {
      // Send the release point as a final segment so a fast flick is not cut
      // short, then let the shell commit the drag.
      const local = localPoint(event);
      const point = screenToLayer(view, local.x, local.y);
      if (!sameLayerPoint(lastLayer, point)) {
        onpaintextend?.(lastLayer, point);
      }
      onpaintend?.();
    }
    dragMode = "none";
    lastLayer = null;
    const target = event.currentTarget as HTMLElement;
    if (target.hasPointerCapture(event.pointerId)) {
      target.releasePointerCapture(event.pointerId);
    }
  }

  function onKeydown(event: KeyboardEvent): void {
    if (!src) {
      return;
    }
    const { width, height } = bounds();
    let next: ViewTransform | null = null;
    switch (event.key) {
      case "ArrowLeft":
        next = panBy(view, PAN_STEP, 0);
        break;
      case "ArrowRight":
        next = panBy(view, -PAN_STEP, 0);
        break;
      case "ArrowUp":
        next = panBy(view, 0, PAN_STEP);
        break;
      case "ArrowDown":
        next = panBy(view, 0, -PAN_STEP);
        break;
      case "+":
      case "=":
        next = zoomStepAt(view, 1, width / 2, height / 2);
        break;
      case "-":
      case "_":
        next = zoomStepAt(view, -1, width / 2, height / 2);
        break;
      case "0":
        next = fitTransform(imageWidth, imageHeight, width, height);
        break;
      default:
        return;
    }
    event.preventDefault();
    event.stopPropagation();
    onchange(clampPan(next, imageWidth, imageHeight, width, height));
  }
</script>

<!-- Deliberate a11y suppression: role="application" is focusable and owns pan/zoom. -->
<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div
  class="canvas-box"
  class:panning={dragMode === "pan"}
  class:painting
  bind:this={box}
  role="application"
  aria-label={CANVAS.label}
  aria-describedby="editor-canvas-help"
  aria-keyshortcuts="ArrowLeft ArrowRight ArrowUp ArrowDown + - 0"
  tabindex="0"
  onwheel={onWheel}
  onpointerdown={onPointerDown}
  onpointermove={onPointerMove}
  onpointerup={endPointer}
  onpointercancel={endPointer}
  onkeydown={onKeydown}
  oncontextmenu={(event) => event.preventDefault()}
>
  {#if src}
    {#if guides}
      {#each guides.onion as ghost (ghost.index)}
        <img
          class="ghost"
          src={ghost.src}
          alt=""
          draggable="false"
          style="left:{view.x}px; top:{view.y}px; width:{imageWidth * view.scale}px; height:{imageHeight * view.scale}px; opacity:{ghost.opacity};"
        />
      {/each}
    {/if}
    <img
      class="frame"
      {src}
      alt={CANVAS.frameAlt}
      draggable="false"
      style="left:{view.x}px; top:{view.y}px; width:{imageWidth * view.scale}px; height:{imageHeight * view.scale}px;"
    />
    {#if guides && imageWidth > 0 && imageHeight > 0}
      <!-- View-only overlay: SVG strokes over the frame, never composited into it. -->
      <svg
        class="guide-layer"
        aria-hidden="true"
        style="left:{view.x}px; top:{view.y}px; width:{imageWidth * view.scale}px; height:{imageHeight * view.scale}px;"
        viewBox="0 0 {imageWidth} {imageHeight}"
      >
        {#if guides.toggles.grid}
          {#each guides.geometry.grid.vertical as line, index (index)}
            <line class="g-grid" x1={line.from.x} y1={line.from.y} x2={line.to.x} y2={line.to.y} vector-effect="non-scaling-stroke" />
          {/each}
          {#each guides.geometry.grid.horizontal as line, index (index)}
            <line class="g-grid" x1={line.from.x} y1={line.from.y} x2={line.to.x} y2={line.to.y} vector-effect="non-scaling-stroke" />
          {/each}
        {/if}
        {#if guides.toggles.snap_centre}
          <line
            class="g-centre"
            x1={guides.geometry.centre.vertical.from.x}
            y1={guides.geometry.centre.vertical.from.y}
            x2={guides.geometry.centre.vertical.to.x}
            y2={guides.geometry.centre.vertical.to.y}
            vector-effect="non-scaling-stroke"
          />
          <line
            class="g-centre"
            x1={guides.geometry.centre.horizontal.from.x}
            y1={guides.geometry.centre.horizontal.from.y}
            x2={guides.geometry.centre.horizontal.to.x}
            y2={guides.geometry.centre.horizontal.to.y}
            vector-effect="non-scaling-stroke"
          />
        {/if}
        {#if guides.toggles.thirds}
          {#each guides.geometry.thirds.vertical as line, index (index)}
            <line class="g-thirds" x1={line.from.x} y1={line.from.y} x2={line.to.x} y2={line.to.y} vector-effect="non-scaling-stroke" />
          {/each}
          {#each guides.geometry.thirds.horizontal as line, index (index)}
            <line class="g-thirds" x1={line.from.x} y1={line.from.y} x2={line.to.x} y2={line.to.y} vector-effect="non-scaling-stroke" />
          {/each}
        {/if}
        {#if guides.toggles.safe_area}
          <rect
            class="g-safe"
            x={guides.geometry.safe_area.x}
            y={guides.geometry.safe_area.y}
            width={guides.geometry.safe_area.width}
            height={guides.geometry.safe_area.height}
            vector-effect="non-scaling-stroke"
          />
        {/if}
      </svg>
    {/if}
  {:else}
    <div class="canvas-empty">
      <div class="big">{CANVAS.emptyTitle}</div>
      <p>{CANVAS.emptyHint}</p>
    </div>
  {/if}
  <p id="editor-canvas-help" class="sr-only">{CANVAS.instructions}</p>
</div>

<style>
  .canvas-box {
    position: relative;
    overflow: hidden;
    min-height: 0;
    background-color: var(--bg-void);
    background-image:
      linear-gradient(rgba(255, 255, 255, 0.03) 1px, transparent 1px),
      linear-gradient(90deg, rgba(255, 255, 255, 0.03) 1px, transparent 1px);
    background-size: 24px 24px;
    cursor: grab;
    touch-action: none;
  }

  /* A plain left drag paints while the Paint panel is open; the pan cursor
     takes precedence so a middle-button drag still reads as a pan. */
  .canvas-box.painting {
    cursor: crosshair;
  }

  .canvas-box.panning {
    cursor: grabbing;
  }

  /* Transparency checkerboard sits behind the PNG, so alpha reads as
     "empty" instead of black. */
  .frame {
    position: absolute;
    display: block;
    image-rendering: pixelated;
    image-rendering: crisp-edges;
    user-select: none;
    -webkit-user-select: none;
    -webkit-user-drag: none;
    box-shadow: 0 0 0 1px var(--line-strong), 0 12px 40px rgba(0, 0, 0, 0.55);
    background-color: #1a1f26;
    background-image:
      linear-gradient(45deg, #232a34 25%, transparent 25%),
      linear-gradient(-45deg, #232a34 25%, transparent 25%),
      linear-gradient(45deg, transparent 75%, #232a34 75%),
      linear-gradient(-45deg, transparent 75%, #232a34 75%);
    background-size: 16px 16px;
    background-position: 0 0, 0 8px, 8px -8px, -8px 0;
  }

  /* Onion ghosts sit behind the current frame so the frame stays readable. */
  .ghost {
    position: absolute;
    display: block;
    image-rendering: pixelated;
    image-rendering: crisp-edges;
    user-select: none;
    -webkit-user-select: none;
    -webkit-user-drag: none;
    pointer-events: none;
  }

  /* The guide overlay is pure paint over the frame image; it is never merged
     into the composited PNG, so it can never reach the document. */
  .guide-layer {
    position: absolute;
    display: block;
    pointer-events: none;
    overflow: visible;
  }

  .guide-layer line,
  .guide-layer rect {
    fill: none;
  }

  .g-grid {
    stroke: rgba(255, 255, 255, 0.16);
    stroke-width: 1;
  }

  .g-centre {
    stroke: var(--cyan);
    stroke-width: 1;
    stroke-opacity: 0.9;
  }

  .g-thirds {
    stroke: var(--amber);
    stroke-width: 1;
    stroke-opacity: 0.55;
    stroke-dasharray: 4 4;
  }

  .g-safe {
    stroke: var(--amber-dim);
    stroke-width: 1;
    stroke-dasharray: 6 4;
  }

  .canvas-empty {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: var(--sp-2);
    padding: var(--sp-4);
    text-align: center;
    color: var(--text-2);
    cursor: default;
  }

  .canvas-empty .big {
    font-size: var(--fs-lg);
    letter-spacing: 0.16em;
    text-transform: uppercase;
    color: var(--amber);
  }

  .canvas-empty p {
    margin: 0;
    max-width: 42ch;
    font-size: var(--fs-sm);
    line-height: 1.7;
  }

  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    margin: -1px;
    padding: 0;
    overflow: hidden;
    clip: rect(0 0 0 0);
    white-space: nowrap;
    border: 0;
  }
</style>
