// @vitest-environment jsdom
/**
 * Task 19's acceptance tests: the Preferences window shell.
 *
 * These prove the four behaviours the plan names: the window opens from the top
 * bar and lists all ten groups; a live-apply change persists and takes effect
 * without a restart; a connection change shows the reconnect affordance and
 * never silently reconnects; and "restore defaults" reverts and saves.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const state = vi.hoisted(() => ({ stored: {} as Record<string, unknown> }));

vi.mock("./api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./api")>();
  return {
    ...actual,
    getSettings: vi.fn(async () => ({
      host: "localhost",
      port: 9998,
      username: "Guest",
      soundfont: null,
    })),
    getPrefs: vi.fn(async () => state.stored),
    setPrefs: vi.fn(async (patch: Record<string, unknown>) => {
      state.stored = { ...state.stored, ...patch };
      return state.stored;
    }),
    resetPrefs: vi.fn(async () => {
      state.stored = { shell: { last_group: "connection" } };
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
    connect: vi.fn(async () => {}),
    disconnect: vi.fn(async () => {}),
    onEvent: vi.fn(async () => () => {}),
    refresh: vi.fn(async () => {}),
    setViewport: vi.fn(async () => {}),
  };
});

import { mount, tick, unmount } from "svelte";

import App from "./App.svelte";
import * as api from "./api";
import TopBar from "./components/TopBar.svelte";
import PreferencesWindow from "./components/prefs/PreferencesWindow.svelte";
import { PREF_GROUPS } from "./prefs";
import { prefs } from "./prefsStore.svelte";

class FakeResizeObserver {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
}
globalThis.ResizeObserver = FakeResizeObserver as unknown as typeof ResizeObserver;

let mounted: ReturnType<typeof mount> | undefined;

function navItem(label: string): HTMLButtonElement {
  const button = [...document.querySelectorAll<HTMLButtonElement>(".prefs-nav-item")].find(
    (node) => node.textContent?.trim() === label,
  );
  if (!button) {
    throw new Error(`no navigation item labelled ${label}`);
  }
  return button;
}

async function renderPreferences(): Promise<void> {
  mounted = mount(PreferencesWindow, { target: document.body });
  await tick();
  await vi.waitFor(() =>
    expect(document.querySelectorAll(".prefs-nav-item")).toHaveLength(PREF_GROUPS.length),
  );
}

async function waitForServerField(value: string): Promise<HTMLInputElement> {
  const input = document.querySelector<HTMLInputElement>(".prefs-field input");
  await vi.waitFor(() => expect(input?.value).toBe(value));
  return input as HTMLInputElement;
}

async function changeField(input: HTMLInputElement, value: string): Promise<void> {
  input.value = value;
  input.dispatchEvent(new Event("input", { bubbles: true }));
  input.dispatchEvent(new Event("change", { bubbles: true }));
  await tick();
}

beforeEach(() => {
  vi.clearAllMocks();
  state.stored = {};
  prefs.reset();
  document.body.innerHTML = "";
});

afterEach(() => {
  if (mounted) {
    unmount(mounted);
    mounted = undefined;
  }
  document.body.innerHTML = "";
  document.documentElement.removeAttribute("style");
});

describe("opening Preferences", () => {
  it("opens the Preferences window from the top bar", async () => {
    mounted = mount(TopBar, { target: document.body });
    await tick();

    const button = [...document.querySelectorAll("button")].find((node) =>
      node.textContent?.includes("Preferences"),
    );
    expect(button, "the top bar offers a Preferences entry point").toBeTruthy();
    button?.click();

    await vi.waitFor(() => expect(vi.mocked(api.openPreferences)).toHaveBeenCalledTimes(1));
  });

  it("lists all ten groups and opens on the first one", async () => {
    await renderPreferences();

    const labels = [...document.querySelectorAll(".prefs-nav-item")].map((node) =>
      node.textContent?.trim(),
    );
    expect(labels).toEqual(PREF_GROUPS.map((group) => group.label));
    expect(labels).toHaveLength(10);
    expect(document.querySelector(".prefs-nav-item.active")?.textContent?.trim()).toBe(
      "Connection & identity",
    );
    expect(document.querySelector(".prefs-content h1")?.textContent).toBe("Connection & identity");
  });

  it("routes the #/prefs hash to the Preferences window, not the shell", async () => {
    mounted = mount(App, { target: document.body, props: { hash: "#/prefs" } });
    await tick();

    expect(document.querySelector(".prefs-window")).not.toBeNull();
    expect(document.querySelector(".app")).toBeNull();
    expect(document.querySelector("header.topbar")).toBeNull();
  });
});

describe("group wiring", () => {
  it("renders the real Appearance group rather than the placeholder", async () => {
    await renderPreferences();

    navItem("Appearance").click();
    await tick();

    await vi.waitFor(() =>
      expect(document.querySelector(".prefs-content h1")?.textContent).toBe("Appearance"),
    );
    expect(
      document.querySelector("#prefs-theme"),
      "the Appearance group must be wired into the shell",
    ).not.toBeNull();
    expect(
      document.querySelector(".prefs-placeholder"),
      "Appearance must not fall through to the placeholder",
    ).toBeNull();
  });

  it("applies a saved appearance in the main shell", async () => {
    state.stored = { appearance: { tokens: { amber: "#123456" } } };

    mounted = mount(App, { target: document.body, props: { hash: "#/" } });
    await tick();

    await vi.waitFor(() =>
      expect(document.documentElement.style.getPropertyValue("--amber")).toBe("#123456"),
    );
  });
});

describe("live-apply preferences", () => {
  it("persists a live change and applies it without a restart", async () => {
    await renderPreferences();

    navItem("Sound").click();
    await tick();

    await vi.waitFor(() =>
      expect(vi.mocked(api.setPrefs)).toHaveBeenCalledWith({ shell: { last_group: "sound" } }),
    );
    expect(state.stored).toMatchObject({ shell: { last_group: "sound" } });
    expect(document.querySelector(".prefs-content h1")?.textContent).toBe("Sound");
    expect(document.querySelector(".prefs-nav-item.active")?.textContent?.trim()).toBe("Sound");
  });
});

describe("connection preferences need a reconnect", () => {
  it("shows the reconnect affordance and never reconnects on its own", async () => {
    await renderPreferences();
    const server = await waitForServerField("localhost");
    expect(document.querySelector(".prefs-reconnect")).toBeNull();

    await changeField(server, "elsewhere.test");

    await vi.waitFor(() =>
      expect(vi.mocked(api.setConnectionSettings)).toHaveBeenCalledWith(
        "elsewhere.test",
        9998,
        "Guest",
      ),
    );
    const affordance = document.querySelector(".prefs-reconnect");
    expect(affordance?.textContent).toContain("Reconnect required");
    expect(document.querySelector(".prefs-reconnect-chip")?.textContent).toBe("Reconnect required");
    expect(vi.mocked(api.connect)).not.toHaveBeenCalled();
    expect(vi.mocked(api.disconnect)).not.toHaveBeenCalled();
  });

  it("rejects an out-of-range port instead of saving it", async () => {
    await renderPreferences();
    await waitForServerField("localhost");

    const port = document.querySelectorAll<HTMLInputElement>(".prefs-field input")[1];
    await changeField(port, "0");

    expect(document.querySelector(".prefs-error")?.textContent).toContain("port");
    expect(vi.mocked(api.setConnectionSettings)).not.toHaveBeenCalled();
    expect(document.querySelector(".prefs-reconnect")).toBeNull();
  });
});

describe("restore defaults", () => {
  it("reverts the values and saves them", async () => {
    await renderPreferences();

    navItem("Sound").click();
    await vi.waitFor(() =>
      expect(state.stored).toMatchObject({ shell: { last_group: "sound" } }),
    );

    const restore = [...document.querySelectorAll("button")].find((node) =>
      node.textContent?.includes("Restore defaults"),
    );
    expect(restore, "the footer offers Restore defaults").toBeTruthy();
    restore?.click();

    await vi.waitFor(() => expect(vi.mocked(api.resetPrefs)).toHaveBeenCalledTimes(1));
    await vi.waitFor(() =>
      expect(document.querySelector(".prefs-content h1")?.textContent).toBe("Connection & identity"),
    );
    expect(vi.mocked(api.setPrefs)).not.toHaveBeenCalledWith({ shell: { last_group: "connection" } });
  });
});
