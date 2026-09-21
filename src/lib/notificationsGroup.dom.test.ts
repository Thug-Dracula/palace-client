// @vitest-environment jsdom
/**
 * Task 26's acceptance tests through the real window: the Notifications group.
 *
 * The plan asks for three things and each has a test that can fail:
 *
 * * notifications are opt-in — the toggle starts off and switching it on
 *   persists `notifications.enabled`, so the Rust pump can act on it;
 * * the scope control writes the two stored flags the pump reads;
 * * the sound option is disabled, visibly marked and writes nothing.
 *
 * The shell routes `notifications` to the real component, so one test mounts
 * the whole PreferencesWindow and clicks the navigation item, proving the group
 * is no longer the placeholder.
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
    failNextSetPrefs: false,
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
      if (state.failNextSetPrefs) {
        state.failNextSetPrefs = false;
        throw new Error("the disk is full");
      }
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

import PreferencesWindow from "./components/prefs/PreferencesWindow.svelte";
import NotificationsGroup from "./components/prefs/NotificationsGroup.svelte";
import { prefs } from "./prefsStore.svelte";

let mounted: ReturnType<typeof mount> | undefined;

function byTestId<T extends HTMLElement>(id: string): T {
  const node = document.querySelector<T>(`[data-testid="${id}"]`);
  if (!node) {
    throw new Error(`no element with data-testid ${id}`);
  }
  return node;
}

function storedNotifications(): Record<string, unknown> {
  return (state.stored.notifications ?? {}) as Record<string, unknown>;
}

async function renderGroup(): Promise<void> {
  await prefs.load();
  mounted = mount(NotificationsGroup, { target: document.body });
  await tick();
}

async function toggleEnabled(): Promise<void> {
  const box = byTestId<HTMLInputElement>("notifications-enabled");
  box.checked = true;
  box.dispatchEvent(new Event("change", { bubbles: true }));
  await vi.waitFor(() => expect(storedNotifications().enabled).toBe(true));
  await tick();
}

beforeEach(() => {
  vi.clearAllMocks();
  state.stored = {};
  state.failNextSetPrefs = false;
  prefs.reset();
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
  it("renders the real group when Notifications is chosen", async () => {
    mounted = mount(PreferencesWindow, { target: document.body });
    await tick();
    await vi.waitFor(() =>
      expect(document.querySelectorAll(".prefs-nav-item").length).toBeGreaterThan(0),
    );

    const nav = [...document.querySelectorAll<HTMLButtonElement>(".prefs-nav-item")].find(
      (node) => node.textContent?.trim() === "Notifications",
    );
    expect(nav).toBeDefined();
    nav?.click();
    await tick();

    expect(byTestId("notifications-group")).toBeTruthy();
    expect(document.querySelector(".prefs-placeholder")).toBeNull();
    expect(document.querySelector(".prefs-content h1")?.textContent).toBe("Notifications");
  });
});

describe("opting in", () => {
  it("starts off and persists the master toggle when switched on", async () => {
    await renderGroup();
    const box = byTestId<HTMLInputElement>("notifications-enabled");
    expect(box.checked).toBe(false);
    expect(byTestId<HTMLSelectElement>("notifications-scope").disabled).toBe(true);

    await toggleEnabled();

    expect(storedNotifications().enabled).toBe(true);
    const select = byTestId<HTMLSelectElement>("notifications-scope");
    await vi.waitFor(() => expect(select.disabled).toBe(false));
  });

  it("persists the scope as the two flags the pump reads", async () => {
    state.stored = { notifications: { enabled: true, on_mention: true, private_message: true } };
    await renderGroup();

    const select = byTestId<HTMLSelectElement>("notifications-scope");
    expect(select.value).toBe("all");

    select.value = "mentions";
    select.dispatchEvent(new Event("change", { bubbles: true }));
    await vi.waitFor(() => expect(storedNotifications().on_mention).toBe(true));
    await tick();
    expect(storedNotifications().private_message).toBe(false);
  });

  it("keeps the toggle off when the write fails", async () => {
    await renderGroup();
    state.failNextSetPrefs = true;

    const box = byTestId<HTMLInputElement>("notifications-enabled");
    box.checked = true;
    box.dispatchEvent(new Event("change", { bubbles: true }));
    await vi.waitFor(() => expect(box.checked).toBe(false));

    expect(storedNotifications().enabled).toBeUndefined();
    expect(document.querySelector(".prefs-error")?.textContent).toContain("Could not save");
  });
});

describe("the unsupported sound option", () => {
  it("is disabled, marked, and writes nothing", async () => {
    state.stored = { notifications: { enabled: true } };
    await renderGroup();

    const sound = byTestId<HTMLInputElement>("notifications-sound");
    expect(sound.disabled).toBe(true);
    const chip = byTestId("unsupported-sound");
    expect(chip.textContent?.trim()).toBe("Not supported");
    const row = chip.closest(".prefs-option");
    expect(row?.querySelector(".prefs-note")?.textContent).toContain("plays its own sound");

    sound.checked = true;
    sound.dispatchEvent(new Event("change", { bubbles: true }));
    await tick();
    expect(storedNotifications().sound).toBeUndefined();
  });
});
