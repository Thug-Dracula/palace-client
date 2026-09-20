// @vitest-environment jsdom
/**
 * App is the per-window router: the full shell for `main`, one panel for a
 * `#/panel/<id>` hash. Unknown hashes fall back to the full layout, which is
 * also what the SPA asset fallback serves.
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
import * as api from "./api";
import { store } from "./store.svelte";

class FakeResizeObserver {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
}
globalThis.ResizeObserver = FakeResizeObserver as unknown as typeof ResizeObserver;

let mounted: ReturnType<typeof mount> | undefined;

async function renderApp(hash?: string): Promise<void> {
  mounted = mount(App, { target: document.body, props: hash === undefined ? {} : { hash } });
  await tick();
}

beforeEach(() => {
  vi.clearAllMocks();
  store.rooms = [];
  store.users = [];
  store.chat = [];
  store.status = "connecting";
  document.body.innerHTML = "";
});

afterEach(() => {
  if (mounted) {
    unmount(mounted);
    mounted = undefined;
  }
  document.body.innerHTML = "";
});

describe("per-window routing", () => {
  it("mounts the full layout for the default (empty) hash", async () => {
    await renderApp();
    expect(document.querySelector(".app")).not.toBeNull();
    expect(document.querySelector(".workspace")).not.toBeNull();
    expect(document.querySelector("header.topbar")).not.toBeNull();
  });

  it("mounts only the named panel for a #/panel/<id> hash", async () => {
    await renderApp("#/panel/chat");
    expect(document.querySelector("section.chat")).not.toBeNull();
    expect(document.querySelector(".app")).toBeNull();
    expect(document.querySelector(".workspace")).toBeNull();
    expect(document.querySelector("header.topbar")).toBeNull();
    expect(document.querySelector("footer.statusbar")).toBeNull();
  });

  it("seeds the detached window like main: one subscribe, one replay", async () => {
    await renderApp("#/panel/users");
    expect(vi.mocked(api.onEvent)).toHaveBeenCalledTimes(1);
    await vi.waitFor(() => expect(vi.mocked(api.refresh)).toHaveBeenCalledTimes(1));
  });

  it("falls back to the full layout for an unknown panel hash", async () => {
    await renderApp("#/panel/editor");
    expect(document.querySelector(".app")).not.toBeNull();
  });
});
