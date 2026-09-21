// @vitest-environment jsdom
/**
 * Task 14 acceptance: the Rooms list panel is a peer view.
 *
 * Everything the spec asks for is asserted here:
 *
 * * the docked header's detach control asks Rust for `panel-rooms` and the dock
 *   slot becomes the re-attach placeholder;
 * * a `panel-rooms` window seeds itself once against the shared channel and
 *   renders only the Rooms list, updating live from `rooms` events;
 * * re-attaching from the placeholder restores exactly one docked list;
 * * a titlebar close re-docks exactly once and never opens a duplicate window;
 * * the filter stays per-window view state, and a filter that matches nothing
 *   says so instead of leaving a blank, unlabelled list.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const { isPanelWindowMock, listenMock, closeHandlers } = vi.hoisted(() => ({
  isPanelWindowMock: vi.fn(() => false),
  listenMock: vi.fn(
    async (_event: string, _handler: (event: { payload: unknown }) => void) => () => {},
  ),
  closeHandlers: [] as Array<(event: { payload: unknown }) => void>,
}));

vi.mock("@tauri-apps/api/event", () => ({ listen: listenMock }));
vi.mock("./windowRole", () => ({ isPanelWindow: isPanelWindowMock }));
vi.mock("./api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./api")>();
  return {
    ...actual,
    // The shell and a panel window both seed through these two.
    onEvent: vi.fn(async (_handler: (event: unknown) => void) => () => {}),
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
    gotoRoom: vi.fn(async () => {}),
  };
});

import { tick, mount, unmount } from "svelte";

import App from "./App.svelte";
import PanelWindow from "./PanelWindow.svelte";
import * as api from "./api";
import type { ClientEvent } from "./api";
import { panelLayout } from "./panelLayout.svelte";
import { store } from "./store.svelte";

/* jsdom does not implement ResizeObserver; the shell's room view uses one. */
class FakeResizeObserver {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
}
globalThis.ResizeObserver = FakeResizeObserver as unknown as typeof ResizeObserver;

let mounted: ReturnType<typeof mount> | undefined;

const live = (panel: string): Element | null =>
  document.querySelector(`[data-panel="${panel}"]:not(.panel-placeholder)`);
const placeholder = (panel: string): Element | null =>
  document.querySelector(`.panel-placeholder[data-panel="${panel}"]`);
const rowNames = (): string[] =>
  Array.from(document.querySelectorAll(".sidebar .scroll .row .name")).map(
    (node) => node.textContent?.trim() ?? "",
  );
const count = (): string => document.querySelector(".panel-head .count")?.textContent?.trim() ?? "";

function resetStore(): void {
  store.status = "connecting";
  store.statusMessage = null;
  store.rooms = [];
  store.users = [];
  store.room = null;
  store.roomFilter = "";
}

async function renderMain(): Promise<void> {
  mounted = mount(App, { target: document.body });
  await tick();
  await tick();
}

/** The detach button's Rust round-trip, as Rust answers it in production. */
async function detachRooms(): Promise<void> {
  document.querySelector<HTMLButtonElement>('[data-detach="rooms"]')?.click();
  await vi.waitFor(() => expect(vi.mocked(api.openPanel)).toHaveBeenCalledWith("rooms"));
  await tick();
}

/** Rust tells `main` a panel window closed; dock that panel again. */
async function waitForCloseListener(): Promise<void> {
  await vi.waitFor(() => expect(closeHandlers.length).toBe(1));
}

function sendPanelClosed(payload: { label: string; panel: string | null }): void {
  for (const handler of [...closeHandlers]) {
    handler({ payload });
  }
}

/** The most recent `palace://event` handler a window registered. */
function channel(): (event: ClientEvent) => void {
  const handler = vi.mocked(api.onEvent).mock.calls.at(-1)?.[0] as
    | ((event: ClientEvent) => void)
    | undefined;
  if (!handler) {
    throw new Error("no palace://event handler was registered");
  }
  return handler;
}

async function renderPanelRooms(): Promise<void> {
  isPanelWindowMock.mockReturnValue(true);
  mounted = mount(PanelWindow, { target: document.body, props: { panel: "rooms" } });
  await tick();
  await vi.waitFor(() => expect(vi.mocked(api.refresh)).toHaveBeenCalled());
}

function type(value: string): void {
  const input = document.querySelector<HTMLInputElement>(".filter input");
  if (!input) {
    throw new Error("no filter input");
  }
  input.value = value;
  input.dispatchEvent(new Event("input", { bubbles: true }));
}

