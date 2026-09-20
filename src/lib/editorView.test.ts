import { describe, expect, it } from "vitest";

import type { EditorState } from "./api";
import {
  MAX_ZOOM,
  MIN_ZOOM,
  PANEL_TABS,
  actualSizeTransform,
  canvasSize,
  clampPan,
  clampScale,
  cyclePanel,
  fitScale,
  fitTransform,
  formatZoom,
  frameInfo,
  hexToRgba,
  isPanelId,
  nextZoomStep,
  onionGhostOpacity,
  originLabel,
  panelLabel,
  panBy,
  propIdOf,
  saveLabel,
  sameLayerPoint,
  screenToLayer,
  zoomAt,
  zoomStepAt,
  type ViewTransform,
} from "./editorView";

const view = (over: Partial<ViewTransform> = {}): ViewTransform => ({
  scale: 1,
  x: 0,
  y: 0,
  ...over,
});

const state = (over: Partial<EditorState> = {}): EditorState => ({
  origin: { kind: "blank" },
  width: 64,
  height: 32,
  frame_count: 3,
  frame_index: 1,
  frames: [],
  layers: [],
  can_undo: false,
  can_redo: false,
  can_paste: false,
  undo_depth: 0,
  redo_depth: 0,
  undo_bytes: 0,
  dirty: false,
  ...over,
});

describe("clampScale", () => {
  it("clamps into the zoom range", () => {
    expect(clampScale(0.001)).toBe(MIN_ZOOM);
    expect(clampScale(9999)).toBe(MAX_ZOOM);
    expect(clampScale(2.5)).toBe(2.5);
  });

  it("falls back to 1:1 for non-finite input", () => {
    expect(clampScale(Number.NaN)).toBe(1);
    expect(clampScale(Number.POSITIVE_INFINITY)).toBe(1);
  });
});

describe("fitScale / fitTransform", () => {
  it("fits the image inside the padded viewport", () => {
    // 100x50 image, 300x200 box, 20px padding -> min((300-40)/100, (200-40)/50) = 2.6
    expect(fitScale(100, 50, 300, 200, 20)).toBeCloseTo(2.6);
  });

  it("never exceeds the source at 1 CSS pixel per source pixel", () => {
    // A 16x16 sprite in a 600x600 box would otherwise want 34x.
    expect(fitScale(16, 16, 600, 600)).toBeLessThanOrEqual(MAX_ZOOM);
  });

  it("centres the fitted image", () => {
    const transform = fitTransform(100, 50, 300, 200, 20);
    expect(transform.scale).toBeCloseTo(2.6);
    expect(transform.x).toBeCloseTo((300 - 100 * 2.6) / 2);
    expect(transform.y).toBeCloseTo((200 - 50 * 2.6) / 2);
  });

  it("reports 1:1 while the viewport is unmeasured", () => {
    expect(fitScale(64, 64, 0, 0)).toBe(1);
  });

  it("actual size is always the centred 1:1 view", () => {
    expect(actualSizeTransform(40, 20, 200, 100)).toEqual({
      scale: 1,
      x: 80,
      y: 40,
    });
  });
});

describe("zoomAt", () => {
  it("keeps the anchored image point fixed on screen", () => {
    const before = view({ scale: 2, x: 10, y: 20 });
    const anchorX = 130;
    const anchorY = 90;
    // The image point under the anchor before the zoom.
    const imageX = (anchorX - before.x) / before.scale;
    const imageY = (anchorY - before.y) / before.scale;

    const after = zoomAt(before, 4, anchorX, anchorY);

    expect(after.scale).toBe(4);
    expect(after.x + imageX * after.scale).toBeCloseTo(anchorX);
    expect(after.y + imageY * after.scale).toBeCloseTo(anchorY);
  });

  it("clamps the target scale", () => {
    expect(zoomAt(view(), 1000, 0, 0).scale).toBe(MAX_ZOOM);
    expect(zoomAt(view(), 0.0001, 0, 0).scale).toBe(MIN_ZOOM);
  });

  it("is a no-op when the scale does not change", () => {
    const before = view({ scale: 2, x: 3, y: 4 });
    expect(zoomAt(before, 2, 50, 50)).toBe(before);
  });

  it("zoomStepAt climbs and descends the ladder around the anchor", () => {
    const up = zoomStepAt(view({ scale: 1 }), 1, 0, 0);
    expect(up.scale).toBe(1.5);
    const down = zoomStepAt(view({ scale: 1 }), -1, 0, 0);
    expect(down.scale).toBe(0.75);
  });
});

