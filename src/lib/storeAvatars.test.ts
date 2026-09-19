import { beforeEach, describe, expect, it } from "vitest";

import { store } from "./store.svelte";
import type { AvatarRoster, RoomInfo, ScreenState, ViewGeometry } from "./api";

const geometry: ViewGeometry = {
  room_w: 100,
  room_h: 80,
  viewport_w: 400,
  viewport_h: 320,
  dpr: 1,
  zoom: 1,
  native: true,
  scale: 2,
  content_x: 10,
  content_y: 20,
  content_w: 200,
  content_h: 160,
  bitmap_w: 100,
  bitmap_h: 80,
};

const roster = (version = 5): AvatarRoster => ({
  version,
  room_id: 1,
  geometry,
  name_tags_visible: true,
  avatars: [
    {
      id: 7,
      name: "Ann",
      x: 5,
      y: 6,
      face: 3,
      color: 4,
      is_self: false,
      away: false,
      parts: [
        { art: { kind: "face", face: 3, color: 4 }, dx: 1, dy: 2, alpha: 0.5, w: 3, h: 4 },
      ],
    },
  ],
});

const screen: ScreenState = {
  version: 11,
  room_id: 1,
  room_name: "Test",
  avatars: 1,
  loose_props: 0,
  props_pending: 0,
  notes: [],
  geometry,
  mid_version: 22,
  top_version: 33,
};

const room: RoomInfo = { id: 2, name: "Next", users: 0, flags: 0 };

beforeEach(() => {
  store.avatars = null;
  store.screen = null;
  store.room = null;
});

describe("store roster lifecycle", () => {
  it("stores the roster delivered by an avatars event", () => {
    const next = roster();
    store.apply({ type: "avatars", roster: next });
    expect(store.avatars).toBe(next);
  });

  it("replaces the roster when a newer one arrives", () => {
    const first = roster(1);
    const second = roster(2);
    store.apply({ type: "avatars", roster: first });
    store.apply({ type: "avatars", roster: second });
    expect(store.avatars).toBe(second);
    expect(store.avatars?.version).toBe(2);
  });

  it("clears the roster when the room changes", () => {
    store.avatars = roster();
    store.apply({ type: "room_entered", room });
    expect(store.avatars).toBeNull();
  });

  it("clears the roster when the session disconnects", () => {
    store.avatars = roster();
    store.apply({ type: "status", status: "disconnected", message: null });
    expect(store.avatars).toBeNull();
  });

  it("clears the roster when the session errors", () => {
    store.avatars = roster();
    store.apply({ type: "status", status: "error", message: "boom" });
    expect(store.avatars).toBeNull();
  });

  it("keeps the roster while the session stays connected", () => {
    const next = roster();
    store.avatars = next;
    store.apply({ type: "status", status: "connected", message: null });
    expect(store.avatars).toBe(next);
  });

  it("leaves the roster alone when a screen event arrives", () => {
    const next = roster();
    store.avatars = next;
    store.apply({ type: "screen", screen });
    expect(store.avatars).toBe(next);
    expect(store.screen).toBe(screen);
  });

  it("repopulates after a room change once a new roster arrives", () => {
    store.apply({ type: "avatars", roster: roster(1) });
    store.apply({ type: "room_entered", room });
    expect(store.avatars).toBeNull();
    const next = roster(9);
    store.apply({ type: "avatars", roster: next });
    expect(store.avatars).toBe(next);
  });
});
