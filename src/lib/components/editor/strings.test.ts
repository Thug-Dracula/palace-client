import { describe, expect, it } from "vitest";

import { PANEL_TABS } from "../../editorView";
import {
  BAG,
  CANVAS,
  EDITOR,
  MESSAGES,
  PANELS,
  PANEL_LABELS,
  REFERENCE_TOOLTIPS,
  TOOLTIPS,
  withShortcut,
} from "./strings";

/** Walk every literal leaf of the string tables. Functions are skipped. */
function leaves(value: unknown, path = ""): Array<[string, string]> {
  if (typeof value === "string") {
    return [[path, value]];
  }
  if (value && typeof value === "object") {
    return Object.entries(value as Record<string, unknown>).flatMap(([key, child]) =>
      leaves(child, path ? `${path}.${key}` : key),
    );
  }
  return [];
}

describe("reference tooltips", () => {
  it("matches the PalaceChat wording exactly", () => {
    expect(REFERENCE_TOOLTIPS.brush).toBe("Brush tool (opaque stroke)");
    expect(REFERENCE_TOOLTIPS.eraser).toBe("Eraser tool (clears alpha)");
    expect(REFERENCE_TOOLTIPS.fill).toBe("Fill tool (flood fill)");
    expect(REFERENCE_TOOLTIPS.pick).toBe("Open color picker");
    expect(REFERENCE_TOOLTIPS.clear).toBe("Clear current frame to transparent");
  });
});

describe("string tables", () => {
  const literals = [
    ...leaves(EDITOR, "EDITOR"),
    ...leaves(CANVAS, "CANVAS"),
    ...leaves(PANELS, "PANELS"),
    ...leaves(BAG, "BAG"),
    ...leaves(TOOLTIPS, "TOOLTIPS"),
  ];

  it("has no blank user-facing string", () => {
    const blank = literals.filter(([, value]) => value.trim() === "");
    expect(blank).toEqual([]);
  });

  it("uses one voice: British spelling outside the quoted reference tooltips", () => {
    const american = literals.filter(([, value]) => /\bcolor\b/i.test(value));
    expect(american).toEqual([]);
  });

  it("names every tool panel in tab order", () => {
    expect(PANEL_LABELS).toHaveLength(PANEL_TABS.length);
    expect(PANEL_LABELS[0]).toBe("Paint");
    expect(PANEL_LABELS.at(-1)).toBe("Guides");
  });
});

describe("message formatters", () => {
  it("keeps the prop id in clipboard copy", () => {
    expect(MESSAGES.copiedPropId(7)).toContain("7");
    expect(MESSAGES.clipboardUnavailable(7)).toContain("7");
  });

  it("reports the animated-save block with the frame count", () => {
    const text = MESSAGES.animatedSaveBlocked(4);
    expect(text).toContain("4 frames");
    expect(text).toContain("not supported yet");
  });

  it("reports bag copy outcomes", () => {
    expect(MESSAGES.copiedIntoBag(3)).toContain("3");
    expect(MESSAGES.nothingNewToCopy).toContain("already in My Bag");
  });

  it("formats the worn counter", () => {
    expect(MESSAGES.wornCount(2, 25)).toBe("Worn 2 / 25");
  });
});

describe("withShortcut", () => {
  it("appends the shortcut in parentheses", () => {
    expect(withShortcut("Undo", "Ctrl+Z")).toBe("Undo (Ctrl+Z)");
  });
});
