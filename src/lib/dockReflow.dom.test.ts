// @vitest-environment jsdom
/**
 * Task 12 acceptance: every detach combination renders without layout errors,
 * a detached slot collapses to a re-attach rail, and docking again restores the
 * original grid slot.
 *
 * jsdom has no real layout engine, so "the room widened" is asserted two ways
 * that are both objective: the pure track maths (`layout.test.ts`) proves the
 * numbers, and here the applied inline custom properties prove the shell wires
 * those numbers into the grid. The components prove which slot renders.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const { isPanelWindowMock, listenMock } = vi.hoisted(() => ({
  isPanelWindowMock: vi.fn(() => false),
  listenMock: vi.fn(async () => () => {}),
}));

vi.mock("@tauri-apps/api/event", () => ({ listen: listenMock }));
vi.mock("./windowRole", () => ({ isPanelWindow: isPanelWindowMock }));
vi.mock("./api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./api")>();
  return {
    ...actual,
    onEvent: vi.fn(async () => () => {}),
    refresh: vi.fn(async () => {}),
    getSettings: vi.fn(async () => ({
      host: "localhost",
      port: 9998,
      username: "Guest",
      soundfont: null,
    })),
    setViewport: vi.fn(async () => {}),
    openPanel: vi.fn(async () => "created"),
    closePanel: vi.fn(async () => "closed"),
  };
});

import { tick, mount, unmount } from "svelte";

import App from "./App.svelte";
import * as api from "./api";
import { panelLayout } from "./panelLayout.svelte";
import { store } from "./store.svelte";

class FakeResizeObserver {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
}
globalThis.ResizeObserver = FakeResizeObserver as unknown as typeof ResizeObserver;

/** The four panels that occupy a track in the main shell. */
const DOCKED = ["rooms", "room", "chat", "users"] as const;

let mounted: ReturnType<typeof mount> | undefined;

async function renderMain(): Promise<void> {
  mounted = mount(App, { target: document.body });
  await tick();
  await tick();
}

const workspace = (): HTMLElement => document.querySelector<HTMLElement>(".workspace")!;
const placeholder = (panel: string): Element | null =>
  document.querySelector(`.panel-placeholder[data-panel="${panel}"]`);
const live = (panel: string): Element | null =>
  document.querySelector(`[data-panel="${panel}"]:not(.panel-placeholder)`);

/**
 * Read one inline custom property from the workspace.
 *
 * The shell sets a single style string; Svelte re-serializes it with a space
 * after each colon, so this parses rather than string-matches.
 */
function track(name: string): string | null {
  const style = workspace().getAttribute("style") ?? "";
  const match = style.match(new RegExp(`${name}\\s*:\\s*([^;]+)`));
  return match ? match[1].trim() : null;
}

beforeEach(() => {
  panelLayout.reset();
  vi.clearAllMocks();
  isPanelWindowMock.mockReturnValue(false);
  listenMock.mockResolvedValue(() => {});
  store.status = "connecting";
  store.rooms = [];
  store.users = [];
  store.chat = [];
  document.body.innerHTML = "";
});

afterEach(() => {
  if (mounted) {
    unmount(mounted);
    mounted = undefined;
  }
  panelLayout.reset();
  document.body.innerHTML = "";
});

describe("grid reflow — none detached", () => {
  it("docks every panel and applies the docked track sizes", async () => {
    await renderMain();

    expect(workspace().getAttribute("data-detached")).toBe("");
    for (const panel of DOCKED) {
      expect(live(panel), `${panel} docked`).not.toBeNull();
      expect(placeholder(panel), `${panel} has no placeholder`).toBeNull();
    }

    expect(track("--col-rooms")).toBe("232px");
    expect(track("--col-users")).toBe("208px");
    expect(track("--row-chat")).toBe("236px");
  });

  it("gives each docked panel header a detach control", async () => {
    await renderMain();
    for (const panel of DOCKED) {
      expect(document.querySelector(`[data-detach="${panel}"]`), `${panel} control`).not.toBeNull();
    }
  });
});

