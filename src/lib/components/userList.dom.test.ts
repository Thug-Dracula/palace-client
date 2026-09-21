// @vitest-environment jsdom
/**
 * Task 13: the Users list as a self-sufficient view.
 *
 * Users is the pattern-proving panel for the five detachable panels. It has to
 * render identically docked and detached: one component, one shared store, one
 * `palace://event` channel. These tests lock the data flow that makes that true
 * — a `users` event updates the rendered roster with no local copy that can
 * drift, the empty state names the connection state, the detach control asks
 * Rust for exactly the `users` window, and the real detached-window bootstrap
 * (PanelWindow -> startSession) seeds once and then feeds the list live.
 *
 * Seeding is a per-window responsibility (WINDOWS-ARCHITECTURE.md section 1:
 * every webview subscribes and calls `refresh` on mount). The detached case is
 * therefore driven through the real window component rather than re-implemented
 * here; the component itself is a pure view over the shared store.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const { capture, unlisten } = vi.hoisted(() => ({
  capture: { deliver: undefined as ((event: unknown) => void) | undefined },
  unlisten: vi.fn(),
}));

vi.mock("../api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api")>();
  return {
    ...actual,
    onEvent: vi.fn(async (handler: (event: unknown) => void) => {
      capture.deliver = handler;
      return unlisten;
    }),
    refresh: vi.fn(async () => {}),
    openPanel: vi.fn(async () => "created"),
    closePanel: vi.fn(async () => "closed"),
  };
});

import { tick, mount, unmount } from "svelte";

import PanelWindow from "../PanelWindow.svelte";
import * as api from "../api";
import type { UserInfo } from "../api";
import { panelLayout } from "../panelLayout.svelte";
import { store } from "../store.svelte";
import UserList from "./UserList.svelte";

let mounted: ReturnType<typeof mount> | undefined;

/** One roster entry, with the fields this panel reads made explicit. */
function user(id: number, name: string, opts: { self?: boolean; away?: boolean } = {}): UserInfo {
  return {
    id,
    name,
    face: 0,
    color: id,
    room_id: 1,
    x: 0,
    y: 0,
    props: [],
    away: opts.away ?? false,
    is_self: opts.self ?? false,
    avatar_type: 0,
    avatar_flags: 0,
    avatar_hash: null,
  };
}

/** The panel as it appears docked in the main shell. */
async function renderDocked(): Promise<void> {
  mounted = mount(UserList, { target: document.body });
  await tick();
}

/** The panel as it appears when it is the only thing in its own OS window. */
async function renderOwnWindow(): Promise<void> {
  window.location.hash = "#/panel/users";
  mounted = mount(PanelWindow, { target: document.body, props: { panel: "users" } });
  await tick();
  await vi.waitFor(() => expect(vi.mocked(api.refresh)).toHaveBeenCalled());
}

function names(): string[] {
  return [...document.querySelectorAll(".sidebar .row .name")].map(
    (node) => node.textContent ?? "",
  );
}

function emptyNote(): string | null {
  return document.querySelector(".sidebar .empty-note")?.textContent?.trim() ?? null;
}

beforeEach(() => {
  panelLayout.reset();
  vi.clearAllMocks();
  capture.deliver = undefined;
  window.history.replaceState(null, "", "/");
  store.status = "connecting";
  store.users = [];
  document.body.innerHTML = "";
});

afterEach(() => {
  if (mounted) {
    unmount(mounted);
    mounted = undefined;
  }
  window.history.replaceState(null, "", "/");
  document.body.innerHTML = "";
});

