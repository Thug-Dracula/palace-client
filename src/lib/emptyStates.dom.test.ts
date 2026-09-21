// @vitest-environment jsdom
/**
 * Task 29 — every panel's empty / disconnected state, locked by a test.
 *
 * The five detachable panels and the Preferences window must never open as a
 * blank, unlabelled rectangle. Each panel already renders a named empty state;
 * this file is the acceptance evidence for that claim, one assertion per panel:
 *
 * * `panel-users`   — "Not connected." / "Nobody here."
 *                     (also covered by `components/userList.dom.test.ts`)
 * * `panel-rooms`   — "Not connected." / "No room list yet." / "No rooms match"
 *                     (the first two are cross-checked here; the no-match case
 *                     also has a test in `roomsPanel.dom.test.ts`)
 * * `panel-chat`    — "Chat appears here once connected." / "Say hello."
 * * `panel-props`   — "No props in your bag yet." / "Nothing matches this view."
 * * `panel-room`    — "Offline" / "Waiting for a room"
 * * the Preferences window — an empty "last servers" list and an empty
 *   ignore list each say so.
 *
 * Panels are mounted through the real `PanelWindow` (the same component a real
 * `panel-*` OS window boots) with only the `api` module mocked, so the state
 * under test is the panel's own rendering, not a reimplementation.
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
    catalog: [] as Array<Record<string, unknown>>,
    stored: {} as Record<string, unknown>,
    unlisten: vi.fn(),
    merge,
  };
});

vi.mock("./api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./api")>();
  return {
    ...actual,
    onEvent: vi.fn(async () => state.unlisten),
    refresh: vi.fn(async () => {}),
    getSettings: vi.fn(async () => ({
      host: "localhost",
      port: 9998,
      username: "Guest",
      soundfont: null,
    })),
    getPrefs: vi.fn(async () => state.stored),
    setPrefs: vi.fn(async (patch: Record<string, unknown>) => {
      state.stored = state.merge(state.stored, patch);
      return state.stored;
    }),
    resetPrefs: vi.fn(async () => {
      state.stored = {};
      return state.stored;
    }),
    setConnectionSettings: vi.fn(async (host: string, port: number, username: string) => ({
      host,
      port,
      username,
      soundfont: null,
    })),
    openPreferences: vi.fn(async () => "created"),
    closePreferences: vi.fn(async () => "closed"),
    openPanel: vi.fn(async () => "created"),
    closePanel: vi.fn(async () => "closed"),
    setViewport: vi.fn(async () => {}),
    gotoRoom: vi.fn(async () => {}),
    bagCollections: vi.fn(async () => [
      { path: "/bags/My Bag.prp", name: "My Bag", writable: true, exists: true },
    ]),
    bagShelves: vi.fn(async () => []),
    bagCatalog: vi.fn(async () => state.catalog),
    bagTrashList: vi.fn(async () => []),
    outfitsList: vi.fn(async () => []),
  };
});

import { tick, mount, unmount } from "svelte";

import PanelWindow from "./PanelWindow.svelte";
import PreferencesWindow from "./components/prefs/PreferencesWindow.svelte";
import type { PanelId } from "./panels";
import { prefs } from "./prefsStore.svelte";
import { store } from "./store.svelte";

/* jsdom does not implement ResizeObserver; the room view observes its own box. */
class FakeResizeObserver {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
}
globalThis.ResizeObserver = FakeResizeObserver as unknown as typeof ResizeObserver;

let mounted: ReturnType<typeof mount> | undefined;

function teardown(): void {
  if (mounted) {
    unmount(mounted);
    mounted = undefined;
  }
  document.body.innerHTML = "";
  window.location.hash = "";
}

/** Mount the real `panel-*` window for one id and let its seeding settle. */
async function mountPanel(panel: PanelId): Promise<void> {
  window.location.hash = `#/panel/${panel}`;
  mounted = mount(PanelWindow, { target: document.body, props: { panel } });
  await tick();
  await tick();
}

function emptyNote(): string {
  return document.querySelector(".empty-note")?.textContent?.trim() ?? "";
}

function viewportEmpty(): { big: string; hint: string } {
  return {
    big: document.querySelector(".viewport .empty .big")?.textContent?.trim() ?? "",
    hint: document.querySelector(".viewport .empty")?.textContent?.trim() ?? "",
  };
}

