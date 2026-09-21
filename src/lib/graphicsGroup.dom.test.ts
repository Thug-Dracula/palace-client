// @vitest-environment jsdom
/**
 * Task 22's acceptance tests: the Room & graphics preference group.
 *
 * The plan asks for three things and each has a test that can fail:
 *
 * * a supported toggle persists AND applies live (the room frame re-composes);
 * * an unsupported option is disabled, visibly marked and writes nothing;
 * * a failed write does not leave a control showing a state the file lacks.
 *
 * The shell routes `graphics` to the real component, so one test mounts the
 * whole PreferencesWindow and clicks the navigation item, proving the group is
 * no longer the placeholder. The guest filter is proved through the real
 * UserList, because that is the view the option changes.
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
    setVisibility: vi.fn(async () => {}),
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
  };
});

import { mount, tick, unmount } from "svelte";

import * as api from "./api";
import type { UserInfo } from "./api";
import PreferencesWindow from "./components/prefs/PreferencesWindow.svelte";
import GraphicsGroup from "./components/prefs/GraphicsGroup.svelte";
import UserList from "./components/UserList.svelte";
import { prefs } from "./prefsStore.svelte";
import { store } from "./store.svelte";

let mounted: ReturnType<typeof mount> | undefined;
let extra: ReturnType<typeof mount> | undefined;

function byTestId<T extends HTMLElement>(id: string): T {
  const node = document.querySelector<T>(`[data-testid="${id}"]`);
  if (!node) {
    throw new Error(`no element with data-testid ${id}`);
  }
  return node;
}

function storedGraphics(): Record<string, unknown> {
  return (state.stored.graphics ?? {}) as Record<string, unknown>;
}

/** One roster entry, with the fields the user list reads made explicit. */
function user(id: number, name: string, opts: { self?: boolean } = {}): UserInfo {
  return {
    id,
    name,
    face: 0,
    color: id,
    room_id: 1,
    x: 0,
    y: 0,
    props: [],
    away: false,
    is_self: opts.self ?? false,
    avatar_type: 0,
    avatar_flags: 0,
    avatar_hash: null,
  };
}

async function renderGroup(): Promise<void> {
  await prefs.load();
  mounted = mount(GraphicsGroup, { target: document.body });
  await tick();
}

async function clickBox(id: string): Promise<HTMLInputElement> {
  const box = byTestId<HTMLInputElement>(id);
  box.click();
  await tick();
  return box;
}

beforeEach(() => {
  vi.clearAllMocks();
  state.stored = {};
  state.failNextSetPrefs = false;
  prefs.reset();
  store.showNames = true;
  store.showAvatars = true;
  store.showGuests = true;
  store.users = [];
  document.body.innerHTML = "";
});

afterEach(() => {
  if (mounted) {
    unmount(mounted);
    mounted = undefined;
  }
  if (extra) {
    unmount(extra);
    extra = undefined;
  }
  document.body.innerHTML = "";
});

describe("supported options: persist and apply live", () => {
  it("toggling Show names off persists the value and re-composes the room frame", async () => {
    await renderGroup();
    expect(byTestId<HTMLInputElement>("graphics-show-names").checked).toBe(true);

    await clickBox("graphics-show-names");

    await vi.waitFor(() => expect(storedGraphics().show_names).toBe(false));
    await vi.waitFor(() =>
      expect(vi.mocked(api.setVisibility)).toHaveBeenCalledWith(false, true),
    );
    expect(store.showNames).toBe(false);
    expect(store.showAvatars).toBe(true);
  });

  it("toggling Show avatars off persists and tells the backend", async () => {
    await renderGroup();

    await clickBox("graphics-show-avatars");

    await vi.waitFor(() => expect(storedGraphics().show_avatars).toBe(false));
    await vi.waitFor(() =>
      expect(vi.mocked(api.setVisibility)).toHaveBeenCalledWith(true, false),
    );
    expect(store.showAvatars).toBe(false);
  });

  it("applies a stored value on load, so the choice survives a restart", async () => {
    state.stored = { graphics: { show_names: false, show_avatars: true } };
    await renderGroup();

    expect(byTestId<HTMLInputElement>("graphics-show-names").checked).toBe(false);
    expect(store.showNames).toBe(false);
    await vi.waitFor(() =>
      expect(vi.mocked(api.setVisibility)).toHaveBeenCalledWith(false, true),
    );
    expect(storedGraphics().show_names).toBe(false);
  });

  it("persists a toggle, then shows the stored value after a remount", async () => {
    await renderGroup();

    await clickBox("graphics-show-names");
    await vi.waitFor(() => expect(storedGraphics().show_names).toBe(false));

    unmount(mounted as ReturnType<typeof mount>);
    mounted = undefined;
    prefs.reset();
    store.showNames = true;
    await renderGroup();

    expect(byTestId<HTMLInputElement>("graphics-show-names").checked).toBe(false);
    expect(store.showNames).toBe(false);
  });

  it("persists a Show guests toggle without touching the backend visibility", async () => {
    await renderGroup();

    await clickBox("graphics-show-guests");

    await vi.waitFor(() => expect(storedGraphics().show_guests).toBe(false));
    expect(store.showGuests).toBe(false);
    expect(vi.mocked(api.setVisibility)).not.toHaveBeenCalled();
  });

  it("keeps the control honest when the write fails", async () => {
    await renderGroup();
    state.failNextSetPrefs = true;

    const box = await clickBox("graphics-show-names");

    await vi.waitFor(() =>
      expect(document.querySelector(".prefs-error")?.textContent).toContain("disk is full"),
    );
    expect(box.checked).toBe(true);
    expect(store.showNames).toBe(true);
    expect(storedGraphics().show_names).toBeUndefined();
    expect(vi.mocked(api.setVisibility)).not.toHaveBeenCalled();
  });
});

