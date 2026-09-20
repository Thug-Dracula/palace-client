import type {
  EditorGuideGeometry,
  EditorGuideToggles,
  EditorOrigin,
  EditorState,
} from "./api";

/**
 * Pure view math for the prop editor's canvas.
 *
 * The editor document lives entirely in Rust; the webview only receives one
 * already-composited PNG per frame. Zoom and pan are therefore *presentation
 * only*: they move the `<img>` on screen and never touch the document. Keeping
 * the math here (rather than inside the component) lets it be unit-tested
 * without a DOM or a Tauri runtime.
 */

/**
 * Where the canvas image sits on screen, in CSS pixels relative to the canvas
 * box. `scale` multiplies the natural image size; `x`/`y` are the top-left
 * offset of the scaled image inside the box.
 */
export interface ViewTransform {
  scale: number;
  x: number;
  y: number;
}

/** Smallest zoom the canvas will honour. */
export const MIN_ZOOM = 0.25;
/** Largest zoom the canvas will honour. */
export const MAX_ZOOM = 32;

/** The discrete zoom ladder the `+`/`-` buttons and wheel climb. */
export const ZOOM_STEPS: readonly number[] = [
  0.25, 0.5, 0.75, 1, 1.5, 2, 3, 4, 6, 8, 12, 16, 24, 32,
];

/** Clamp a scale into the editor's zoom range; non-finite input means 1:1. */
export function clampScale(scale: number): number {
  if (!Number.isFinite(scale)) {
    return 1;
  }
  return Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, scale));
}

/**
 * The scale at which the whole image fits inside the viewport with `padding`
 * CSS pixels of breathing room on every side. Zero-area inputs report 1:1 so a
 * canvas that has not been measured yet does not collapse the view.
 */
export function fitScale(
  imageWidth: number,
  imageHeight: number,
  viewWidth: number,
  viewHeight: number,
  padding = 24,
): number {
  if (imageWidth <= 0 || imageHeight <= 0 || viewWidth <= 0 || viewHeight <= 0) {
    return 1;
  }
  const availableWidth = Math.max(1, viewWidth - padding * 2);
  const availableHeight = Math.max(1, viewHeight - padding * 2);
  return clampScale(Math.min(availableWidth / imageWidth, availableHeight / imageHeight));
}

/** Place a scaled image in the middle of the viewport. */
export function centeredTransform(
  scale: number,
  imageWidth: number,
  imageHeight: number,
  viewWidth: number,
  viewHeight: number,
): ViewTransform {
  return {
    scale,
    x: (viewWidth - imageWidth * scale) / 2,
    y: (viewHeight - imageHeight * scale) / 2,
  };
}

/** Fit the whole image, centred. What `Fit to Window` produces. */
export function fitTransform(
  imageWidth: number,
  imageHeight: number,
  viewWidth: number,
  viewHeight: number,
  padding = 24,
): ViewTransform {
  return centeredTransform(
    fitScale(imageWidth, imageHeight, viewWidth, viewHeight, padding),
    imageWidth,
    imageHeight,
    viewWidth,
    viewHeight,
  );
}

/** Show every source pixel as one CSS pixel, centred. What `Actual Size` does. */
export function actualSizeTransform(
  imageWidth: number,
  imageHeight: number,
  viewWidth: number,
  viewHeight: number,
): ViewTransform {
  return centeredTransform(1, imageWidth, imageHeight, viewWidth, viewHeight);
}

/**
 * Zoom to `nextScale` while keeping the image point under `(anchorX, anchorY)`
 * pinned to that same screen position. The anchor is in canvas-box CSS pixels,
 * so wheel-zoom follows the cursor instead of the box centre.
 */
export function zoomAt(
  view: ViewTransform,
  nextScale: number,
  anchorX: number,
  anchorY: number,
): ViewTransform {
  const scale = clampScale(nextScale);
  if (scale === view.scale) {
    return view;
  }
  const ratio = scale / view.scale;
  return {
    scale,
    x: anchorX - (anchorX - view.x) * ratio,
    y: anchorY - (anchorY - view.y) * ratio,
  };
}

/** Translate the view by a screen-space delta (drag pan). */
export function panBy(view: ViewTransform, dx: number, dy: number): ViewTransform {
  return { scale: view.scale, x: view.x + dx, y: view.y + dy };
}

