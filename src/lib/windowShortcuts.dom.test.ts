// @vitest-environment jsdom
/**
 * Task 28: keyboard ownership is per window, not global.
 *
 * A keypress reaches only the webview that has focus, and each window binds
 * only the shortcuts it owns. The main window owns the zoom keys because they
 * resize it through `set_ui_scale`; a detached panel binds none of them. There
 * is no global input hook in Rust, by design. This test mounts both a main and
 * a panel window in one document and proves one keypress issues exactly one
 * command, and a panel alone issues none.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

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
    getLayoutMemory: vi.fn(async () => ({ remember: true, detached: [], path: null })),
    setViewport: vi.fn(async () => {}),
    setUiScale: vi.fn(async (scale: number) => scale),
    bagCollections: vi.fn(async () => []),
    bagShelves: vi.fn(async () => []),
    bagCatalog: vi.fn(async () => []),
    bagTrashList: vi.fn(async () => []),
    outfitsList: vi.fn(async () => []),
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

const mounted: Array<ReturnType<typeof mount>> = [];

async function render(hash?: string): Promise<void> {
  mounted.push(mount(App, { target: document.body, props: hash === undefined ? {} : { hash } }));
  await tick();
}

function press(key: string): void {
  window.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));
}

beforeEach(() => {
  panelLayout.reset();
  store.scale = 1;
  store.native = false;
  vi.clearAllMocks();
  document.body.innerHTML = "";
});

afterEach(() => {
  while (mounted.length > 0) {
    const component = mounted.pop();
    if (component) {
      unmount(component);
    }
  }
  document.body.innerHTML = "";
});

describe("focused-window shortcut ownership", () => {
  it("fires a zoom shortcut in exactly one window when main and a panel are open", async () => {
    await render();
    await render("#/panel/users");
    await vi.waitFor(() => expect(api.getLayoutMemory).toHaveBeenCalled());
    vi.mocked(api.setUiScale).mockClear();

    press("+");

    expect(
      vi.mocked(api.setUiScale),
      "one keypress must issue exactly one zoom command",
    ).toHaveBeenCalledTimes(1);
  });

  it("binds no zoom shortcut in a detached panel window", async () => {
    await render("#/panel/users");
    vi.mocked(api.setUiScale).mockClear();

    press("+");

    expect(vi.mocked(api.setUiScale)).not.toHaveBeenCalled();
  });
});