describe("nextZoomStep", () => {
  it("walks the ladder in both directions", () => {
    expect(nextZoomStep(0.25, -1)).toBe(MIN_ZOOM);
    expect(nextZoomStep(0.25, 1)).toBe(0.5);
    expect(nextZoomStep(1, 1)).toBe(1.5);
    expect(nextZoomStep(1, -1)).toBe(0.75);
    expect(nextZoomStep(MAX_ZOOM, 1)).toBe(MAX_ZOOM);
  });
});

describe("panBy", () => {
  it("translates without touching the scale", () => {
    expect(panBy(view({ scale: 3, x: 5, y: 6 }), -2, 4)).toEqual({
      scale: 3,
      x: 3,
      y: 10,
    });
  });
});

describe("clampPan", () => {
  it("keeps a margin of the image on screen", () => {
    const clamped = clampPan(view({ scale: 1, x: 10_000, y: -10_000 }), 64, 64, 400, 300);
    expect(clamped.x).toBeLessThanOrEqual(400 - 32);
    expect(clamped.y).toBeGreaterThanOrEqual(32 - 64);
  });

  it("does not move a view that is already inside the window", () => {
    const inside = view({ scale: 1, x: 100, y: 80 });
    expect(clampPan(inside, 64, 64, 400, 300)).toEqual(inside);
  });

  it("is a no-op before the image or viewport is measured", () => {
    const before = view({ x: 999 });
    expect(clampPan(before, 0, 0, 400, 300)).toBe(before);
    expect(clampPan(before, 64, 64, 0, 0)).toBe(before);
  });
});

describe("formatZoom", () => {
  it("renders a percentage", () => {
    expect(formatZoom(1)).toBe("100%");
    expect(formatZoom(0.25)).toBe("25%");
    expect(formatZoom(2.5)).toBe("250%");
  });
});

describe("screenToLayer", () => {
  it("maps box pixels to image pixels at 1:1 with no offset", () => {
    expect(screenToLayer(view(), 10, 20)).toEqual({ x: 10, y: 20 });
  });

  it("subtracts the pan offset before scaling", () => {
    // Image drawn at (40, 60); a point 10 CSS px in is 10 image px in.
    expect(screenToLayer(view({ scale: 1, x: 40, y: 60 }), 50, 80)).toEqual({ x: 10, y: 20 });
  });

  it("divides by the zoom so a zoomed pixel maps back to itself", () => {
    const zoomed = view({ scale: 4, x: -100, y: -40 });
    // Image pixel (30, 12) sits at 4*30-100=20, 4*12-40=8 on screen.
    expect(screenToLayer(zoomed, 20, 8)).toEqual({ x: 30, y: 12 });
  });

  it("floors fractional positions so a point inside a pixel selects it", () => {
    expect(screenToLayer(view({ scale: 2, x: 3, y: 5 }), 10, 12)).toEqual({ x: 3, y: 3 });
  });

  it("reports off-canvas points without clamping them", () => {
    expect(screenToLayer(view({ scale: 2, x: 100, y: 100 }), 90, 190)).toEqual({ x: -5, y: 45 });
  });

  it("treats a non-positive scale as 1:1 instead of producing infinities", () => {
    expect(screenToLayer(view({ scale: 0, x: 0, y: 0 }), 5, 6)).toEqual({ x: 5, y: 6 });
    expect(screenToLayer(view({ scale: Number.NaN, x: 0, y: 0 }), 5, 6)).toEqual({ x: 5, y: 6 });
  });
});