/** The next rung up (`direction > 0`) or down (`direction < 0`) the zoom ladder. */
export function nextZoomStep(scale: number, direction: number): number {
  if (direction > 0) {
    for (const step of ZOOM_STEPS) {
      if (step > scale + 1e-9) {
        return step;
      }
    }
    return MAX_ZOOM;
  }
  for (let index = ZOOM_STEPS.length - 1; index >= 0; index -= 1) {
    if (ZOOM_STEPS[index] < scale - 1e-9) {
      return ZOOM_STEPS[index];
    }
  }
  return MIN_ZOOM;
}

/** One rung of the zoom ladder, anchored at a screen point. */
export function zoomStepAt(
  view: ViewTransform,
  direction: number,
  anchorX: number,
  anchorY: number,
): ViewTransform {
  return zoomAt(view, nextZoomStep(view.scale, direction), anchorX, anchorY);
}

/**
 * Keep at least `margin` CSS pixels of the image inside the viewport, so panning
 * can never fling the artwork completely out of sight. The window is a no-op
 * when the image or viewport has no area yet.
 */
export function clampPan(
  view: ViewTransform,
  imageWidth: number,
  imageHeight: number,
  viewWidth: number,
  viewHeight: number,
  margin = 32,
): ViewTransform {
  if (imageWidth <= 0 || imageHeight <= 0 || viewWidth <= 0 || viewHeight <= 0) {
    return view;
  }
  const width = imageWidth * view.scale;
  const height = imageHeight * view.scale;
  const keepX = Math.min(margin, width);
  const keepY = Math.min(margin, height);
  const minX = keepX - width;
  const maxX = viewWidth - keepX;
  const minY = keepY - height;
  const maxY = viewHeight - keepY;
  return {
    scale: view.scale,
    x: Math.min(maxX, Math.max(minX, view.x)),
    y: Math.min(maxY, Math.max(minY, view.y)),
  };
}

/** Human zoom readout, e.g. `150%`. */
export function formatZoom(scale: number): string {
  return `${Math.round(scale * 100)}%`;
}

/* ------------------------------------------------------------------ */
/* Pointer painting                                                    */
/* ------------------------------------------------------------------ */

/** The two drag tools the Paint panel offers. */
export type PaintTool = "brush" | "eraser";

/** A point on a layer's own pixel grid. */
export interface LayerPoint {
  x: number;
  y: number;
}

/**
 * The Paint panel state the canvas drag and the panel controls share.
 *
 * The shell owns one of these and hands it to both the panel (which edits it)
 * and the canvas drag (which reads the current tool, colour, size, layer and
 * frame scope). Keeping it a plain data shape lets the shell build a stroke
 * request without either component knowing about the other.
 */
export interface PaintState {
  tool: PaintTool;
  /** `#rrggbb`. */
  color: string;
  /** Stamp side in pixels, 1-48. */
  size: number;
  /** Layer index to paint on, 0 is the base. */
  layer: number;
  /** Apply the drag to every frame. */
  allFrames: boolean;
}

/**
 * Map a point in canvas-box CSS pixels to the layer pixel under it.
 *
 * The canvas draws the image at `view.x`/`view.y` with `view.scale` CSS pixels
 * per source pixel, so the inverse is `(screen - offset) / scale`. The result is
 * floored: a point anywhere inside a pixel paints that pixel. It may fall
 * outside the image (negative or past the far edge); the Rust paint ops clip
 * off-canvas pixels, so an edge drag that leaves and returns stays safe.
 *
 * A non-positive or non-finite scale is read as 1:1 so a view mid-fit can never
 * produce `Infinity`/`NaN` layer coordinates.
 */
export function screenToLayer(
  view: ViewTransform,
  screenX: number,
  screenY: number,
): LayerPoint {
  const scale = Number.isFinite(view.scale) && view.scale > 0 ? view.scale : 1;
  return {
    x: Math.floor((screenX - view.x) / scale),
    y: Math.floor((screenY - view.y) / scale),
  };
}

/** Whether two layer points address the same pixel. */
export function sameLayerPoint(a: LayerPoint, b: LayerPoint): boolean {
  return a.x === b.x && a.y === b.y;
}

/**
 * Parse a `#rrggbb` or `#rgb` colour into an opaque RGBA byte array.
 *
 * The Paint panel's `<input type="color">` always yields `#rrggbb`, but the
 * 3-digit shorthand and a missing `#` are accepted too. Anything unparseable
 * reads as opaque black, so a malformed value can never send NaN channels.
 */
