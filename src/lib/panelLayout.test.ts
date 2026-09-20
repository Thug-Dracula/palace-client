/**
 * The detach state holder: the reactive fact the shell's grid is built from,
 * plus the Rust calls that flip it. A panel is only ever marked detached once
 * Rust has agreed (or rolled straight back if it refused), so the grid never
 * claims a window that does not exist.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";

const { listenMock } = vi.hoisted(() => ({ listenMock: vi.fn() }));

vi.mock("@tauri-apps/api/event", () => ({ listen: listenMock }));

vi.mock("./api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./api")>();
  return {
    ...actual,
    openPanel: vi.fn(async () => "created"),
    closePanel: vi.fn(async () => "closed"),
  };
});

import * as api from "./api";
import { panelLayout } from "./panelLayout.svelte";

beforeEach(() => {
  panelLayout.reset();
  vi.clearAllMocks();
  listenMock.mockResolvedValue(() => {});
});

describe("detach state", () => {
  it("starts fully docked", () => {
    expect(panelLayout.detachedIds()).toEqual([]);
    expect(panelLayout.isDetached("users")).toBe(false);
  });

  it("detach opens exactly one window and flips the flag", async () => {
    await panelLayout.detach("users");
    expect(vi.mocked(api.openPanel)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(api.openPanel)).toHaveBeenCalledWith("users");
    expect(panelLayout.isDetached("users")).toBe(true);
    expect(panelLayout.detachedIds()).toEqual(["users"]);
  });

  it("is idempotent: detaching an already-detached panel does not reopen it", async () => {
    await panelLayout.detach("users");
    await panelLayout.detach("users");
    expect(vi.mocked(api.openPanel)).toHaveBeenCalledTimes(1);
  });

  it("rolls the grid back when the window cannot be created", async () => {
    vi.mocked(api.openPanel).mockRejectedValueOnce(new Error("no window"));
    await expect(panelLayout.detach("chat")).rejects.toThrow("no window");
    expect(panelLayout.isDetached("chat")).toBe(false);
    expect(panelLayout.detachedIds()).toEqual([]);
  });

  it("reattach closes the window and docks the panel", async () => {
    await panelLayout.detach("rooms");
    await panelLayout.reattach("rooms");
    expect(vi.mocked(api.closePanel)).toHaveBeenCalledWith("rooms");
    expect(panelLayout.isDetached("rooms")).toBe(false);
  });

  it("reattach docks the panel even when the close call fails", async () => {
    await panelLayout.detach("room");
    vi.mocked(api.closePanel).mockRejectedValueOnce(new Error("already gone"));
    await panelLayout.reattach("room");
    expect(panelLayout.isDetached("room")).toBe(false);
  });

  it("applyPanelClosed docks the panel from its label or its id", () => {
    panelLayout.setDetached("props", true);
    panelLayout.applyPanelClosed({ label: "panel-props", panel: "props" });
    expect(panelLayout.isDetached("props")).toBe(false);

    panelLayout.setDetached("users", true);
    panelLayout.applyPanelClosed({ label: "panel-users", panel: null });
    expect(panelLayout.isDetached("users")).toBe(false);
  });

  it("ignores a close for a window that is not a known panel", () => {
    panelLayout.setDetached("chat", true);
    panelLayout.applyPanelClosed({ label: "panel-spike", panel: null });
    expect(panelLayout.isDetached("chat")).toBe(true);
  });

  it("reports detached ids in the fixed panel order", () => {
    panelLayout.setDetached("chat", true);
    panelLayout.setDetached("room", true);
    expect(panelLayout.detachedIds()).toEqual(["room", "chat"]);
  });
});

describe("close listener", () => {
  it("docks the panel when Rust reports its window closed", async () => {
    let handler: ((event: { payload: unknown }) => void) | undefined;
    listenMock.mockImplementation(
      async (_name: string, callback: (event: { payload: unknown }) => void) => {
        handler = callback;
        return () => {};
      },
    );
    panelLayout.setDetached("users", true);
    panelLayout.startCloseListener();
    await vi.waitFor(() => expect(handler).toBeDefined());
    handler?.({ payload: { label: "panel-users", panel: "users" } });
    expect(panelLayout.isDetached("users")).toBe(false);
  });

  it("stops listening when the stop function is called", async () => {
    const unlisten = vi.fn();
    listenMock.mockResolvedValue(unlisten);
    const stop = panelLayout.startCloseListener();
    await vi.waitFor(() => expect(listenMock).toHaveBeenCalled());
    stop();
    await vi.waitFor(() => expect(unlisten).toHaveBeenCalled());
  });

  it("does not throw when there is no Tauri event bridge", async () => {
    listenMock.mockRejectedValue(new Error("no bridge"));
    expect(() => panelLayout.startCloseListener()).not.toThrow();
    // The rejection is swallowed, so an unhandled rejection would fail the run.
    await Promise.resolve();
  });
});
