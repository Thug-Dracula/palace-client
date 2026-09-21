// @vitest-environment jsdom
/**
 * The saved graphics choices must reach the backend when a window starts.
 *
 * Name/avatar visibility is process-global backend state, so without a seed at
 * session start a launch would show names until someone opened the Preferences
 * group — the setting would look like it had been forgotten. These tests lock
 * the seed and, equally important, that it does not re-send a value the store
 * already holds.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";

const { state } = vi.hoisted(() => ({
  state: { stored: {} as Record<string, unknown> },
}));

vi.mock("./api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./api")>();
  return {
    ...actual,
    onEvent: vi.fn(async () => () => {}),
    refresh: vi.fn(async () => {}),
    getPrefs: vi.fn(async () => state.stored),
    setVisibility: vi.fn(async () => {}),
  };
});

import * as api from "./api";
import { startSession } from "./session";
import { store } from "./store.svelte";

beforeEach(() => {
  vi.clearAllMocks();
  state.stored = {};
  store.showNames = true;
  store.showAvatars = true;
  store.showGuests = true;
});

describe("graphics seed at session start", () => {
  it("applies a saved hide-names choice and tells the backend", async () => {
    state.stored = { graphics: { show_names: false } };

    const session = startSession();
    await vi.waitFor(() => expect(store.showNames).toBe(false));

    expect(api.setVisibility).toHaveBeenCalledWith(false, true);
    session.stop();
  });

  it("seeds the guest filter without touching the backend", async () => {
    state.stored = { graphics: { show_guests: false } };

    const session = startSession();
    await vi.waitFor(() => expect(store.showGuests).toBe(false));

    expect(store.showNames).toBe(true);
    expect(api.setVisibility).not.toHaveBeenCalled();
    session.stop();
  });

  it("does not re-send a value the store already holds", async () => {
    state.stored = {};

    const session = startSession();
    await vi.waitFor(() => expect(api.getPrefs).toHaveBeenCalled());

    expect(store.showNames).toBe(true);
    expect(store.showAvatars).toBe(true);
    expect(api.setVisibility).not.toHaveBeenCalled();
    session.stop();
  });
});