export function hexToRgba(hex: string): number[] {
  let value = hex.trim().replace(/^#/, "");
  if (value.length === 3) {
    value = value
      .split("")
      .map((channel) => channel + channel)
      .join("");
  }
  if (!/^[0-9a-fA-F]{6}$/.test(value)) {
    return [0, 0, 0, 255];
  }
  return [
    parseInt(value.slice(0, 2), 16),
    parseInt(value.slice(2, 4), 16),
    parseInt(value.slice(4, 6), 16),
    255,
  ];
}

/* ------------------------------------------------------------------ */
/* Panel framework                                                     */
/* ------------------------------------------------------------------ */

/** The pinned panel set the editor shell exposes. */
export type PanelId =
  | "paint"
  | "transform"
  | "adjust"
  | "size"
  | "frames"
  | "text"
  | "guides";

/** One panel tab: a stable id plus its display label. */
export interface PanelTab {
  id: PanelId;
  label: string;
}

/** Tab strip order; also the order `cyclePanel` walks. */
export const PANEL_TABS: readonly PanelTab[] = [
  { id: "paint", label: "Paint" },
  { id: "transform", label: "Transform" },
  { id: "adjust", label: "Adjust" },
  { id: "size", label: "Size" },
  { id: "frames", label: "Frames" },
  { id: "text", label: "Text" },
  { id: "guides", label: "Guides" },
];

/** Narrow an arbitrary string to a known panel id. */
export function isPanelId(value: string): value is PanelId {
  return PANEL_TABS.some((tab) => tab.id === value);
}

/** The label for a panel id. */
export function panelLabel(id: PanelId): string {
  return PANEL_TABS.find((tab) => tab.id === id)?.label ?? id;
}

/**
 * Move `delta` tabs from `current`, wrapping at both ends. A delta of 0 (or a
 * bad current id) lands on the first tab so the strip can never be blank.
 */
export function cyclePanel(current: PanelId, delta: number): PanelId {
  const index = PANEL_TABS.findIndex((tab) => tab.id === current);
  if (index < 0) {
    return PANEL_TABS[0].id;
  }
  const count = PANEL_TABS.length;
  const next = (((index + delta) % count) + count) % count;
  return PANEL_TABS[next].id;
}

/* ------------------------------------------------------------------ */
/* Status line                                                         */
/* ------------------------------------------------------------------ */

/**
 * The prop id behind the open document, when it came from a bag prop. A blank
 * canvas and an imported image have no Palace id.
 */
export function propIdOf(origin: EditorOrigin | null | undefined): number | null {
  return origin?.kind === "bag_prop" ? origin.id : null;
}

/**
 * A human label for where the open document came from: `Untitled canvas`,
 * `Imported image`, or `My Bag #7 (Dot)`.
 */
export function originLabel(origin: EditorOrigin | null | undefined): string {
  if (!origin) {
    return "No document";
  }
  switch (origin.kind) {
    case "blank":
      return "Untitled canvas";
    case "image":
      return "Imported image";
    case "bag_prop": {
      const name = origin.name ? ` (${origin.name})` : "";
      return `${origin.collection} #${origin.id}${name}`;
    }
  }
}

/** `Frame 2 / 5`, or `No document` when nothing is open. */
export function frameInfo(state: EditorState | null): string {
  return state ? `Frame ${state.frame_index + 1} / ${state.frame_count}` : "No document";
}

/** `64 x 64` in canvas pixels, or an em dash when nothing is open. */
export function canvasSize(state: EditorState | null): string {
  return state ? `${state.width} × ${state.height}` : "—";
}

/** The save indicator's text: whether the document holds unsaved edits. */
export function saveLabel(state: EditorState | null): string {
  if (!state) {
    return "No document";
  }
  return state.dirty ? "Unsaved changes" : "No unsaved changes";
}

/* ------------------------------------------------------------------ */
/* Guide overlays                                                      */
/* ------------------------------------------------------------------ */

/** One onion-skin ghost: a neighbouring frame image plus how faintly to draw it. */
export interface GuideGhost {
  index: number;
  src: string;
  opacity: number;
}

/** Everything the canvas needs to draw guides over the current frame. */
export interface CanvasGuides {
  geometry: EditorGuideGeometry;
  toggles: EditorGuideToggles;
  onion: GuideGhost[];
}

/**
 * How faintly an onion ghost is drawn. The immediate neighbours are the
 * strongest, because they are the frames the motion actually came from and
 * goes to; distance 1 is the reference convention.
 */
export function onionGhostOpacity(offset: number): number {
  const distance = Math.abs(offset);
  if (distance <= 1) {
    return 0.4;
  }
  return Math.max(0.12, 0.4 / distance);
}