describe("sameLayerPoint", () => {
  it("compares pixel coordinates", () => {
    expect(sameLayerPoint({ x: 1, y: 2 }, { x: 1, y: 2 })).toBe(true);
    expect(sameLayerPoint({ x: 1, y: 2 }, { x: 2, y: 1 })).toBe(false);
  });
});

describe("hexToRgba", () => {
  it("parses six-digit colours as opaque RGBA", () => {
    expect(hexToRgba("#ff8000")).toEqual([255, 128, 0, 255]);
    expect(hexToRgba("00ff00")).toEqual([0, 255, 0, 255]);
  });

  it("expands the three-digit shorthand", () => {
    expect(hexToRgba("#f0a")).toEqual([255, 0, 170, 255]);
  });

  it("falls back to opaque black for malformed input", () => {
    expect(hexToRgba("#xyz")).toEqual([0, 0, 0, 255]);
    expect(hexToRgba("")).toEqual([0, 0, 0, 255]);
  });
});

describe("panel framework", () => {
  it("exposes the pinned seven tabs in order", () => {
    expect(PANEL_TABS.map((tab) => tab.id)).toEqual([
      "paint",
      "transform",
      "adjust",
      "size",
      "frames",
      "text",
      "guides",
    ]);
  });

  it("narrows ids and labels them", () => {
    expect(isPanelId("paint")).toBe(true);
    expect(isPanelId("effects")).toBe(false);
    expect(panelLabel("frames")).toBe("Frames");
  });

  it("cycles forward and backward with wraparound", () => {
    expect(cyclePanel("paint", 1)).toBe("transform");
    expect(cyclePanel("guides", 1)).toBe("paint");
    expect(cyclePanel("paint", -1)).toBe("guides");
    expect(cyclePanel("paint", 0)).toBe("paint");
  });
});

describe("onionGhostOpacity", () => {
  it("draws the immediate neighbours strongest, either direction", () => {
    expect(onionGhostOpacity(1)).toBe(0.4);
    expect(onionGhostOpacity(-1)).toBe(0.4);
  });

  it("fades further neighbours but never to nothing", () => {
    expect(onionGhostOpacity(2)).toBeLessThan(onionGhostOpacity(1));
    expect(onionGhostOpacity(8)).toBeGreaterThanOrEqual(0.12);
    expect(onionGhostOpacity(99)).toBe(0.12);
  });
});

describe("status line helpers", () => {
  it("reads the prop id only from a bag prop", () => {
    expect(propIdOf({ kind: "blank" })).toBeNull();
    expect(propIdOf({ kind: "image" })).toBeNull();
    expect(
      propIdOf({ kind: "bag_prop", collection: "My Bag", id: 7, crc: 9, name: "Dot" }),
    ).toBe(7);
  });

  it("labels each origin", () => {
    expect(originLabel(null)).toBe("No document");
    expect(originLabel({ kind: "blank" })).toBe("Untitled canvas");
    expect(originLabel({ kind: "image" })).toBe("Imported image");
    expect(
      originLabel({ kind: "bag_prop", collection: "My Bag", id: 7, crc: 9, name: "Dot" }),
    ).toBe("My Bag #7 (Dot)");
    expect(
      originLabel({ kind: "bag_prop", collection: "My Bag", id: 7, crc: 9, name: null }),
    ).toBe("My Bag #7");
  });

  it("reports frame, size and save state", () => {
    expect(frameInfo(null)).toBe("No document");
    expect(frameInfo(state())).toBe("Frame 2 / 3");
    expect(canvasSize(null)).toBe("—");
    expect(canvasSize(state())).toBe("64 × 32");
    expect(saveLabel(null)).toBe("No document");
    expect(saveLabel(state({ dirty: false }))).toBe("No unsaved changes");
    expect(saveLabel(state({ dirty: true }))).toBe("Unsaved changes");
  });
});
