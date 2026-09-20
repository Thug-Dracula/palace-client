// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { focusFirst, focusables, trapTabKey, visibleFocusables } from "./a11y";

/* jsdom does not lay elements out, so `getClientRects()` is empty for every
   node; the trap treats that as "not visible". Report a fake rect instead so
   the visibility filter behaves as it does in a real WebView. */
function show(element: HTMLElement): void {
  Object.defineProperty(element, "getClientRects", {
    configurable: true,
    value: () => [{ width: 10, height: 10 }],
  });
}

function tab(shiftKey = false): KeyboardEvent {
  return new KeyboardEvent("keydown", { key: "Tab", shiftKey, cancelable: true });
}

describe("focus trap", () => {
  let root: HTMLDivElement;
  let first: HTMLButtonElement;
  let second: HTMLButtonElement;
  let third: HTMLButtonElement;

  beforeEach(() => {
    root = document.createElement("div");
    first = document.createElement("button");
    second = document.createElement("button");
    third = document.createElement("button");
    const disabled = document.createElement("button");
    disabled.disabled = true;
    root.append(first, second, disabled, third);
    document.body.append(root);
    show(first);
    show(second);
    show(third);
    show(disabled);
  });

  afterEach(() => {
    root.remove();
  });

  it("lists only enabled controls", () => {
    expect(focusables(root)).toHaveLength(3);
    expect(visibleFocusables(root)).toHaveLength(3);
  });

  it("moves forward and wraps from the last control back to the first", () => {
    first.focus();
    expect(trapTabKey(tab(), root)).toBe(true);
    expect(document.activeElement).toBe(second);
    expect(trapTabKey(tab(), root)).toBe(true);
    expect(document.activeElement).toBe(third);
    expect(trapTabKey(tab(), root)).toBe(true);
    expect(document.activeElement).toBe(first);
  });

  it("moves backward and wraps from the first control to the last", () => {
    first.focus();
    expect(trapTabKey(tab(true), root)).toBe(true);
    expect(document.activeElement).toBe(third);
    expect(trapTabKey(tab(true), root)).toBe(true);
    expect(document.activeElement).toBe(second);
  });

  it("enters the trap from the dialog itself", () => {
    root.tabIndex = -1;
    root.focus();
    expect(trapTabKey(tab(), root)).toBe(true);
    expect(document.activeElement).toBe(first);
  });

  it("ignores non-Tab keys", () => {
    first.focus();
    const escape = new KeyboardEvent("keydown", { key: "Escape", cancelable: true });
    expect(trapTabKey(escape, root)).toBe(false);
    expect(document.activeElement).toBe(first);
  });

  it("focusFirst lands on the first enabled control", () => {
    document.body.focus();
    focusFirst(root);
    expect(document.activeElement).toBe(first);
  });
});
