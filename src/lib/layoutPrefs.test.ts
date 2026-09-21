/**
 * Task 27's pure rules for the Layout memory group.
 *
 * The group is the UI over the layout store, so the rules that can be checked
 * without a window are the ones that decide what the group *claims*: the
 * default for the toggle, and the defensive reading of the state Rust reports.
 * The window half (persist, reset, live detached list) lives in
 * `layoutGroup.dom.test.ts`, and the store half is proved by the Rust tests.
 */
import { describe, expect, it } from "vitest";

import {
  DEFAULT_REMEMBER_LAYOUT,
  LAYOUT_CHANGED_EVENT,
  PANEL_TITLES,
  detachedNames,
  isPanelId,
  layoutMemoryFrom,
} from "./layoutPrefs";
import { PANEL_IDS } from "./panels";

describe("layoutMemoryFrom", () => {
  it("defaults to remembering when the state says nothing", () => {
    expect(layoutMemoryFrom({})).toEqual({
      remember: true,
      detached: [],
      path: null,
    });
    expect(DEFAULT_REMEMBER_LAYOUT).toBe(true);
  });

  it("reads the remember flag, the detached ids and the file path", () => {
    expect(
      layoutMemoryFrom({
        remember: false,
        detached: ["users", "chat"],
        path: "$HOME/.config/org.palace.client/window-layout.json",
      }),
    ).toEqual({
      remember: false,
      detached: ["users", "chat"],
      path: "$HOME/.config/org.palace.client/window-layout.json",
    });
  });

  it("orders the detached list by the panel registry, whatever order arrived", () => {
    expect(layoutMemoryFrom({ detached: ["props", "room", "users"] }).detached).toEqual([
      "room",
      "users",
      "props",
    ]);
  });

  it("drops unknown ids, duplicates and anything that is not a state object", () => {
    expect(layoutMemoryFrom({ detached: ["users", "users", "spike", 7, null] }).detached).toEqual([
      "users",
    ]);
    expect(layoutMemoryFrom(null).detached).toEqual([]);
    expect(layoutMemoryFrom("nonsense").remember).toBe(true);
  });

  it("falls back to the default for a non-boolean remember and an empty path", () => {
    expect(layoutMemoryFrom({ remember: "off" }).remember).toBe(true);
    expect(layoutMemoryFrom({ remember: 0 }).remember).toBe(true);
    expect(layoutMemoryFrom({ path: "" }).path).toBeNull();
    expect(layoutMemoryFrom({ path: 7 }).path).toBeNull();
  });

  it("names every panel the registry knows", () => {
    expect(Object.keys(PANEL_TITLES).sort()).toEqual([...PANEL_IDS].sort());
    expect(detachedNames(["users", "props"])).toEqual(["Users", "Prop bag"]);
  });

  it("names the one event the group follows", () => {
    expect(LAYOUT_CHANGED_EVENT).toBe("palace://layout");
  });
});

describe("isPanelId", () => {
  it("accepts exactly the five panel ids", () => {
    for (const panel of PANEL_IDS) {
      expect(isPanelId(panel)).toBe(true);
    }
    for (const other of ["prefs", "main", "panel-users", "", "spike", 7, null]) {
      expect(isPanelId(other)).toBe(false);
    }
  });
});
