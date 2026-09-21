// @vitest-environment jsdom
/**
 * Task 28 regression: a panel restored into its own window must not also be
 * drawn docked in the main grid.
 *
 * Startup restore reopens detached panels in Rust before the main webview has
 * necessarily subscribed to the layout broadcast. The main shell therefore
 * seeds its detach state from the live registry on mount; without that seed it
 * renders a docked copy of a panel that is already floating, which is the bug
 * this test locks down.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const { layout } = vi.hoisted(() => ({
  layout: { detached: [] as string[] },
}));

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
    getPrefs: vi.fn(async () => ({})),
    getLayoutMemory: vi.fn(async () => ({
      remember: true,
      detached: layout.detached,
      path: null,
    })),
    setViewport: vi.fn(async () => {}),
    bagCollections: vi.fn(async () => []),
    bagShelves: vi.fn(async () => []),
    bagCatalog: vi.fn(async () => []),
    bagTrashList: vi.fn(async () => []),
    outfitsList: vi.fn(async () => []),
  };
});

import { tick, mount, unmount } from "svelte";

import App from "./App.svelte";
import { panelLayout } from "./panelLayout.svelte";
import { store } from "./store.svelte";

class FakeResizeObserver {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
}
globalThis.ResizeObserver = FakeResizeObserver as unknown as typeof ResizeObserver;

let mounted: ReturnType<typeof mount> | undefined;

beforeEach(() => {
  layout.detached = [];
  panelLayout.reset();
  store.users = [];
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

describe("restored detached panels in the main grid", () => {
  it("draws a docked panel when the registry reports nothing detached", async () => {
    mounted = mount(App, { target: document.body });
    await tick();
    await vi.waitFor(() => expect(panelLayout.detachedIds()).toEqual([]));
    expect(document.querySelector('.panel-placeholder[data-panel="users"]')).toBeNull();
    expect(document.querySelector('.sidebar[data-panel="users"]')).not.toBeNull();
  });

  it("draws the re-attach placeholder for a panel the registry says is detached", async () => {
    layout.detached = ["users"];
    mounted = mount(App, { target: document.body });
    await tick();

    await vi.waitFor(() => {
      const placeholder = document.querySelector('.panel-placeholder[data-panel="users"]');
      expect(placeholder, "the restored panel must not also be drawn docked").not.toBeNull();
      expect(placeholder?.textContent).toContain("own window");
    });
  });
});
