// @vitest-environment jsdom
/**
 * Task 25's acceptance tests through the real window: the Mute/ignore group.
 *
 * The plan asks for three things and each has a test that can fail:
 *
 * * adding a user persists them AND their messages are suppressed afterwards;
 * * removing a user restores their messages;
 * * an empty or whitespace-only entry is refused, and the window says the
 *   classic client had no such list.
 *
 * The shell routes `mute` to the real component, so one test mounts the whole
 * PreferencesWindow and clicks the navigation item, proving the group is no
 * longer the placeholder.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

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
    merge,
    invoke: vi.fn(async () => true),
  };
});

vi.mock("@tauri-apps/api/core", () => ({ invoke: state.invoke }));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => {}),
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
    resetPrefs: vi.fn(async () => {
      state.stored = {};
      return state.stored;
    }),
    getSettings: vi.fn(async () => ({
      host: "localhost",
      port: 9998,
      username: "Guest",
      soundfont: null,
    })),
    openPreferences: vi.fn(async () => "created"),
    closePreferences: vi.fn(async () => "closed"),
    openPanel: vi.fn(async () => "created"),
    closePanel: vi.fn(async () => "closed"),
    onEvent: vi.fn(async () => () => {}),
    refresh: vi.fn(async () => {}),
    setViewport: vi.fn(async () => {}),
    setVisibility: vi.fn(async () => {}),
  };
});

import { mount, tick, unmount } from "svelte";

import * as api from "./api";
import type { ChatLine, ServerBanner } from "./api";
import PreferencesWindow from "./components/prefs/PreferencesWindow.svelte";
import MuteGroup from "./components/prefs/MuteGroup.svelte";
import { prefs } from "./prefsStore.svelte";
import { store } from "./store.svelte";

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

let mounted: ReturnType<typeof mount> | undefined;

function byTestId<T extends HTMLElement>(id: string): T {
  const node = document.querySelector<T>(`[data-testid="${id}"]`);
  if (!node) {
    throw new Error(`no element with data-testid ${id}`);
  }
  return node;
}

function storedMute(): Record<string, unknown> {
  return (state.stored.mute ?? {}) as Record<string, unknown>;
}

function line(over: Partial<ChatLine> = {}): ChatLine {
  return { seq: 1, user_id: 4, name: "Ada", text: "hello", kind: "talk", ...over };
}

async function renderGroup(): Promise<void> {
  await prefs.load();
  mounted = mount(MuteGroup, { target: document.body });
  await tick();
}

async function typeDraft(value: string): Promise<void> {
  const input = byTestId<HTMLInputElement>("mute-add-input");
  input.value = value;
  input.dispatchEvent(new Event("input", { bubbles: true }));
  await tick();
  expect(input.value).toBe(value);
}

async function submitAdd(): Promise<void> {
  const form = byTestId("mute-add").closest("form");
  if (!form) {
    throw new Error("the add control is not inside a form");
  }
  form.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
  await tick();
}

/** Wait until a save has finished, so the next click is not on a disabled control. */
async function settle(): Promise<void> {
  await vi.waitFor(() => expect(byTestId<HTMLButtonElement>("mute-add").disabled).toBe(false));
  await tick();
}

beforeEach(() => {
  vi.clearAllMocks();
  state.stored = {};
  prefs.reset();
  store.resetMuteRules();
  store.chat = [];
  store.users = [];
  store.banner = null;
  document.body.innerHTML = "";
});

afterEach(() => {
  if (mounted) {
    unmount(mounted);
    mounted = undefined;
  }
  document.body.innerHTML = "";
});

describe("the shell routes the group", () => {
  it("renders the real group when Mute/ignore is chosen", async () => {
    mounted = mount(PreferencesWindow, { target: document.body });
    await tick();
    await vi.waitFor(() =>
      expect(document.querySelectorAll(".prefs-nav-item").length).toBeGreaterThan(0),
    );

    const nav = [...document.querySelectorAll<HTMLButtonElement>(".prefs-nav-item")].find(
      (node) => node.textContent?.trim() === "Mute/ignore",
    );
    expect(nav).toBeDefined();
    nav?.click();
    await tick();

    expect(byTestId("mute-group")).toBeTruthy();
    expect(document.querySelector(".prefs-placeholder")).toBeNull();
    expect(document.querySelector(".prefs-content h1")?.textContent).toBe("Mute/ignore");
  });

  it("says the classic client had no persistent list", async () => {
    await renderGroup();
    const note = byTestId("mute-classic-note").textContent ?? "";
    expect(note).toContain("classic PalaceChat client had no ignore list");
    expect(note).toContain("enhancement");
  });

  it("says no server mute command is sent, and marks it not supported", async () => {
    await renderGroup();
    expect(byTestId("mute-local-note").textContent).toContain("never asks the server to mute");
    const chip = byTestId("mute-unsupported-server_mute");
    expect(chip.textContent?.trim()).toBe("Not supported");
    const row = chip.closest(".prefs-option");
    expect(row?.querySelector(".prefs-note")?.textContent).toContain(
      "No mute command is sent to the server",
    );
  });
});

