// @vitest-environment jsdom
/**
 * Task 17: the room viewport's ownership and reporting rules.
 *
 * The geometry a window draws is built for the size it reported, so two rules
 * keep a stale or other-window reply from corrupting the shared frame: a reply
 * is applied only when it names this window and is at least as new as the last
 * one accepted, and resize reports are coalesced so a drag cannot storm the
 * compositor. A device-pixel-ratio change re-reports, because moving a window
 * to a scaled monitor changes the physical size without a resize.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const { setViewport, onGeometry, capture } = vi.hoisted(() => {
  const capture: {
    handler?: (reply: unknown) => void;
    resize?: () => void;
  } = {};
  const setViewport = vi.fn(
    async (_width: number, _height: number, _dpr: number, _zoom: number, _native: boolean) => {},
  );
  const onGeometry = vi.fn(async (_label: string, handler: (reply: unknown) => void) => {
    capture.handler = handler;
    return () => {};
  });
  return { setViewport, onGeometry, capture };
});

vi.mock("./api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./api")>();
  return {
    ...actual,
    onEvent: vi.fn(async () => () => {}),
    onGeometry,
    setViewport,
    currentWindowLabel: () => "panel-room",
  };
});

import { tick, mount, unmount } from "svelte";

import Viewport from "./components/Viewport.svelte";
import type { ScreenState, ViewGeometry } from "./api";
import { acceptsGeometry, store } from "./store.svelte";

/* A ResizeObserver whose callback the test can fire on demand. */
class ControllableResizeObserver {
  constructor(callback: () => void) {
    capture.resize = callback;
  }
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
}
globalThis.ResizeObserver = ControllableResizeObserver as unknown as typeof ResizeObserver;

const geometry: ViewGeometry = {
  room_w: 100,
  room_h: 80,
  viewport_w: 640,
  viewport_h: 480,
  dpr: 1,
  zoom: 1,
  native: false,
  scale: 1,
  content_x: 0,
  content_y: 0,
  content_w: 100,
  content_h: 80,
  bitmap_w: 100,
  bitmap_h: 80,
};

function screen(roomName: string): ScreenState {
  return {
    version: 1,
    room_id: 1,
    room_name: roomName,
    avatars: 0,
    loose_props: 0,
    props_pending: 0,
    notes: ["one"],
    geometry,
    mid_version: null,
    top_version: null,
  };
}

let mounted: ReturnType<typeof mount> | undefined;
let rect = { width: 640, height: 480 };
let dpr = 1;
let dprChange: (() => void) | null = null;

function resetStore(): void {
  store.screen = null;
  store.notes = [];
  store.native = false;
  store.viewport = { width: 960, height: 540, dpr: 1 };
}

beforeEach(() => {
  vi.clearAllMocks();
  capture.handler = undefined;
  capture.resize = undefined;
  rect = { width: 640, height: 480 };
  dpr = 1;
  dprChange = null;
  resetStore();
  document.body.innerHTML = "";
  vi.spyOn(Element.prototype, "getBoundingClientRect").mockImplementation(
    () =>
      ({
        width: rect.width,
        height: rect.height,
        top: 0,
        left: 0,
        right: rect.width,
        bottom: rect.height,
        x: 0,
        y: 0,
        toJSON: () => ({}),
      }) as DOMRect,
  );
  Object.defineProperty(window, "devicePixelRatio", {
    configurable: true,
    get: () => dpr,
  });
  Object.defineProperty(window, "matchMedia", {
    configurable: true,
    writable: true,
    value: (query: string) =>
      ({
        media: query,
        matches: true,
        onchange: null,
        addEventListener: (_type: string, callback: () => void) => {
          dprChange = callback;
        },
        removeEventListener: () => {},
        addListener: (callback: () => void) => {
          dprChange = callback;
        },
        removeListener: () => {},
        dispatchEvent: () => false,
      }) as unknown as MediaQueryList,
  });
});