beforeEach(() => {
  panelLayout.reset();
  closeHandlers.length = 0;
  listenMock.mockImplementation(
    async (event: string, handler: (event: { payload: unknown }) => void) => {
      if (event === "palace://panel-closed") {
        closeHandlers.push(handler);
      }
      return () => {};
    },
  );
  vi.clearAllMocks();
  isPanelWindowMock.mockReturnValue(false);
  resetStore();
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

describe("docked Rooms panel detach control", () => {
  it("asks Rust for panel-rooms and swaps the list for the re-attach placeholder", async () => {
    await renderMain();
    expect(live("rooms")).not.toBeNull();

    await detachRooms();

    expect(vi.mocked(api.openPanel)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(api.openPanel)).toHaveBeenCalledWith("rooms");
    expect(live("rooms")).toBeNull();
    expect(placeholder("rooms")).not.toBeNull();
    expect(placeholder("rooms")?.querySelector("button")?.textContent?.trim()).toBe("Reattach");
  });
});

describe("a detached panel-rooms window", () => {
  it("seeds itself once and renders only the Rooms list", async () => {
    await renderPanelRooms();

    expect(vi.mocked(api.onEvent)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(api.refresh)).toHaveBeenCalledTimes(1);
    expect(document.querySelector('.sidebar[data-panel="rooms"]')).not.toBeNull();
    // No shell, no other panel, and no detach control inside a panel window.
    expect(document.querySelector("header.topbar")).toBeNull();
    expect(document.querySelector(".workspace")).toBeNull();
    expect(document.querySelector('[data-detach="rooms"]')).toBeNull();
    expect(document.querySelector(".viewport-wrap")).toBeNull();
    expect(document.querySelector("section.chat")).toBeNull();
  });

  it("updates live from rooms events and replaces rather than appends", async () => {
    await renderPanelRooms();
    const emit = channel();

    emit({
      type: "rooms",
      rooms: [
        { id: 2, name: "Bravo", users: 0, flags: 0 },
        { id: 1, name: "Alpha", users: 3, flags: 0 },
      ],
    });
    await tick();
    expect(rowNames()).toEqual(["Alpha", "Bravo"]);
    expect(count()).toBe("2/2");

    emit({ type: "rooms", rooms: [{ id: 9, name: "Charlie", users: 1, flags: 0 }] });
    await tick();
    expect(rowNames()).toEqual(["Charlie"]);
    expect(count()).toBe("1/1");
  });

  it("filters locally, keeps the filter per-window, and labels the no-match state", async () => {
    await renderPanelRooms();
    const emit = channel();
    emit({
      type: "rooms",
      rooms: [
        { id: 1, name: "Alpha", users: 3, flags: 0 },
        { id: 2, name: "Bravo", users: 0, flags: 0 },
        { id: 3, name: "Charlie", users: 1, flags: 0 },
      ],
    });
    await tick();

    type("br");
    await tick();
    expect(rowNames()).toEqual(["Bravo"]);
    expect(count()).toBe("1/3");

    type("zzz");
    await tick();
    expect(rowNames()).toEqual([]);
    const note = document.querySelector(".empty-note")?.textContent ?? "";
    expect(note).toContain("No rooms match");
    expect(note).toContain("zzz");

    type("");
    await tick();
    expect(rowNames()).toEqual(["Alpha", "Bravo", "Charlie"]);
  });

  it("shows a clean disconnected state and refuses to enter rooms", async () => {
    await renderPanelRooms();
    const emit = channel();
    emit({ type: "rooms", rooms: [{ id: 1, name: "Alpha", users: 1, flags: 0 }] });
    emit({ type: "status", status: "disconnected", message: null });
    await tick();

    const row = document.querySelector<HTMLButtonElement>(".sidebar .scroll .row");
    expect(row?.disabled).toBe(true);
    row?.click();
    expect(vi.mocked(api.gotoRoom)).not.toHaveBeenCalled();

    emit({ type: "rooms", rooms: [] });
    await tick();
    expect(document.querySelector(".empty-note")?.textContent).toContain("Not connected");
  });
});

describe("re-attaching the Rooms panel", () => {
  it("restores exactly one docked list after the re-attach round-trip", async () => {
    await renderMain();
    await waitForCloseListener();
    await detachRooms();

    document
      .querySelector<HTMLButtonElement>('.panel-placeholder[data-panel="rooms"] button')
      ?.click();
    await vi.waitFor(() => expect(vi.mocked(api.closePanel)).toHaveBeenCalledWith("rooms"));

    // Rust intercepts the close and signals `main`; that is what re-docks it.
    sendPanelClosed({ label: "panel-rooms", panel: "rooms" });
    await tick();

    expect(placeholder("rooms")).toBeNull();
    expect(document.querySelectorAll('.sidebar[data-panel="rooms"]')).toHaveLength(1);
    expect(vi.mocked(api.openPanel)).toHaveBeenCalledTimes(1);
  });

  it("re-docks exactly once on a titlebar close and never opens a duplicate window", async () => {
    await renderMain();
    await waitForCloseListener();
    panelLayout.setDetached("rooms", true);
    await tick();
    expect(placeholder("rooms")).not.toBeNull();

    sendPanelClosed({ label: "panel-rooms", panel: "rooms" });
    await tick();

    expect(placeholder("rooms")).toBeNull();
    expect(document.querySelectorAll('.sidebar[data-panel="rooms"]')).toHaveLength(1);
    expect(vi.mocked(api.openPanel)).not.toHaveBeenCalled();

    // A duplicate close signal (or a stray one after re-attach) must be inert.
    sendPanelClosed({ label: "panel-rooms", panel: "rooms" });
    await tick();
    expect(document.querySelectorAll('.sidebar[data-panel="rooms"]')).toHaveLength(1);
    expect(placeholder("rooms")).toBeNull();
    expect(vi.mocked(api.openPanel)).not.toHaveBeenCalled();
  });

  it("ignores a close signal from a window that is not a panel", async () => {
    await renderMain();
    await waitForCloseListener();
    panelLayout.setDetached("rooms", true);
    await tick();

    sendPanelClosed({ label: "panel-spike", panel: null });
    await tick();
    expect(placeholder("rooms")).not.toBeNull();

    sendPanelClosed({ label: "main", panel: null });
    await tick();
    expect(placeholder("rooms")).not.toBeNull();
  });
});
