// @vitest-environment jsdom
/**
 * The detached-window bootstrap: a `panel-*` window seeds itself exactly like
 * `main` — one subscription to the single event channel, then one `refresh`
 * replay — and mounts only its own panel, never the full layout.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const unlisten = vi.fn();

vi.mock("./api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./api")>();
  return {
    ...actual,
    onEvent: vi.fn(async () => unlisten),
    refresh: vi.fn(async () => {}),
    setViewport: vi.fn(async () => {}),
    bagCollections: vi.fn(async () => []),
    bagShelves: vi.fn(async () => []),
    bagCatalog: vi.fn(async () => []),
    bagTrashList: vi.fn(async () => []),
    outfitsList: vi.fn(async () => []),
  };
});

import { tick, mount, unmount } from "svelte";

import PanelWindow from "./PanelWindow.svelte";
import * as api from "./api";
import { PANEL_IDS, type PanelId } from "./panels";
import { store } from "./store.svelte";

/* jsdom does not implement ResizeObserver; the room panel uses one. */
class FakeResizeObserver {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
}
globalThis.ResizeObserver = FakeResizeObserver as unknown as typeof ResizeObserver;

let mounted: ReturnType<typeof mount> | undefined;

async function renderPanel(panel: PanelId): Promise<void> {
  mounted = mount(PanelWindow, { target: document.body, props: { panel } });
  await tick();
  await vi.waitFor(() => expect(vi.mocked(api.refresh)).toHaveBeenCalled());
}

function teardown(): void {
  if (mounted) {
    unmount(mounted);
    mounted = undefined;
  }
  document.body.innerHTML = "";
}

beforeEach(() => {
  vi.clearAllMocks();
  store.bagLoaded = false;
  document.body.innerHTML = "";
});

afterEach(() => {
  if (mounted) {
    unmount(mounted);
    mounted = undefined;
  }
  document.body.innerHTML = "";
});

describe("detached panel window bootstrap", () => {
  it("subscribes to the one shared channel and asks for exactly one replay", async () => {
    await renderPanel("users");
    expect(vi.mocked(api.onEvent)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(api.refresh)).toHaveBeenCalledTimes(1);
  });

  it("renders only the named panel, never the main shell", async () => {
    await renderPanel("users");
    expect(document.querySelector(".sidebar")).not.toBeNull();
    expect(document.querySelector("header.topbar")).toBeNull();
    expect(document.querySelector("footer.statusbar")).toBeNull();
    expect(document.querySelector(".workspace")).toBeNull();
    expect(document.querySelector("section.chat")).toBeNull();
  });

  it("stops listening when the panel window unmounts", async () => {
    await renderPanel("users");
    teardown();
    expect(unlisten).toHaveBeenCalledTimes(1);
  });

  it("boots every panel in the set without rendering the others", async () => {
    const markers: Record<PanelId, string> = {
      room: ".viewport-wrap",
      users: ".sidebar",
      rooms: ".sidebar",
      chat: "section.chat",
      props: ".bag-dialog",
    };
    for (const panel of PANEL_IDS) {
      await renderPanel(panel);
      expect(document.querySelector(markers[panel]), `${panel} renders`).not.toBeNull();
      expect(document.querySelector("header.topbar"), `${panel} has no shell`).toBeNull();
      teardown();
    }
  });
});
