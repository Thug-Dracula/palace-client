/**
 * Task 25's acceptance tests at the suppression point: `store.pushLine`.
 *
 * The store is where every incoming chat line is reduced, so a line dropped
 * there is dropped for the docked panel and every detached one alike. These
 * tests drive the real store with the IPC layer mocked and prove the plan's
 * three claims that belong to the store: an ignored user's messages are
 * suppressed, removing them restores the messages, and the persisted list is
 * re-read after a restart. The fourth claim — that adding a user through the
 * window persists the entry — is exercised through the real component in
 * `muteGroup.dom.test.ts`.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";

const state = vi.hoisted(() => {
  const isRecord = (value: unknown): value is Record<string, unknown> =>
    typeof value === "object" && value !== null && !Array.isArray(value);
  const merge = (
    base: Record<string, unknown>,
    patch: Record<string, unknown>,
  ): Record<string, unknown> => {
    const out: Record<string, unknown> = { ...base };
    for (const [key, value] of Object.entries(patch)) {
      out[key] = isRecord(value) && isRecord(out[key]) ? merge(out[key], value) : value;
    }
    return out;
  };
  return {
    stored: {} as Record<string, unknown>,
    handlers: [] as Array<{
      active: boolean;
      run: (event: { payload: Record<string, unknown> }) => void;
    }>,
    merge,
  };
});

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(
    async (
      _name: string,
      handler: (event: { payload: Record<string, unknown> }) => void,
    ): Promise<() => void> => {
      const entry = { active: true, run: handler };
      state.handlers.push(entry);
      return () => {
        entry.active = false;
      };
    },
  ),
}));

vi.mock("./api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./api")>();
  return {
    ...actual,
    getPrefs: vi.fn(async () => state.stored),
    setPrefs: vi.fn(async (patch: Record<string, unknown>) => {
      state.stored = state.merge(state.stored, patch);
      return state.stored;
    }),
  };
});

import * as api from "./api";
import { addMuteEntry, mutePrefsFrom, removeMuteEntry } from "./mutePrefs";
import { store } from "./store.svelte";
import type { ChatLine, ServerBanner } from "./api";

const banner: ServerBanner = {
  host: "localhost",
  port: 9998,
  byte_order: "LE",
  user_id: 1,
  version: "1.0",
  name: "Lobby Server",
  media_base: null,
  total_users: 2,
};

function line(over: Partial<ChatLine> = {}): ChatLine {
  return { seq: 1, user_id: 4, name: "Ada", text: "hello", kind: "talk", ...over };
}

function storedIdentities(): unknown {
  const mute = state.stored.mute;
  return typeof mute === "object" && mute !== null
    ? (mute as Record<string, unknown>).identities
    : undefined;
}

/** Deliver a `palace://prefs` broadcast to every window still subscribed. */
function broadcast(payload: Record<string, unknown>): void {
  for (const entry of state.handlers) {
    if (entry.active) {
      entry.run({ payload });
    }
  }
}

beforeEach(() => {
  vi.clearAllMocks();
  state.stored = {};
  state.handlers.length = 0;
  store.resetMuteRules();
  store.chat = [];
  store.users = [];
  store.banner = null;
  store.settings = { host: "localhost", port: 9998, username: "Guest", soundfont: null };
});

