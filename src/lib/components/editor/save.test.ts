import { describe, expect, it } from "vitest";

import { defaultSaveForm, isAnimatedSaveBlocked, toSaveOptions } from "./save";

describe("defaultSaveForm", () => {
  it("appends a new prop with no explicit name or id", () => {
    const form = defaultSaveForm();
    expect(form.placement).toBe("new_prop_at_end");
    expect(form.name).toBe("");
    expect(form.propIdText).toBe("");
  });
});

describe("toSaveOptions", () => {
  it("sends null for a blank name and id so Rust allocates them", () => {
    const options = toSaveOptions({ ...defaultSaveForm(), name: "   ", propIdText: "" });
    expect(options.name).toBeNull();
    expect(options.propId).toBeNull();
  });

  it("trims a real name and parses a numeric id", () => {
    const options = toSaveOptions({ ...defaultSaveForm(), name: "  Dot  ", propIdText: "42" });
    expect(options.name).toBe("Dot");
    expect(options.propId).toBe(42);
  });

  it("treats an unparseable id as blank instead of NaN", () => {
    const options = toSaveOptions({ ...defaultSaveForm(), propIdText: "abc" });
    expect(options.propId).toBeNull();
  });

  it("carries the flags and offsets through unchanged", () => {
    const options = toSaveOptions({
      ...defaultSaveForm(),
      placement: "overwrite_in_place",
      head: true,
      ghost: true,
      rare: true,
      bounce: true,
      animate: true,
      hOffset: -3,
      vOffset: 5,
    });
    expect(options).toMatchObject({
      placement: "overwrite_in_place",
      head: true,
      ghost: true,
      rare: true,
      bounce: true,
      animate: true,
      hOffset: -3,
      vOffset: 5,
    });
  });
});

describe("isAnimatedSaveBlocked", () => {
  it("blocks 2+ frames and allows a single frame", () => {
    expect(isAnimatedSaveBlocked(1)).toBe(false);
    expect(isAnimatedSaveBlocked(2)).toBe(true);
  });
});