describe("the guest filter reaches the user list", () => {
  it("hides Guest names live and puts them back when re-enabled", async () => {
    store.users = [user(1, "Ada"), user(2, "Guest1234"), user(3, "Grace", { self: true })];
    await renderGroup();

    extra = mount(UserList, { target: document.body });
    await tick();
    const names = (): string[] =>
      [...document.querySelectorAll(".sidebar .row .name")].map((node) => node.textContent ?? "");
    expect(names()).toEqual(["Ada", "Guest1234", "Grace"]);

    await clickBox("graphics-show-guests");
    await vi.waitFor(() => expect(storedGraphics().show_guests).toBe(false));
    await tick();
    expect(names()).toEqual(["Ada", "Grace"]);
    expect(document.querySelector(".sidebar .count")?.textContent).toBe("2");

    await clickBox("graphics-show-guests");
    await tick();
    expect(names()).toEqual(["Ada", "Guest1234", "Grace"]);
  });

  it("never hides the signed-in user, even when their own name reads as a guest", async () => {
    store.users = [user(1, "Ada"), user(2, "Guest", { self: true })];
    await renderGroup();

    await clickBox("graphics-show-guests");

    await vi.waitFor(() => expect(store.showGuests).toBe(false));
    expect(store.visibleUsers.map((entry) => entry.name)).toEqual(["Ada", "Guest"]);
  });
});

describe("unsupported options", () => {
  it("renders every unsupported option disabled, marked and explained", async () => {
    await renderGroup();

    for (const [control, marker] of [
      ["graphics-animation", "unsupported-animation"],
      ["graphics-tinted-balloons", "unsupported-tinted_balloons"],
      ["graphics-balloon-delay", "unsupported-balloon_delay"],
    ] as const) {
      expect(byTestId<HTMLInputElement | HTMLSelectElement>(control).disabled).toBe(true);
      const chip = byTestId(marker);
      expect(chip.textContent?.trim()).toBe("Not supported");
      const row = chip.closest(".prefs-option");
      expect(row?.querySelector(".prefs-note")?.textContent?.trim().length ?? 0).toBeGreaterThan(
        0,
      );
    }
  });

  it("writes nothing when an unsupported control is poked", async () => {
    await renderGroup();

    const animation = byTestId<HTMLInputElement>("graphics-animation");
    animation.checked = !animation.checked;
    animation.dispatchEvent(new Event("change", { bubbles: true }));
    const delay = byTestId<HTMLSelectElement>("graphics-balloon-delay");
    delay.value = "fast";
    delay.dispatchEvent(new Event("change", { bubbles: true }));
    await tick();

    expect(vi.mocked(api.setPrefs)).not.toHaveBeenCalled();
    expect(storedGraphics()).toEqual({});
  });
});

describe("the Preferences window shell", () => {
  it("renders the real group when Room & graphics behaviour is chosen", async () => {
    mounted = mount(PreferencesWindow, { target: document.body });
    await tick();
    await vi.waitFor(() =>
      expect(document.querySelectorAll(".prefs-nav-item").length).toBeGreaterThan(0),
    );

    const nav = [...document.querySelectorAll<HTMLButtonElement>(".prefs-nav-item")].find(
      (node) => node.textContent?.trim() === "Room & graphics behaviour",
    );
    expect(nav).toBeDefined();
    nav?.click();
    await tick();

    expect(byTestId("graphics-show-names")).toBeTruthy();
    expect(document.querySelector(".prefs-placeholder")).toBeNull();
    expect(document.querySelector(".prefs-content h1")?.textContent).toBe(
      "Room & graphics behaviour",
    );
  });

  it("renders the avatar group through the same navigation", async () => {
    mounted = mount(PreferencesWindow, { target: document.body });
    await tick();
    await vi.waitFor(() =>
      expect(document.querySelectorAll(".prefs-nav-item").length).toBeGreaterThan(0),
    );

    const nav = [...document.querySelectorAll<HTMLButtonElement>(".prefs-nav-item")].find(
      (node) => node.textContent?.trim() === "Avatar & prop behaviour",
    );
    nav?.click();
    await tick();

    expect(byTestId("avatar-prop-animation")).toBeTruthy();
    expect(document.querySelector(".prefs-placeholder")).toBeNull();
  });
});
