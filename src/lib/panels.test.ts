/**
 * Route/view resolution for the panel-host split.
 *
 * The Rust registry (`src-tauri/src/windows.rs`) builds
 * `index.html#/panel/<id>` for every detached window, and its enum is the
 * authority on the five ids. These tests lock the frontend half of that
 * contract: which id a hash names, and whether a window showing it is the main
 * shell or a single detached panel.
 */
import { describe, expect, it } from "vitest";

import { PANEL_IDS, panelFromId, viewForHash } from "./panels";

describe("panel ids", () => {
  it("are exactly the five panels the Rust registry exposes", () => {
    expect(PANEL_IDS).toEqual(["room", "users", "rooms", "chat", "props"]);
  });

  it("accepts the window-label form and is lenient about case and whitespace", () => {
    expect(panelFromId("users")).toBe("users");
    expect(panelFromId(" Users ")).toBe("users");
    expect(panelFromId("PANEL-USERS")).toBe("users");
    expect(panelFromId("panel-props")).toBe("props");
    expect(panelFromId("propbag")).toBe("props");
    expect(panelFromId("prop-bag")).toBe("props");
  });

  it("rejects anything outside the enum, including the prop editor", () => {
    for (const input of ["", " ", "main", "panel-", "editor", "panel-editor", "props2"]) {
      expect(panelFromId(input)).toBeNull();
    }
  });
});

describe("viewForHash", () => {
  it("treats an empty or root hash as the main window", () => {
    expect(viewForHash("")).toEqual({ kind: "main" });
    expect(viewForHash("#")).toEqual({ kind: "main" });
    expect(viewForHash("#/")).toEqual({ kind: "main" });
  });

  it("mounts the named panel for a #/panel/<id> hash", () => {
    for (const id of PANEL_IDS) {
      expect(viewForHash(`#/panel/${id}`)).toEqual({ kind: "panel", panel: id });
    }
  });

  it("is lenient in the same ways the Rust id parser is", () => {
    expect(viewForHash("#/panel/Users")).toEqual({ kind: "panel", panel: "users" });
    expect(viewForHash("#/panel/panel-chat")).toEqual({ kind: "panel", panel: "chat" });
  });

  it("falls back to the full layout for an unknown panel route", () => {
    expect(viewForHash("#/panel/editor")).toEqual({ kind: "main" });
    expect(viewForHash("#/panel/")).toEqual({ kind: "main" });
    expect(viewForHash("#/panel")).toEqual({ kind: "main" });
    expect(viewForHash("#/something-else")).toEqual({ kind: "main" });
  });
});