afterEach(() => {
  if (mounted) {
    unmount(mounted);
    mounted = undefined;
  }
  vi.useRealTimers();
  vi.restoreAllMocks();
  document.body.innerHTML = "";
});

describe("acceptsGeometry", () => {
  it("accepts a reply for this window at the current or a newer epoch", () => {
    expect(acceptsGeometry("panel-room", 0, { owner: "panel-room", epoch: 1 })).toBe(true);
    expect(acceptsGeometry("panel-room", 2, { owner: "panel-room", epoch: 2 })).toBe(true);
    expect(acceptsGeometry("panel-room", 2, { owner: "panel-room", epoch: 3 })).toBe(true);
  });

  it("rejects a reply for another window", () => {
    expect(acceptsGeometry("panel-room", 0, { owner: "main", epoch: 5 })).toBe(false);
    expect(acceptsGeometry("panel-room", 0, { owner: "panel-users", epoch: 5 })).toBe(false);
  });

  it("rejects a superseded reply", () => {
    expect(acceptsGeometry("panel-room", 4, { owner: "panel-room", epoch: 3 })).toBe(false);
  });
});

describe("Viewport geometry ownership", () => {
  it("subscribes with its own label and applies only its own replies", async () => {
    mounted = mount(Viewport, { target: document.body });
    await tick();

    expect(onGeometry).toHaveBeenCalledTimes(1);
    expect(onGeometry.mock.calls[0][0]).toBe("panel-room");
    expect(capture.handler).toBeDefined();

    capture.handler?.({ owner: "panel-room", epoch: 2, screen: screen("Mine") });
    expect(store.screen?.room_name).toBe("Mine");

    capture.handler?.({ owner: "main", epoch: 9, screen: screen("Foreign") });
    expect(store.screen?.room_name).toBe("Mine");

    capture.handler?.({ owner: "panel-room", epoch: 1, screen: screen("Stale") });
    expect(store.screen?.room_name).toBe("Mine");

    capture.handler?.({ owner: "panel-room", epoch: 3, screen: screen("Newer") });
    expect(store.screen?.room_name).toBe("Newer");
  });
});

describe("Viewport reporting", () => {
  it("reports its measured size immediately on mount", async () => {
    mounted = mount(Viewport, { target: document.body });
    await tick();

    expect(setViewport).toHaveBeenCalledTimes(1);
    const [width, height, ratio, zoom, native] = setViewport.mock.calls[0];
    expect(width).toBe(640);
    expect(height).toBe(480);
    expect(ratio).toBe(1);
    expect(zoom).toBe(1);
    expect(native).toBe(false);
  });

  it("debounces a resize burst into one report after it settles", async () => {
    vi.useFakeTimers();
    mounted = mount(Viewport, { target: document.body });
    await tick();
    expect(setViewport).toHaveBeenCalledTimes(1);

    rect = { width: 800, height: 600 };
    capture.resize?.();
    capture.resize?.();
    capture.resize?.();
    expect(setViewport).toHaveBeenCalledTimes(1);

    await vi.advanceTimersByTimeAsync(200);
    expect(setViewport).toHaveBeenCalledTimes(2);
    const [width, height] = setViewport.mock.calls[1];
    expect(width).toBe(800);
    expect(height).toBe(600);
  });

  it("re-reports when the device pixel ratio changes with no resize", async () => {
    mounted = mount(Viewport, { target: document.body });
    await tick();
    expect(setViewport).toHaveBeenCalledTimes(1);
    expect(setViewport.mock.calls[0][2]).toBe(1);

    dpr = 2;
    dprChange?.();
    expect(setViewport).toHaveBeenCalledTimes(2);
    const [width, height, ratio] = setViewport.mock.calls[1];
    expect(width).toBe(640);
    expect(height).toBe(480);
    expect(ratio).toBe(2);
  });
});
