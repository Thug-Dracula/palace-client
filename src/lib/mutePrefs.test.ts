/**
 * Task 25's pure rules for the Mute/ignore group.
 *
 * The defaults, the key names, the add/remove rules and the suppression
 * predicate are all independent of any window, so they are locked here; the
 * DOM half (persist + apply live) lives in `muteGroup.dom.test.ts` and the
 * store's reduction point in `muteMessages.test.ts`. The key names are asserted
 * against the literal strings from `PREFERENCES.md`, so a rename cannot drift
 * away from the shared settings file unnoticed.
 */
import { describe, expect, it } from "vitest";

import {
  DEFAULT_IGNORE_ALL,
  MUTE_CAPABILITIES,
  MUTE_IDENTITIES_KEY,
  MUTE_IGNORE_ALL_KEY,
  PREFS_CHANGED_EVENT,
  addMuteEntry,
  isMuteCapabilitySupported,
  isMutedUser,
  muteCapability,
  muteEntryFrom,
  mutePrefsFrom,
  normaliseMuteName,
  removeMuteEntry,
  suppressesMessage,
  type MutePrefs,
} from "./mutePrefs";

const empty: MutePrefs = { ignoreAll: false, identities: [] };

describe("mutePrefsFrom", () => {
  it("returns the specification's defaults for an empty document", () => {
    expect(mutePrefsFrom({})).toEqual({ ignoreAll: false, identities: [] });
    expect(DEFAULT_IGNORE_ALL).toBe(false);
  });

  it("names the keys from PREFERENCES.md", () => {
    expect(MUTE_IGNORE_ALL_KEY).toBe("ignore_all");
    expect(MUTE_IDENTITIES_KEY).toBe("identities");
    expect(PREFS_CHANGED_EVENT).toBe("palace://prefs");
  });

  it("reads the stored block by its PREFERENCES.md names", () => {
    const values = {
      mute: {
        ignore_all: true,
        identities: [{ name: "Ada", identity: "ada" }],
      },
    };
    expect(mutePrefsFrom(values)).toEqual({
      ignoreAll: true,
      identities: [{ name: "Ada", identity: "ada" }],
    });
  });

  it("trims names, defaults a missing identity and drops unusable entries", () => {
    const values = {
      mute: {
        identities: [
          { name: "  Ada  ", identity: " ada " },
          { name: "Bob" },
          { name: "" },
          { name: "   " },
          { name: 7 },
          "Ada",
          null,
          [1, 2],
        ],
      },
    };
    expect(mutePrefsFrom(values).identities).toEqual([
      { name: "Ada", identity: "ada" },
      { name: "Bob", identity: "" },
    ]);
  });

  it("falls back per key when a stored value has the wrong type", () => {
    expect(mutePrefsFrom({ mute: { ignore_all: "yes", identities: "Ada" } })).toEqual(empty);
    expect(mutePrefsFrom({ mute: "nope" })).toEqual(empty);
  });

  it("keeps the first of two entries with the same name", () => {
    const values = {
      mute: {
        identities: [
          { name: "Ada", identity: "first" },
          { name: "ada", identity: "second" },
        ],
      },
    };
    expect(mutePrefsFrom(values).identities).toEqual([{ name: "Ada", identity: "first" }]);
  });
});

describe("muteEntryFrom", () => {
  it("accepts a usable record and rejects everything else", () => {
    expect(muteEntryFrom({ name: "Ada", identity: "ada" })).toEqual({
      name: "Ada",
      identity: "ada",
    });
    expect(muteEntryFrom({ name: "Ada" })).toEqual({ name: "Ada", identity: "" });
    for (const value of [null, 7, "Ada", [], {}, { name: "" }, { name: " " }]) {
      expect(muteEntryFrom(value), JSON.stringify(value)).toBeNull();
    }
  });
});

describe("addMuteEntry", () => {
  it("rejects an empty or whitespace-only name", () => {
    for (const name of ["", "   ", "\t\n"]) {
      const result = addMuteEntry(empty, name);
      expect(result.ok).toBe(false);
      if (!result.ok) {
        expect(result.error).toContain("user name");
      }
    }
  });

  it("adds a trimmed name and mints the folded name as the identity", () => {
    const result = addMuteEntry(empty, "  Ada Lovelace ");
    expect(result.ok).toBe(true);
    if (result.ok) {
      expect(result.prefs.identities).toEqual([
        { name: "Ada Lovelace", identity: "ada lovelace" },
      ]);
    }
  });

  it("keeps an explicit identity", () => {
    const result = addMuteEntry(empty, "Ada", "ada-42");
    expect(result.ok).toBe(true);
    if (result.ok) {
      expect(result.prefs.identities).toEqual([{ name: "Ada", identity: "ada-42" }]);
    }
  });

  it("refuses a duplicate, whatever the letter case", () => {
    const once = addMuteEntry(empty, "Ada");
    expect(once.ok).toBe(true);
    if (!once.ok) {
      return;
    }
    const twice = addMuteEntry(once.prefs, "ADA");
    expect(twice.ok).toBe(false);
    if (!twice.ok) {
      expect(twice.error).toContain("already");
    }
    expect(once.prefs.identities).toHaveLength(1);
  });

  it("does not mutate the rules it was given", () => {
    const before = { ...empty, identities: [...empty.identities] };
    addMuteEntry(empty, "Ada");
    expect(empty).toEqual(before);
  });
});

