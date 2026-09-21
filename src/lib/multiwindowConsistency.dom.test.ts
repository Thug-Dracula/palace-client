// @vitest-environment jsdom
/**
 * Task 28 acceptance: several windows, one session.
 *
 * Each OS window is a webview with its own module instance, so this test gives
 * every "window" a *fresh* import of the store and the session helper. That is
 * what makes the assertions real: five independent stores, five independent
 * subscriptions to the one `palace://event` channel, and one broadcast applied
 * to all of them. The low-level Tauri primitives are mocked, so no window ever
 * reaches a backend, and every command a window issues is recorded.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";

const h = vi.hoisted(() => ({
  sessionHandlers: [] as Array<(event: { payload: unknown }) => void>,
  prefsHandlers: [] as Array<(event: { payload: unknown }) => void>,
  invoked: [] as string[],
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (name: string, handler: (event: { payload: unknown }) => void) => {
    if (name === "palace://event") {
      h.sessionHandlers.push(handler);
    } else if (name === "palace://prefs") {
      h.prefsHandlers.push(handler);
    }
    return () => {};
  }),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async (cmd: string) => {
    h.invoked.push(cmd);
    if (cmd === "get_prefs") {
      return {};
    }
    if (cmd === "get_layout_memory") {
      return { remember: true, detached: [], path: null };
    }
    return undefined;
  }),
}));

type Store = (typeof import("./store.svelte"))["store"];
type Session = ReturnType<(typeof import("./session"))["startSession"]>;
interface Window {
  store: Store;
  session: Session;
}

/** The five detached panels, one independent webview each. */
const PANELS = ["room", "users", "rooms", "chat", "props"] as const;

async function openWindow(): Promise<Window> {
  vi.resetModules();
  const storeModule = await import("./store.svelte");
  const sessionModule = await import("./session");
  const session = sessionModule.startSession();
  await vi.waitFor(() =>
    expect(h.sessionHandlers.length, "the window subscribes to the one channel").toBeGreaterThan(0),
  );
  return { store: storeModule.store, session };
}

async function openAll(): Promise<Window[]> {
  const windows: Window[] = [];
  for (const _panel of PANELS) {
    windows.push(await openWindow());
  }
  return windows;
}

/** Deliver one backend event to every subscribed window, as a broadcast does. */
function broadcast(event: unknown): void {
  for (const handler of [...h.sessionHandlers]) {
    handler({ payload: event });
  }
}

function user(id: number, name: string) {
  return {
    id,
    name,
    face: 0,
    color: 1,
    room_id: 1,
    x: 0,
    y: 0,
    props: [],
    away: false,
    is_self: id === 1,
    avatar_type: 0,
    avatar_flags: 0,
    avatar_hash: null,
  };
}

/** Settle each window's one-time mute seed so later command counts are clean. */
async function settle(windows: Window[]): Promise<void> {
  broadcast({ type: "status", status: "connecting", message: null });
  await vi.waitFor(() => {
    for (const window of windows) {
      expect(window.store.status).toBe("connecting");
    }
  });
  await new Promise((resolve) => setTimeout(resolve, 0));
}

beforeEach(() => {
  h.sessionHandlers.length = 0;
  h.prefsHandlers.length = 0;
  h.invoked.length = 0;
});