describe("ignored users are suppressed at the reduction point", () => {
  it("drops a message from an ignored user and keeps everyone else's", () => {
    store.applyMute({
      ignoreAll: false,
      identities: [{ name: "Ada", identity: "ada" }],
    });

    store.pushLine(line({ seq: 1, name: "Ada", text: "from Ada" }));
    store.pushLine(line({ seq: 2, name: "Bob", text: "from Bob" }));

    expect(store.chat.map((entry) => entry.text)).toEqual(["from Bob"]);
  });

  it("matches the name whatever the letter case", () => {
    store.applyMute({ ignoreAll: false, identities: [{ name: "Ada", identity: "ada" }] });
    store.pushLine(line({ seq: 1, name: "aDA" }));
    expect(store.chat).toHaveLength(0);
  });

  it("removing the user restores their messages", () => {
    const rules = { ignoreAll: false, identities: [{ name: "Ada", identity: "ada" }] };
    store.applyMute(rules);
    store.pushLine(line({ seq: 1, name: "Ada", text: "while ignored" }));
    expect(store.chat).toHaveLength(0);

    store.applyMute(removeMuteEntry(rules, "Ada"));
    store.pushLine(line({ seq: 2, name: "Ada", text: "after removal" }));
    expect(store.chat.map((entry) => entry.text)).toEqual(["after removal"]);
  });

  it("ignore-all drops everyone else but never the signed-in user", () => {
    store.banner = banner;
    store.applyMute({ ignoreAll: true, identities: [] });

    store.pushLine(line({ seq: 1, user_id: 9, name: "Ada", text: "from Ada" }));
    store.pushLine(line({ seq: 2, user_id: 1, name: "Me", text: "from me" }));

    expect(store.chat.map((entry) => entry.text)).toEqual(["from me"]);
  });

  it("never hides the server's own notices", () => {
    store.applyMute({ ignoreAll: true, identities: [] });
    store.pushLine(line({ seq: 1, kind: "system", name: "Server", text: "maintenance" }));
    store.pushLine(line({ seq: 2, kind: "error", name: "Server", text: "lost" }));
    expect(store.chat.map((entry) => entry.text)).toEqual(["maintenance", "lost"]);
  });
});

describe("persistence and restart", () => {
  it("writes an added user through the additive settings merge", async () => {
    const added = addMuteEntry(mutePrefsFrom(state.stored), "Bob");
    expect(added.ok).toBe(true);
    if (!added.ok) {
      return;
    }
    await api.setPrefs({ mute: { identities: added.prefs.identities } });
    expect(storedIdentities()).toEqual([{ name: "Bob", identity: "bob" }]);
  });

  it("re-reads the persisted list at session start and suppresses with it", async () => {
    const added = addMuteEntry(mutePrefsFrom(state.stored), "Bob");
    expect(added.ok).toBe(true);
    if (!added.ok) {
      return;
    }
    await api.setPrefs({ mute: { identities: added.prefs.identities } });

    // A restart: the in-memory rules are gone, the file is not.
    store.resetMuteRules();
    store.pushLine(line({ seq: 1, name: "Bob", text: "before the seed" }));
    expect(store.chat).toHaveLength(1);

    store.startMuteRules();
    await vi.waitFor(() => expect(store.mute.identities).toHaveLength(1));

    store.pushLine(line({ seq: 2, name: "Bob", text: "after the seed" }));
    store.pushLine(line({ seq: 3, name: "Ada", text: "still here" }));
    expect(store.chat.map((entry) => entry.text)).toEqual(["before the seed", "still here"]);
  });

  it("seeds itself when the session starts, without opening Preferences", async () => {
    state.stored = { mute: { identities: [{ name: "Bob", identity: "bob" }] } };

    store.apply({ type: "status", status: "connected", message: null });
    await vi.waitFor(() => expect(store.mute.identities).toHaveLength(1));

    store.pushLine(line({ seq: 1, name: "Bob" }));
    expect(store.chat).toHaveLength(0);
  });

  it("follows the broadcast a Preferences change sends to every window", async () => {
    store.startMuteRules();
    await vi.waitFor(() => expect(state.handlers).toHaveLength(1));

    store.pushLine(line({ seq: 1, name: "Bob", text: "before" }));
    expect(store.chat).toHaveLength(1);

    broadcast({ mute: { identities: [{ name: "Bob", identity: "bob" }] } });

    store.pushLine(line({ seq: 2, name: "Bob", text: "after" }));
    expect(store.chat.map((entry) => entry.text)).toEqual(["before"]);
  });

  it("stops following the broadcast once the rules are reset", async () => {
    store.startMuteRules();
    await vi.waitFor(() => expect(state.handlers).toHaveLength(1));

    store.resetMuteRules();
    broadcast({ mute: { ignore_all: true } });

    store.pushLine(line({ seq: 1, name: "Ada", text: "still visible" }));
    expect(store.chat.map((entry) => entry.text)).toEqual(["still visible"]);
  });
});
