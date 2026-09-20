// @vitest-environment jsdom
/**
 * Regression baseline: the single-window page still lays out the five visible
 * regions together.
 *
 * The multi-window refactor replaces the fixed grid with detachable panel
 * hosts. This test pins the *observable* current behaviour — one page shows the
 * Rooms list, the room viewport, the chat log, the Users list and the status
 * bar — so a refactor that drops one of them fails loudly instead of shipping a
 * silently empty window.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("./api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./api")>();
  return {
    ...actual,
    // The shell subscribes and asks for a replay on mount; neither touches a
    // real backend in jsdom.
    onEvent: vi.fn(async () => () => {}),
    refresh: vi.fn(async () => {}),
    getSettings: vi.fn(async () => ({
      host: "localhost",
      port: 9998,
      username: "Guest",
      soundfont: null,
    })),
    // The viewport reports its measured size; that is the one API call the
    // single-window layout makes on mount.
    setViewport: vi.fn(async () => {}),
  };
});

import { tick } from "svelte";
import { mount, unmount } from "svelte";

import App from "./App.svelte";
import * as api from "./api";
import { store } from "./store.svelte";

/* jsdom does not implement ResizeObserver; the viewport uses one to re-report
   its size. A no-op observer keeps the mount path real without layout. */
class FakeResizeObserver {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
}
globalThis.ResizeObserver = FakeResizeObserver as unknown as typeof ResizeObserver;

let mounted: ReturnType<typeof mount> | undefined;

function resetStore(): void {
  store.status = "connecting";
  store.statusMessage = null;
  store.banner = null;
  store.rooms = [];
  store.users = [];
  store.room = null;
  store.chat = [];
  store.screen = null;
  store.avatars = null;
  store.scale = 1;
  store.native = false;
  store.roomFilter = "";
}

beforeEach(() => {
  resetStore();
  vi.clearAllMocks();
  document.body.innerHTML = "";
});

afterEach(() => {
  if (mounted) {
    unmount(mounted);
    mounted = undefined;
  }
  document.body.innerHTML = "";
});

describe("single-window App shell", () => {
  it("renders the Rooms list, viewport, chat, Users list and StatusBar on one page", async () => {
    mounted = mount(App, { target: document.body });
    await tick();

    // TopBar
    expect(document.querySelector("header.topbar")).not.toBeNull();

    // The workspace grid holds sidebars around a centre column.
    const workspace = document.querySelector(".workspace");
    expect(workspace).not.toBeNull();
    expect(workspace?.querySelectorAll(":scope > .sidebar")).toHaveLength(2);
    expect(workspace?.querySelector(".center .viewport-wrap")).not.toBeNull();

    // Room view (Viewport) with its Fit / 1:1 controls.
    const viewport = document.querySelector(".viewport-wrap");
    expect(viewport?.textContent).toContain("Fit");
    expect(viewport?.textContent).toContain("1:1");

    // Chat panel with its input.
    const chat = document.querySelector("section.chat");
    expect(chat).not.toBeNull();
    expect(chat?.querySelector('input[aria-label="Chat message"]')).not.toBeNull();

    // Rooms list and Users list are the two sidebars.
    const sidebarHeads = Array.from(document.querySelectorAll(".workspace > .sidebar .panel-head span:first-child")).map(
      (node) => node.textContent?.trim(),
    );
    expect(sidebarHeads).toContain("Rooms");
    expect(sidebarHeads).toContain("In room");

    // StatusBar footer with the signed-in identity.
    const status = document.querySelector("footer.statusbar");
    expect(status).not.toBeNull();
    expect(status?.textContent).toContain("you");
  });

  it("renders the room list rows the backend sends", async () => {
    store.apply({
      type: "rooms",
      rooms: [
        { id: 1, name: "Alpha", users: 2, flags: 0 },
        { id: 2, name: "Bravo", users: 0, flags: 0 },
      ],
    });
    mounted = mount(App, { target: document.body });
    await tick();

    const rows = Array.from(document.querySelectorAll(".workspace > .sidebar .scroll .row .name")).map(
      (node) => node.textContent?.trim(),
    );
    expect(rows).toEqual(["Alpha", "Bravo"]);
  });

  it("renders the users the backend sends into the Users sidebar", async () => {
    store.apply({
      type: "users",
      users: [
        {
          id: 1,
          name: "Ann",
          face: 0,
          color: 1,
          room_id: 1,
          x: 0,
          y: 0,
          props: [],
          away: false,
          is_self: true,
          avatar_type: 0,
          avatar_flags: 0,
          avatar_hash: null,
        },
      ],
    });
    mounted = mount(App, { target: document.body });
    await tick();

    const names = Array.from(document.querySelectorAll(".workspace > .sidebar .scroll .row .name")).map(
      (node) => node.textContent?.trim(),
    );
    expect(names).toContain("Ann");
    expect(document.body.textContent).toContain("you");
  });

  it("reports the viewport size through the expected API on mount", async () => {
    mounted = mount(App, { target: document.body });
    await tick();

    const setViewport = vi.mocked(api.setViewport);
    expect(setViewport).toHaveBeenCalled();
    const [width, height, dpr, zoom, native] = setViewport.mock.calls[0];
    expect(width).toBeGreaterThanOrEqual(1);
    expect(height).toBeGreaterThanOrEqual(1);
    expect(dpr).toBeGreaterThan(0);
    expect(zoom).toBe(1);
    expect(native).toBe(false);
  });

  it("requests a state replay on mount so a fresh window is not empty", async () => {
    mounted = mount(App, { target: document.body });
    await tick();

    expect(vi.mocked(api.onEvent)).toHaveBeenCalledTimes(1);
    // `refresh()` runs after the async event subscription resolves.
    await vi.waitFor(() => expect(vi.mocked(api.refresh)).toHaveBeenCalledTimes(1));
  });
});