describe("adding and removing a user", () => {
  it("persists an added user and suppresses their messages afterwards", async () => {
    await renderGroup();

    await typeDraft("Bob");
    await submitAdd();
    await settle();

    await vi.waitFor(() => expect(storedMute().identities).toEqual([{ name: "Bob", identity: "bob" }]));
    expect(byTestId("mute-entry-bob").textContent).toContain("Bob");

    store.pushLine(line({ seq: 1, name: "Bob", text: "from Bob" }));
    store.pushLine(line({ seq: 2, name: "Ada", text: "from Ada" }));
    expect(store.chat.map((entry) => entry.text)).toEqual(["from Ada"]);
  });

  it("removing a user restores their messages", async () => {
    await renderGroup();
    await typeDraft("Bob");
    await submitAdd();
    await settle();
    await vi.waitFor(() => expect(storedMute().identities).toHaveLength(1));

    byTestId<HTMLButtonElement>("mute-remove-bob").click();
    await settle();

    await vi.waitFor(() => expect(storedMute().identities).toEqual([]));
    store.pushLine(line({ seq: 1, name: "Bob", text: "back again" }));
    expect(store.chat.map((entry) => entry.text)).toEqual(["back again"]);
  });

  it("trims the typed name before storing it", async () => {
    await renderGroup();
    await typeDraft("  Bob  ");
    await submitAdd();
    await settle();
    await vi.waitFor(() =>
      expect(storedMute().identities).toEqual([{ name: "Bob", identity: "bob" }]),
    );
  });

  it("shows a stored list again after a remount", async () => {
    state.stored = { mute: { identities: [{ name: "Bob", identity: "bob" }] } };
    await renderGroup();

    expect(byTestId("mute-entry-bob")).toBeTruthy();
    expect(store.mute.identities).toEqual([{ name: "Bob", identity: "bob" }]);
  });
});

describe("bad input is refused", () => {
  it("rejects an empty entry without writing anything", async () => {
    await renderGroup();
    await submitAdd();

    expect(byTestId("mute-error").textContent).toContain("Type a user name");
    expect(vi.mocked(api.setPrefs)).not.toHaveBeenCalled();
    expect(storedMute()).toEqual({});
  });

  it("rejects a whitespace-only entry without writing anything", async () => {
    await renderGroup();
    await typeDraft("   ");
    await submitAdd();

    expect(byTestId("mute-error").textContent).toContain("Type a user name");
    expect(vi.mocked(api.setPrefs)).not.toHaveBeenCalled();
  });

  it("refuses a duplicate instead of storing it twice", async () => {
    await renderGroup();
    await typeDraft("Bob");
    await submitAdd();
    await settle();
    await vi.waitFor(() => expect(storedMute().identities).toHaveLength(1));

    await typeDraft("bob");
    await submitAdd();

    expect(byTestId("mute-error").textContent).toContain("already");
    expect(storedMute().identities).toEqual([{ name: "Bob", identity: "bob" }]);
  });
});

describe("ignore all", () => {
  it("persists the toggle and hides everyone but the signed-in user", async () => {
    await renderGroup();
    store.banner = banner;

    byTestId<HTMLInputElement>("mute-ignore-all").click();
    await settle();
    await vi.waitFor(() => expect(storedMute().ignore_all).toBe(true));
    await vi.waitFor(() => expect(store.mute.ignoreAll).toBe(true));

    store.pushLine(line({ seq: 1, user_id: 9, name: "Ada", text: "from Ada" }));
    store.pushLine(line({ seq: 2, user_id: 1, name: "Me", text: "from me" }));
    expect(store.chat.map((entry) => entry.text)).toEqual(["from me"]);
  });

  it("shows the stored toggle after a remount", async () => {
    state.stored = { mute: { ignore_all: true, identities: [] } };
    await renderGroup();
    expect(byTestId<HTMLInputElement>("mute-ignore-all").checked).toBe(true);
    expect(store.mute.ignoreAll).toBe(true);
  });
});