describe("grid reflow — one detached", () => {
  it("collapses the Users slot to a rail and shows its re-attach placeholder", async () => {
    panelLayout.setDetached("users", true);
    await renderMain();

    expect(workspace().getAttribute("data-detached")).toBe("users");
    expect(live("users")).toBeNull();
    const slot = placeholder("users");
    expect(slot).not.toBeNull();
    expect(slot?.querySelector("button")?.textContent?.trim()).toBe("Reattach");

    expect(track("--col-users")).toBe("44px");
    // The room keeps its left column and gains the space the rail gave back.
    expect(track("--col-rooms")).toBe("232px");
    expect(track("--row-chat")).toBe("236px");
  });

  it("shows the full placeholder (not a rail) when the room view is detached", async () => {
    panelLayout.setDetached("room", true);
    await renderMain();

    expect(workspace().getAttribute("data-detached")).toBe("room");
    expect(document.querySelector('.center > .panel-placeholder[data-panel="room"]')).not.toBeNull();
    expect(live("room")).toBeNull();
    // No track changes for the room view: it owns none.
    expect(track("--col-rooms")).toBe("232px");
    expect(track("--col-users")).toBe("208px");
    expect(track("--row-chat")).toBe("236px");
  });
});

describe("grid reflow — several detached", () => {
  it("collapses every detached track and leaves the docked ones intact", async () => {
    panelLayout.setDetached("rooms", true);
    panelLayout.setDetached("chat", true);
    await renderMain();

    expect(workspace().getAttribute("data-detached")).toBe("rooms chat");
    expect(placeholder("rooms")).not.toBeNull();
    expect(placeholder("chat")).not.toBeNull();
    expect(live("users")).not.toBeNull();
    expect(live("room")).not.toBeNull();

    expect(track("--col-rooms")).toBe("44px");
    expect(track("--row-chat")).toBe("44px");
    expect(track("--col-users")).toBe("208px");
  });

  it("stays usable with every docked panel detached", async () => {
    for (const panel of DOCKED) {
      panelLayout.setDetached(panel, true);
    }
    await renderMain();

    for (const panel of DOCKED) {
      expect(placeholder(panel), `${panel} placeholder`).not.toBeNull();
      expect(live(panel), `${panel} not live`).toBeNull();
    }
    expect(workspace().getAttribute("data-detached")).toBe("room users rooms chat");
    expect(track("--col-rooms")).toBe("44px");
    expect(track("--col-users")).toBe("44px");
    expect(track("--row-chat")).toBe("44px");
  });
});

describe("re-attach restores the original layout", () => {
  it("brings the docked panel back and restores its track", async () => {
    panelLayout.setDetached("users", true);
    await renderMain();
    expect(live("users")).toBeNull();

    panelLayout.setDetached("users", false);
    await tick();

    expect(placeholder("users")).toBeNull();
    expect(live("users")).not.toBeNull();
    expect(workspace().getAttribute("data-detached")).toBe("");
    expect(track("--col-users")).toBe("208px");
  });
});

describe("detach control", () => {
  it("asks Rust to open the panel and collapses the slot", async () => {
    await renderMain();

    document.querySelector<HTMLButtonElement>('[data-detach="users"]')?.click();
    await vi.waitFor(() => expect(vi.mocked(api.openPanel)).toHaveBeenCalledWith("users"));
    await tick();

    expect(live("users")).toBeNull();
    expect(placeholder("users")).not.toBeNull();
  });

  it("is not rendered inside a panel window", async () => {
    isPanelWindowMock.mockReturnValue(true);
    mounted = mount(App, { target: document.body, props: { hash: "#/panel/users" } });
    await tick();

    expect(document.querySelector('.sidebar[data-panel="users"]')).not.toBeNull();
    expect(document.querySelector("[data-detach]")).toBeNull();
  });
});
