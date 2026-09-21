// @vitest-environment jsdom
/**
 * Task 27's acceptance tests through the real window: the Layout memory group.
 *
 * The plan asks for three things and each has a test that can fail:
 *
 * * "remember layout" off stops the writing — the switch persists through the
 *   layout store, and the group never touches the shared `prefs` block;
 * * "reset layout to default" re-docks everything immediately: the panel list
 *   is empty afterwards and the backend reports no panel window;
 * * the detached list reflects reality, including a change made in another
 *   window while this one is open.
 *
 * The shell routes `layout` to the real component, so one test mounts the whole
 * PreferencesWindow and clicks the navigation item, proving the group is no
 * longer the placeholder.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const state = vi.hoisted(() => ({
  /** The fake layout store the mocked commands read and write. */
  layout: {
    remember: true,
    detached: [] as string[],
    path: "$HOME/.config/org.palace.client/window-layout.json",
    /** How many panel windows the fake backend still has open. */
    panelWindows: 0,
  },
  failNextRemember: false,
  failNextReset: false,
  /** Every listener the group registered, so a test can broadcast an event. */
  listeners: {} as Record<string, ((event: { payload: unknown }) => void)[]>,
  invoke: vi.fn(async () => true),
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke: state.invoke }));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(
    async (event: string, handler: (event: { payload: unknown }) => void) => {
      (state.listeners[event] ??= []).push(handler);
      return () => {
        state.listeners[event] = (state.listeners[event] ?? []).filter(
          (registered) => registered !== handler,
        );
      };
    },
  ),
}));

function snapshot(): {
  remember: boolean;
  detached: string[];
  path: string | null;
} {
  return {
    remember: state.layout.remember,
    detached: [...state.layout.detached],
    path: state.layout.path,
  };
}

