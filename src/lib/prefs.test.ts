/**
 * The Preferences keyspace rules, independent of the window.
 *
 * `PREFERENCES.md` (Task 5) is the authority for the ten group names and their
 * order, and `src-tauri/src/prefs.rs` mirrors this registry. These tests pin
 * the frontend half so a shell that lists eleven groups, or the wrong names,
 * fails loudly.
 */
import { describe, expect, it } from "vitest";

import {
  FIRST_GROUP_ID,
  PREF_GROUPS,
  isPrefGroupId,
  lastGroupFrom,
  prefGroupById,
  readPref,
} from "./prefs";

describe("the preference group registry", () => {
  it("is exactly the ten groups from the specification, in order", () => {
    expect(PREF_GROUPS.map((group) => group.id)).toEqual([
      "connection",
      "appearance",
      "graphics",
      "sound",
      "chat_log",
      "avatar",
      "mute",
      "notifications",
      "layout",
      "shell",
    ]);
    expect(PREF_GROUPS.map((group) => group.label)).toEqual([
      "Connection & identity",
      "Appearance",
      "Room & graphics behaviour",
      "Sound",
      "Chat logging",
      "Avatar & prop behaviour",
      "Mute/ignore",
      "Notifications",
      "Layout memory",
      "Preferences shell",
    ]);
    expect(PREF_GROUPS).toHaveLength(10);
  });

  it("has a unique id and a non-empty summary for every group", () => {
    const ids = PREF_GROUPS.map((group) => group.id);
    expect(new Set(ids).size).toBe(ids.length);
    for (const group of PREF_GROUPS) {
      expect(group.summary.length, group.id).toBeGreaterThan(0);
      expect(prefGroupById(group.id)).toBe(group);
    }
    expect(prefGroupById("editor")).toBeNull();
  });

  it("starts on Connection & identity", () => {
    expect(FIRST_GROUP_ID).toBe("connection");
    expect(PREF_GROUPS[0].id).toBe(FIRST_GROUP_ID);
  });
});

describe("isPrefGroupId", () => {
  it("accepts only the ten real ids", () => {
    for (const group of PREF_GROUPS) {
      expect(isPrefGroupId(group.id), group.id).toBe(true);
    }
    for (const input of ["", " ", "prefs", "Connection", "shell ", "editor", 7, null, {}]) {
      expect(isPrefGroupId(input), String(input)).toBe(false);
    }
  });
});

describe("lastGroupFrom", () => {
  it("returns the remembered group when it is a real one", () => {
    expect(lastGroupFrom({ shell: { last_group: "sound" } })).toBe("sound");
    expect(lastGroupFrom({ shell: { last_group: "layout" } })).toBe("layout");
  });

  it("falls back to the first group when the value is missing or stale", () => {
    expect(lastGroupFrom({})).toBe(FIRST_GROUP_ID);
    expect(lastGroupFrom({ shell: {} })).toBe(FIRST_GROUP_ID);
    expect(lastGroupFrom({ shell: { last_group: "gone" } })).toBe(FIRST_GROUP_ID);
    expect(lastGroupFrom({ shell: { last_group: 12 } })).toBe(FIRST_GROUP_ID);
    expect(lastGroupFrom({ shell: "not-an-object" })).toBe(FIRST_GROUP_ID);
  });
});

describe("readPref", () => {
  it("walks a nested path and returns the fallback when any step is absent", () => {
    const values = { appearance: { font_size_px: 14 }, sound: { enabled: false } };
    expect(readPref(values, ["appearance", "font_size_px"], 13)).toBe(14);
    expect(readPref(values, ["appearance", "theme"], "crt-dark")).toBe("crt-dark");
    expect(readPref(values, ["sound", "enabled"], true)).toBe(false);
    expect(readPref(values, ["sound", "volume"], 1)).toBe(1);
    expect(readPref(values, ["missing", "deep", "path"], "fallback")).toBe("fallback");
  });
});
