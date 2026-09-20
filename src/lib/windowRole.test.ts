/**
 * `isPanelWindow` decides where the detach control may appear: the main shell
 * only. The panel window's route is `#/panel/<id>`; anything else (including an
 * unknown panel) is the shell, matching `viewForHash`.
 */
import { describe, expect, it } from "vitest";

import { isPanelWindow } from "./windowRole";

describe("isPanelWindow", () => {
  it("is false for the main shell", () => {
    expect(isPanelWindow("")).toBe(false);
    expect(isPanelWindow("#/")).toBe(false);
    expect(isPanelWindow("#/something")).toBe(false);
  });

  it("is true for every panel route the Rust registry builds", () => {
    for (const id of ["room", "users", "rooms", "chat", "props"]) {
      expect(isPanelWindow(`#/panel/${id}`), id).toBe(true);
    }
  });

  it("falls back to the shell for an unknown panel hash", () => {
    expect(isPanelWindow("#/panel/editor")).toBe(false);
  });
});