describe("several windows, one session", () => {
  it("subscribes each detached window once and replays once", async () => {
    const windows = await openAll();
    expect(h.sessionHandlers).toHaveLength(PANELS.length);
    await vi.waitFor(() =>
      expect(h.invoked.filter((cmd) => cmd === "refresh")).toHaveLength(PANELS.length),
    );
    expect(
      h.invoked.filter((cmd) => cmd === "refresh"),
      "no window asks for a second replay",
    ).toHaveLength(PANELS.length);
    for (const window of windows) {
      window.session.stop();
    }
  });

  it("applies one broadcast to all five windows exactly once and issues no command", async () => {
    const windows = await openAll();
    await settle(windows);
    h.invoked.length = 0;

    broadcast({ type: "users", users: [user(1, "Ada"), user(2, "Bo")] });

    for (const window of windows) {
      expect(window.store.users.map((entry) => entry.name)).toEqual(["Ada", "Bo"]);
    }
    expect(h.invoked, "receiving an event must not issue a command").toEqual([]);
    for (const window of windows) {
      window.session.stop();
    }
  });

  it("refreshes every window on reconnect, with no stale state left behind", async () => {
    const windows = await openAll();
    await settle(windows);

    broadcast({ type: "status", status: "connected", message: null });
    broadcast({ type: "users", users: [user(1, "Ada")] });
    broadcast({ type: "rooms", rooms: [{ id: 1, name: "Hall", users: 1, flags: 0 }] });
    for (const window of windows) {
      expect(window.store.connected).toBe(true);
      expect(window.store.users).toHaveLength(1);
      expect(window.store.rooms.map((room) => room.name)).toEqual(["Hall"]);
    }

    broadcast({ type: "status", status: "disconnected", message: "lost" });
    for (const window of windows) {
      expect(window.store.connected).toBe(false);
      expect(window.store.avatars).toBeNull();
    }

    broadcast({ type: "status", status: "connected", message: null });
    broadcast({ type: "users", users: [user(1, "Ada"), user(3, "Cy")] });
    broadcast({ type: "rooms", rooms: [{ id: 2, name: "New Hall", users: 2, flags: 0 }] });
    for (const window of windows) {
      expect(window.store.connected).toBe(true);
      expect(window.store.users.map((entry) => entry.name)).toEqual(["Ada", "Cy"]);
      expect(window.store.rooms.map((room) => room.name)).toEqual(["New Hall"]);
    }
    for (const window of windows) {
      window.session.stop();
    }
  });

  it("issues no command while a reconnect replays to every window", async () => {
    const windows = await openAll();
    await settle(windows);
    h.invoked.length = 0;

    broadcast({ type: "status", status: "disconnected", message: "lost" });
    broadcast({ type: "status", status: "connected", message: null });
    broadcast({ type: "users", users: [user(1, "Ada")] });
    broadcast({ type: "rooms", rooms: [{ id: 1, name: "Hall", users: 1, flags: 0 }] });
    broadcast({ type: "chat", line: { seq: 1, user_id: 1, name: "Ada", text: "hi", kind: "talk" } });

    for (const window of windows) {
      expect(window.store.connected).toBe(true);
      expect(window.store.users.map((entry) => entry.name)).toEqual(["Ada"]);
    }
    expect(
      h.invoked,
      "a reconnect replay must not make any window issue a command",
    ).toEqual([]);
    for (const window of windows) {
      window.session.stop();
    }
  });

  it("keeps one session per window when startSession is called twice", async () => {
    vi.resetModules();
    const storeModule = await import("./store.svelte");
    const sessionModule = await import("./session");
    const first = sessionModule.startSession();
    const second = sessionModule.startSession();
    await vi.waitFor(() =>
      expect(h.sessionHandlers.length, "the window subscribes once").toBeGreaterThan(0),
    );
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(
      h.sessionHandlers,
      "a second startSession in one window must not open a second listener",
    ).toHaveLength(1);
    expect(
      h.invoked.filter((cmd) => cmd === "refresh"),
      "a second startSession must not issue a second seed",
    ).toHaveLength(1);
    expect(second).toBe(first);
    first.stop();
    void storeModule;
  });

  it("does not let any window play a sound; the pump is the only audio owner", async () => {
    const windows = await openAll();
    await settle(windows);
    const before = windows.map((window) => window.store.users.length);
    h.invoked.length = 0;

    broadcast({ type: "sound", name: "door.mp3" });

    windows.forEach((window, index) => {
      expect(window.store.users.length).toBe(before[index]);
    });
    expect(h.invoked, "a sound event must not make a webview play or fetch anything").toEqual([]);
    for (const window of windows) {
      window.session.stop();
    }
  });
});