vi.mock("./api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./api")>();
  return {
    ...actual,
    getLayoutMemory: vi.fn(async () => snapshot()),
    setLayoutRemember: vi.fn(async (enabled: boolean) => {
      if (state.failNextRemember) {
        state.failNextRemember = false;
        throw new Error("the layout file is read-only");
      }
      state.layout.remember = enabled;
      return snapshot();
    }),
    resetLayout: vi.fn(async () => {
      if (state.failNextReset) {
        state.failNextReset = false;
        throw new Error("the window refused to close");
      }
      state.layout.detached = [];
      state.layout.panelWindows = 0;
      return snapshot();
    }),
    getPrefs: vi.fn(async () => ({})),
    setPrefs: vi.fn(async () => ({})),
    resetPrefs: vi.fn(async () => ({})),
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
import LayoutGroup from "./components/prefs/LayoutGroup.svelte";
import PreferencesWindow from "./components/prefs/PreferencesWindow.svelte";
import { prefs } from "./prefsStore.svelte";

let mounted: ReturnType<typeof mount> | undefined;

function byTestId<T extends HTMLElement>(id: string): T {
  const node = document.querySelector<T>(`[data-testid="${id}"]`);
  if (!node) {
    throw new Error(`no element with data-testid ${id}`);
  }
  return node;
}

function hasTestId(id: string): boolean {
  return document.querySelector(`[data-testid="${id}"]`) !== null;
}

async function renderGroup(): Promise<void> {
  mounted = mount(LayoutGroup, { target: document.body });
  await tick();
  await vi.waitFor(() => expect(hasTestId("layout-remember")).toBe(true));
  await tick();
}

async function toggleRemember(checked: boolean): Promise<void> {
  const box = byTestId<HTMLInputElement>("layout-remember");
  box.checked = checked;
  box.dispatchEvent(new Event("change", { bubbles: true }));
  await vi.waitFor(() => expect(api.setLayoutRemember).toHaveBeenCalled());
  await tick();
}

beforeEach(() => {
  vi.clearAllMocks();
  state.layout = {
    remember: true,
    detached: [],
    path: "$HOME/.config/org.palace.client/window-layout.json",
    panelWindows: 0,
  };
  state.failNextRemember = false;
  state.failNextReset = false;
  state.listeners = {};
  prefs.reset();
  document.body.innerHTML = "";
});

afterEach(() => {
  if (mounted) {
    void unmount(mounted);
    mounted = undefined;
  }
});

describe("the Layout memory group", () => {
  it("shows the remembered state and the panels that are detached right now", async () => {
    state.layout.detached = ["users", "chat"];
    state.layout.panelWindows = 2;

    await renderGroup();

    expect(byTestId<HTMLInputElement>("layout-remember").checked).toBe(true);
    expect(byTestId("layout-detached-users").textContent).toContain("Users");
    expect(byTestId("layout-detached-chat").textContent).toContain("Chat");
    expect(hasTestId("layout-detached-room")).toBe(false);
    expect(byTestId("layout-path").textContent).toContain("window-layout.json");
  });

  it("says so when nothing is detached", async () => {
    await renderGroup();
    expect(byTestId("layout-detached-none").textContent).toContain("All panels are docked");
  });

  it("turns layout memory off through the layout store, never through prefs", async () => {
    await renderGroup();

    await toggleRemember(false);

    expect(api.setLayoutRemember).toHaveBeenCalledWith(false);
    expect(byTestId<HTMLInputElement>("layout-remember").checked).toBe(false);
    expect(byTestId("layout-remember-off-note").textContent).toContain(
      "default single-window layout",
    );
    expect(byTestId("layout-notice").textContent).toContain("nothing more is saved");
    expect(state.layout.remember).toBe(false);
    expect(api.setPrefs).not.toHaveBeenCalled();
    expect(api.resetPrefs).not.toHaveBeenCalled();
  });

  it("puts the switch back and reports the error when the change fails", async () => {
    state.failNextRemember = true;
    await renderGroup();

    await toggleRemember(false);

    expect(byTestId<HTMLInputElement>("layout-remember").checked).toBe(true);
    expect(document.querySelector('[role="alert"]')?.textContent).toContain(
      "Could not change layout memory",
    );
    expect(state.layout.remember).toBe(true);
  });

  it("re-docks every panel immediately and leaves no panel window", async () => {
    state.layout.detached = ["users", "chat"];
    state.layout.panelWindows = 2;
    await renderGroup();
    expect(hasTestId("layout-detached-list")).toBe(true);

    byTestId("layout-reset").click();
    await vi.waitFor(() => expect(api.resetLayout).toHaveBeenCalledTimes(1));
    await tick();

    expect(hasTestId("layout-detached-list")).toBe(false);
    expect(byTestId("layout-detached-none").textContent).toContain("All panels are docked");
    expect(state.layout.detached).toEqual([]);
    expect(state.layout.panelWindows).toBe(0);
    expect(api.setPrefs).not.toHaveBeenCalled();
  });

  it("keeps the list when the reset fails", async () => {
    state.layout.detached = ["users"];
    state.failNextReset = true;
    await renderGroup();

    byTestId("layout-reset").click();
    await vi.waitFor(() => expect(api.resetLayout).toHaveBeenCalled());
    await tick();

    expect(byTestId("layout-detached-users").textContent).toContain("Users");
    expect(document.querySelector('[role="alert"]')?.textContent).toContain(
      "Could not reset the layout",
    );
  });

  it("follows a detach that happened in another window", async () => {
    await renderGroup();
    expect(byTestId("layout-detached-none")).toBeTruthy();

    state.layout.detached = ["rooms"];
    for (const listener of state.listeners["palace://layout"] ?? []) {
      listener({ payload: snapshot() });
    }
    await tick();

    expect(byTestId("layout-detached-rooms").textContent).toContain("Rooms");
    expect(hasTestId("layout-detached-none")).toBe(false);
  });
});

describe("the Preferences shell", () => {
  it("routes the layout group to the real component instead of the placeholder", async () => {
    mounted = mount(PreferencesWindow, { target: document.body });
    // The window loads the stored group on mount; let that settle first, or it
    // would put the content area back after the click.
    await vi.waitFor(() => expect(prefs.loaded).toBe(true));
    await tick();

    const nav = [...document.querySelectorAll<HTMLButtonElement>("button.prefs-nav-item")].find(
      (button) => button.textContent?.includes("Layout memory"),
    );
    expect(nav, "the navigation lists the Layout memory group").toBeTruthy();
    nav!.click();
    await vi.waitFor(() => expect(hasTestId("layout-remember")).toBe(true));
    await tick();

    expect(hasTestId("layout-reset")).toBe(true);
    expect(api.getLayoutMemory).toHaveBeenCalled();
  });
});
