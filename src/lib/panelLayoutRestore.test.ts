/**
 * Task 28: the detach grid is seeded from the authoritative window registry.
 *
 * Startup restore reopens detached panels in Rust, and that can happen before
 * the main webview has subscribed to `palace://layout`. These tests lock the
 * one-shot seed that closes the gap, and the live listener that keeps a window
 * in step with a change made elsewhere.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";

const { listenMock } = vi.hoisted(() => ({ listenMock: vi.fn() }));

vi.mock("@tauri-apps/api/event", () => ({ listen: listenMock }));

vi.mock("./api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./api")>();
  return {
    ...actual,
    getLayoutMemory: vi.fn(async () => ({ remember: true, detached: [], path: null })),
  };
});

import * as api from "./api";
import { panelLayout } from "./panelLayout.svelte";

beforeEach(() => {
  panelLayout.reset();
  vi.clearAllMocks();
  listenMock.mockResolvedValue(() => {});
});

describe("seeding the detach grid", () => {
  it("marks exactly the panels the backend reports as detached", async () => {
    vi.mocked(api.getLayoutMemory).mockResolvedValueOnce({
      remember: true,
      detached: ["users", "chat"],
      path: "/tmp/window-layout.json",
    });
    await panelLayout.seedFromBackend();
    expect(panelLayout.detachedIds()).toEqual(["users", "chat"]);
    expect(panelLayout.isDetached("rooms")).toBe(false);
  });

  it("drops an unknown id and keeps the registry order", () => {
    panelLayout.applyDetached(["chat", "not-a-panel", "room", "chat"]);
    expect(panelLayout.detachedIds()).toEqual(["room", "chat"]);
  });

  it("stays fully docked when the backend cannot be reached", async () => {
    vi.mocked(api.getLayoutMemory).mockRejectedValueOnce(new Error("no bridge"));
    await panelLayout.seedFromBackend();
    expect(panelLayout.detachedIds()).toEqual([]);
  });
});

describe("following layout changes", () => {
  it("applies the detached list Rust broadcasts", async () => {
    let handler: ((event: { payload: unknown }) => void) | undefined;
    listenMock.mockImplementation(
      async (_name: string, callback: (event: { payload: unknown }) => void) => {
        handler = callback;
        return () => {};
      },
    );
    panelLayout.startLayoutListener();
    await vi.waitFor(() => expect(handler).toBeDefined());
    handler?.({ payload: { remember: true, detached: ["props"], path: null } });
    expect(panelLayout.detachedIds()).toEqual(["props"]);

    handler?.({ payload: { remember: true, detached: [], path: null } });
    expect(panelLayout.detachedIds()).toEqual([]);
  });

  it("stops listening when the stop function is called", async () => {
    const unlisten = vi.fn();
    listenMock.mockResolvedValue(unlisten);
    const stop = panelLayout.startLayoutListener();
    await vi.waitFor(() => expect(listenMock).toHaveBeenCalled());
    stop();
    await vi.waitFor(() => expect(unlisten).toHaveBeenCalled());
  });

  it("does not throw when there is no Tauri event bridge", async () => {
    listenMock.mockRejectedValue(new Error("no bridge"));
    expect(() => panelLayout.startLayoutListener()).not.toThrow();
    await Promise.resolve();
  });
});