describe("Users panel — docked in the main shell", () => {
  it("renders the shared roster: names, count, self and away marks", async () => {
    store.status = "connected";
    store.apply({
      type: "users",
      users: [user(1, "Ada"), user(2, "Grace", { self: true, away: true })],
    });
    await renderDocked();

    expect(names()).toEqual(["Ada", "Grace"]);
    expect(document.querySelector(".panel-head .count")?.textContent).toBe("2");

    const rows = document.querySelectorAll(".sidebar .row");
    expect(rows[1]?.classList.contains("self")).toBe(true);
    expect(rows[1]?.querySelector(".you-tag")?.textContent).toBe("you");
    expect(rows[1]?.classList.contains("away")).toBe(true);
    expect(rows[1]?.querySelector(".meta")?.textContent).toBe("away");
  });

  it("follows the store live and never keeps a stale copy of the roster", async () => {
    // Mounted first and empty: the normal detached-window order, where the view
    // exists before the replay lands.
    await renderDocked();
    expect(names()).toEqual([]);

    store.apply({ type: "users", users: [user(1, "Ada")] });
    await tick();
    expect(names()).toEqual(["Ada"]);

    store.apply({ type: "users", users: [user(2, "Hopper"), user(3, "Kay")] });
    await tick();
    expect(names()).toEqual(["Hopper", "Kay"]);
    expect(document.querySelector(".panel-head .count")?.textContent).toBe("2");
  });

  it("names the connection state when the roster is empty", async () => {
    store.status = "disconnected";
    await renderDocked();
    expect(emptyNote()).toBe("Not connected.");

    store.status = "connected";
    await tick();
    expect(emptyNote()).toBe("Nobody here.");

    store.apply({ type: "users", users: [user(1, "Ada")] });
    await tick();
    expect(emptyNote()).toBeNull();
  });

  it("asks Rust for the users panel when the detach control is used", async () => {
    await renderDocked();

    const control = document.querySelector<HTMLButtonElement>('[data-detach="users"]');
    expect(control).not.toBeNull();
    control?.click();

    await vi.waitFor(() => expect(vi.mocked(api.openPanel)).toHaveBeenCalledWith("users"));
    expect(vi.mocked(api.openPanel)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(api.closePanel)).not.toHaveBeenCalled();
  });

  it("is a pure view: it opens no event subscription of its own", async () => {
    // The window seeds itself (App for main, PanelWindow for a panel window). A
    // second listener here would double-apply every event — duplicate chat
    // lines included — so the panel deliberately binds only to the store.
    await renderDocked();
    expect(vi.mocked(api.onEvent)).not.toHaveBeenCalled();
    expect(vi.mocked(api.refresh)).not.toHaveBeenCalled();
  });
});

describe("Users panel — its own OS window (detached)", () => {
  it("seeds once on mount, then renders users delivered by the event channel", async () => {
    store.status = "connected";
    await renderOwnWindow();

    expect(vi.mocked(api.onEvent)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(api.refresh)).toHaveBeenCalledTimes(1);

    capture.deliver?.({ type: "users", users: [user(1, "Ada"), user(2, "Grace")] });
    await tick();

    expect(names()).toEqual(["Ada", "Grace"]);
    expect(document.querySelector(".panel-head .count")?.textContent).toBe("2");
  });

  it("renders only the Users panel and offers no second detach control", async () => {
    await renderOwnWindow();

    expect(document.querySelector('.sidebar[data-panel="users"]')).not.toBeNull();
    expect(document.querySelector("header.topbar")).toBeNull();
    expect(document.querySelector("footer.statusbar")).toBeNull();
    expect(document.querySelector(".workspace")).toBeNull();
    // The way back from a detached panel is closing its window, not a button.
    expect(document.querySelector("[data-detach]")).toBeNull();
  });

  it("shows a clean empty state when the app is disconnected", async () => {
    store.status = "disconnected";
    await renderOwnWindow();

    capture.deliver?.({ type: "status", status: "disconnected", message: null });
    await tick();

    expect(emptyNote()).toBe("Not connected.");
    expect(names()).toEqual([]);
  });

  it("stops listening when the panel window unmounts", async () => {
    await renderOwnWindow();

    unmount(mounted!);
    mounted = undefined;

    expect(unlisten).toHaveBeenCalledTimes(1);
  });
});
