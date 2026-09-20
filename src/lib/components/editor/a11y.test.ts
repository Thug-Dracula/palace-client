import { describe, expect, it } from "vitest";

import {
  FOCUSABLE_SELECTOR,
  editorCommand,
  nextFocusIndex,
  type ShortcutEvent,
} from "./a11y";

describe("nextFocusIndex", () => {
  it("starts at the first control on forward Tab from the dialog", () => {
    expect(nextFocusIndex(4, -1, false)).toBe(0);
  });

  it("starts at the last control on backward Tab from the dialog", () => {
    expect(nextFocusIndex(4, -1, true)).toBe(3);
  });

  it("wraps forward from the last control to the first", () => {
    expect(nextFocusIndex(4, 3, false)).toBe(0);
  });

  it("wraps backward from the first control to the last", () => {
    expect(nextFocusIndex(4, 0, true)).toBe(3);
  });

  it("walks the list one step at a time", () => {
    expect(nextFocusIndex(3, 0, false)).toBe(1);
    expect(nextFocusIndex(3, 1, true)).toBe(0);
  });

  it("reports nothing to focus for an empty dialog", () => {
    expect(nextFocusIndex(0, -1, false)).toBe(-1);
  });
});

describe("FOCUSABLE_SELECTOR", () => {
  it("excludes disabled controls and tabindex=-1 elements", () => {
    expect(FOCUSABLE_SELECTOR).toContain(":not([disabled])");
    expect(FOCUSABLE_SELECTOR).toContain('not([tabindex="-1"])');
  });
});

const press = (over: Partial<ShortcutEvent>): ShortcutEvent => ({
  key: "",
  ctrlKey: false,
  metaKey: false,
  shiftKey: false,
  altKey: false,
  ...over,
});

describe("editorCommand", () => {
  it("maps Ctrl+Z / Ctrl+Shift+Z / Ctrl+Y", () => {
    expect(editorCommand(press({ key: "z", ctrlKey: true }))).toEqual({ kind: "undo" });
    expect(editorCommand(press({ key: "z", ctrlKey: true, shiftKey: true }))).toEqual({
      kind: "redo",
    });
    expect(editorCommand(press({ key: "y", ctrlKey: true }))).toEqual({ kind: "redo" });
  });

  it("accepts the Command key as the primary modifier", () => {
    expect(editorCommand(press({ key: "z", metaKey: true }))).toEqual({ kind: "undo" });
  });

  it("maps zoom and view shortcuts", () => {
    expect(editorCommand(press({ key: "=", ctrlKey: true }))).toEqual({ kind: "zoom-in" });
    expect(editorCommand(press({ key: "+", ctrlKey: true }))).toEqual({ kind: "zoom-in" });
    expect(editorCommand(press({ key: "-", ctrlKey: true }))).toEqual({ kind: "zoom-out" });
    expect(editorCommand(press({ key: "0", ctrlKey: true }))).toEqual({ kind: "fit" });
    expect(editorCommand(press({ key: "1", ctrlKey: true }))).toEqual({ kind: "actual-size" });
  });

  it("maps Ctrl+S to save", () => {
    expect(editorCommand(press({ key: "s", ctrlKey: true }))).toEqual({ kind: "save" });
  });

  it("cycles tool panels with Ctrl+Tab and Alt+Arrow", () => {
    expect(editorCommand(press({ key: "Tab", ctrlKey: true }))).toEqual({
      kind: "panel-cycle",
      delta: 1,
    });
    expect(editorCommand(press({ key: "Tab", ctrlKey: true, shiftKey: true }))).toEqual({
      kind: "panel-cycle",
      delta: -1,
    });
    expect(editorCommand(press({ key: "ArrowRight", altKey: true }))).toEqual({
      kind: "panel-cycle",
      delta: 1,
    });
    expect(editorCommand(press({ key: "ArrowLeft", altKey: true }))).toEqual({
      kind: "panel-cycle",
      delta: -1,
    });
  });

  it("selects a tool panel directly with Alt+number", () => {
    expect(editorCommand(press({ key: "1", altKey: true }))).toEqual({
      kind: "panel-select",
      id: "paint",
    });
    expect(editorCommand(press({ key: "7", altKey: true }))).toEqual({
      kind: "panel-select",
      id: "guides",
    });
    expect(editorCommand(press({ key: "8", altKey: true }))).toBeNull();
  });

  it("never claims a plain key press", () => {
    expect(editorCommand(press({ key: "z" }))).toBeNull();
    expect(editorCommand(press({ key: "s" }))).toBeNull();
    expect(editorCommand(press({ key: "a", ctrlKey: true }))).toBeNull();
  });
});