describe("removeMuteEntry", () => {
  const rules: MutePrefs = {
    ignoreAll: false,
    identities: [
      { name: "Ada", identity: "ada" },
      { name: "Bob", identity: "bob" },
    ],
  };

  it("removes the matching entry, case-insensitively", () => {
    expect(removeMuteEntry(rules, "aDa").identities).toEqual([{ name: "Bob", identity: "bob" }]);
  });

  it("leaves the list alone when the name is not on it", () => {
    expect(removeMuteEntry(rules, "Cara").identities).toEqual(rules.identities);
  });

  it("does not mutate the rules it was given", () => {
    removeMuteEntry(rules, "Ada");
    expect(rules.identities).toHaveLength(2);
  });
});

describe("isMutedUser", () => {
  const rules: MutePrefs = {
    ignoreAll: false,
    identities: [{ name: "Ada Lovelace", identity: "ada-42" }],
  };

  it("matches the name or the identity, ignoring case", () => {
    expect(isMutedUser(rules, "Ada Lovelace")).toBe(true);
    expect(isMutedUser(rules, "ada lovelace")).toBe(true);
    expect(isMutedUser(rules, "ADA-42")).toBe(true);
    expect(isMutedUser(rules, "Ada")).toBe(false);
    expect(isMutedUser(rules, "")).toBe(false);
  });

  it("ignores everyone but the caller when ignore-all is on", () => {
    expect(isMutedUser({ ignoreAll: true, identities: [] }, "Anyone")).toBe(true);
  });

  it("ignores nobody with the shipped defaults", () => {
    expect(isMutedUser(empty, "Anyone")).toBe(false);
  });

  it("normalises names the same way everywhere", () => {
    expect(normaliseMuteName("  Ada Lovelace  ")).toBe("ada lovelace");
  });
});

describe("suppressesMessage", () => {
  const rules: MutePrefs = {
    ignoreAll: false,
    identities: [{ name: "Ada", identity: "ada" }],
  };

  it("suppresses a talk line from an ignored user", () => {
    expect(suppressesMessage(rules, { name: "Ada", kind: "talk", user_id: 4 })).toBe(true);
    expect(suppressesMessage(rules, { name: "Bob", kind: "talk", user_id: 5 })).toBe(false);
  });

  it("never suppresses the server's own notices", () => {
    const all: MutePrefs = { ignoreAll: true, identities: [] };
    expect(suppressesMessage(all, { name: "Server", kind: "system" })).toBe(false);
    expect(suppressesMessage(all, { name: "Server", kind: "error" })).toBe(false);
    expect(suppressesMessage(all, { name: "Ada", kind: "talk" })).toBe(true);
  });

  it("never hides the signed-in user from themselves", () => {
    const all: MutePrefs = { ignoreAll: true, identities: [] };
    expect(suppressesMessage(all, { name: "Me", kind: "talk", user_id: 1 }, { user_id: 1 })).toBe(
      false,
    );
    expect(suppressesMessage(all, { name: "Me", kind: "talk" }, { name: "me" })).toBe(false);
    expect(suppressesMessage(all, { name: "Ada", kind: "talk", user_id: 4 }, { user_id: 1 })).toBe(
      true,
    );
  });
});

describe("the capability table", () => {
  it("marks message hiding and name matching as the backed capabilities", () => {
    expect(isMuteCapabilitySupported("message_hiding")).toBe(true);
    expect(isMuteCapabilitySupported("name_matching")).toBe(true);
  });

  it("marks the unbacked capabilities and explains why", () => {
    for (const key of ["account_identity", "server_mute", "motion"]) {
      const capability = muteCapability(key);
      expect(capability?.support).toBe("unsupported");
      expect(capability?.reason?.trim().length ?? 0).toBeGreaterThan(0);
      expect(isMuteCapabilitySupported(key)).toBe(false);
    }
  });

  it("says plainly that no server mute command is sent", () => {
    const server = muteCapability("server_mute");
    expect(server?.reason ?? "").toContain("No mute command is sent to the server");
  });

  it("treats an unknown capability as unsupported rather than assuming it works", () => {
    expect(muteCapability("telepathy")).toBeNull();
    expect(isMuteCapabilitySupported("telepathy")).toBe(false);
  });

  it("lists every capability once, in display order", () => {
    const keys = MUTE_CAPABILITIES.map((capability) => capability.key);
    expect(keys).toEqual([
      "message_hiding",
      "name_matching",
      "account_identity",
      "server_mute",
      "motion",
    ]);
    expect(new Set(keys).size).toBe(keys.length);
  });
});