function resetStore(): void {
  store.status = "connecting";
  store.statusMessage = null;
  store.users = [];
  store.rooms = [];
  store.room = null;
  store.roomFilter = "";
  store.chat = [];
  store.screen = null;
  store.bagCatalog = [];
  store.bagCollections = [];
  store.bagShelves = [];
  store.bagTrash = [];
  store.outfits = [];
  store.bagLoaded = false;
}

beforeEach(() => {
  vi.clearAllMocks();
  state.catalog = [];
  state.stored = {};
  prefs.reset();
  resetStore();
  document.body.innerHTML = "";
});

afterEach(teardown);

describe("Task 29 — the five panels render a named empty state", () => {
  it("Users panel: 'Not connected.' when disconnected, 'Nobody here.' when connected but empty", async () => {
    store.status = "disconnected";
    await mountPanel("users");
    expect(emptyNote()).toBe("Not connected.");

    store.status = "connected";
    await tick();
    expect(emptyNote()).toBe("Nobody here.");
  });

  it("Rooms panel: 'No room list yet.' when connected with no rooms, 'Not connected.' when disconnected", async () => {
    store.status = "connected";
    await mountPanel("rooms");
    expect(emptyNote()).toBe("No room list yet.");

    store.status = "disconnected";
    await tick();
    expect(emptyNote()).toBe("Not connected.");
  });

  it("Rooms panel: a filter that matches nothing says so instead of going blank", async () => {
    store.status = "connected";
    store.rooms = [{ id: 1, name: "Alpha", users: 3, flags: 0 }];
    await mountPanel("rooms");

    store.roomFilter = "zzz";
    await tick();
    expect(document.querySelectorAll(".sidebar .scroll .row")).toHaveLength(0);
    expect(emptyNote()).toContain("No rooms match");
    expect(emptyNote()).toContain("zzz");
  });

  it("Chat panel: names the disconnected state once, in the log and the input", async () => {
    store.status = "disconnected";
    await mountPanel("chat");

    expect(emptyNote()).toBe("Chat appears here once connected.");
    expect(
      document.querySelector<HTMLInputElement>('input[aria-label="Chat message"]')?.placeholder,
    ).toBe("Not connected");

    store.status = "connected";
    await tick();
    expect(emptyNote()).toBe("Say hello.");
  });

  it("Prop bag panel: an empty bag says 'No props in your bag yet.'", async () => {
    await mountPanel("props");

    await vi.waitFor(() =>
      expect(document.querySelector(".dialog-hint")?.textContent).toContain(
        "No props in your bag yet.",
      ),
    );
  });

  it("Prop bag panel: a query that matches nothing says 'Nothing matches this view.'", async () => {
    state.catalog = [
      {
        id: 1,
        crc: 1001,
        name: "Prop 1",
        w: 44,
        h: 44,
        flags: 0,
        fav: false,
        trash: false,
        collection: "My Bag",
        source: "bag",
      },
    ];
    await mountPanel("props");
    await vi.waitFor(() => expect(document.querySelectorAll(".bag-tile")).toHaveLength(1));

    const search = document.querySelector<HTMLInputElement>(
      'input[aria-label="Search props by name, id or crc"]',
    )!;
    search.value = "zzz";
    search.dispatchEvent(new Event("input", { bubbles: true }));
    await tick();

    expect(document.querySelectorAll(".bag-tile")).toHaveLength(0);
    expect(document.querySelector(".dialog-hint")?.textContent).toContain(
      "Nothing matches this view.",
    );
  });

  it("Room view panel: 'Offline' when disconnected, 'Waiting for a room' when connected", async () => {
    store.status = "disconnected";
    await mountPanel("room");
    expect(viewportEmpty().big).toBe("Offline");

    store.status = "connected";
    await tick();
    expect(viewportEmpty().big).toBe("Waiting for a room");
    expect(viewportEmpty().hint).toContain("The server has not described a room yet.");
  });

  it("Preferences window: an empty last-servers list and an empty ignore list each say so", async () => {
    mounted = mount(PreferencesWindow, { target: document.body });
    await tick();
    await vi.waitFor(() =>
      expect(document.querySelector('[data-testid="last-servers-empty"]')).not.toBeNull(),
    );

    const muteNav = [...document.querySelectorAll<HTMLButtonElement>(".prefs-nav-item")].find(
      (button) => button.textContent?.includes("Mute/ignore"),
    )!;
    muteNav.click();
    await vi.waitFor(() =>
      expect(document.querySelector('[data-testid="mute-empty"]')?.textContent?.trim()).toBe(
        "Nobody is ignored yet.",
      ),
    );
  });
});
