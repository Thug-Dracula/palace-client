/**
 * Regression baseline: the refresh snapshot drives the ONE shared store.
 *
 * The backend replays this exact event sequence on `refresh()`. The upcoming
 * multi-window work keeps a single store per webview and a single event
 * channel, so this file pins how the snapshot lands in that store before the
 * refactor can move anything around.
 */
import { beforeEach, describe, expect, it } from "vitest";

import { store } from "./store.svelte";
import { EVENT_NAME } from "./api";
import type {
  AvatarRoster,
  ClientEvent,
  RoomInfo,
  ScreenState,
  ServerBanner,
  UserInfo,
  ViewGeometry,
} from "./api";

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

const room = (id: number, name: string): RoomInfo => ({ id, name, users: 1, flags: 0 });

const user = (over: Partial<UserInfo> = {}): UserInfo => ({
  id: 1,
  name: "Me",
  face: 0,
  color: 0,
  room_id: 1,
  x: 0,
  y: 0,
  props: [],
  away: false,
  is_self: true,
  avatar_type: 0,
  avatar_flags: 0,
  avatar_hash: null,
  ...over,
});

const banner: ServerBanner = {
  host: "localhost",
  port: 9998,
  byte_order: "LE",
  user_id: 1,
  version: "1.0",
  name: "Lobby Server",
  media_base: null,
  total_users: 2,
};

const screen: ScreenState = {
  version: 7,
  room_id: 1,
  room_name: "Lobby",
  avatars: 1,
  loose_props: 0,
  props_pending: 0,
  notes: ["note one", "note two"],
  geometry,
  mid_version: null,
  top_version: null,
};

const roster: AvatarRoster = {
  version: 3,
  room_id: 1,
  geometry,
  name_tags_visible: true,
  avatars: [],
};

/** The exact order the backend replays on `refresh()`. */
const snapshot: ClientEvent[] = [
  { type: "status", status: "connected", message: null },
  { type: "banner", banner },
  { type: "rooms", rooms: [room(2, "Zeta"), room(1, "Alpha")] },
  { type: "users", users: [user()] },
  { type: "room_entered", room: room(1, "Alpha") },
  { type: "chat", line: { seq: 1, user_id: 1, name: "Me", text: "hello", kind: "talk" } },
  { type: "screen", screen },
  { type: "avatars", roster },
];

function resetStore(): void {
  store.status = "connecting";
  store.statusMessage = null;
  store.banner = null;
  store.rooms = [];
  store.users = [];
  store.room = null;
  store.chat = [];
  store.screen = null;
  store.avatars = null;
  store.notes = [];
  store.notices = [];
  store.tooltip = null;
}

beforeEach(resetStore);

describe("refresh snapshot -> single store", () => {
  it("subscribes to the one shared event channel name", () => {
    expect(EVENT_NAME).toBe("palace://event");
  });

  it("applies every snapshot event to the shared store", () => {
    for (const event of snapshot) {
      store.apply(event);
    }

    expect(store.status).toBe("connected");
    expect(store.connected).toBe(true);
    expect(store.banner).toBe(banner);
    expect(store.users).toEqual([user()]);
    expect(store.room).toEqual(room(1, "Alpha"));
    expect(store.chat).toHaveLength(1);
    expect(store.screen).toBe(screen);
    expect(store.notes).toEqual(["note one", "note two"]);
    expect(store.avatars).toBe(roster);
  });

  it("starts from an empty store", () => {
    expect(store.status).toBe("connecting");
    expect(store.rooms).toHaveLength(0);
    expect(store.users).toHaveLength(0);
    expect(store.chat).toHaveLength(0);
    expect(store.screen).toBeNull();
    expect(store.avatars).toBeNull();
  });

  it("sorts the room list by name when a rooms event arrives", () => {
    store.apply({ type: "rooms", rooms: [room(3, "Charlie"), room(1, "Alpha"), room(2, "Bravo")] });
    expect(store.rooms.map((entry) => entry.name)).toEqual(["Alpha", "Bravo", "Charlie"]);
  });

  it("replaces the user list on a users event", () => {
    const first = user({ id: 1, name: "One" });
    const second = user({ id: 2, name: "Two", is_self: false });
    store.apply({ type: "users", users: [first] });
    expect(store.users).toEqual([first]);
    store.apply({ type: "users", users: [second] });
    expect(store.users).toEqual([second]);
    expect(store.self).toBeNull();
  });

  it("finds self in the roster", () => {
    store.apply({ type: "users", users: [user({ id: 9, name: "Me", is_self: true })] });
    expect(store.self?.id).toBe(9);
  });

  it("sets the room and clears the roster on room_entered", () => {
    store.avatars = roster;
    store.apply({ type: "room_entered", room: room(5, "Next") });
    expect(store.room).toEqual(room(5, "Next"));
    expect(store.avatars).toBeNull();
  });

  it("stores the screen and copies its notes", () => {
    store.apply({ type: "screen", screen });
    expect(store.screen).toBe(screen);
    expect(store.notes).toEqual(screen.notes);
  });

  it("stores and replaces the avatar roster", () => {
    const next = { ...roster, version: 4 };
    store.apply({ type: "avatars", roster });
    expect(store.avatars).toBe(roster);
    store.apply({ type: "avatars", roster: next });
    expect(store.avatars).toBe(next);
  });

  it("records a status change and its message", () => {
    store.apply({ type: "status", status: "error", message: "boom" });
    expect(store.status).toBe("error");
    expect(store.statusMessage).toBe("boom");
    expect(store.connected).toBe(false);
  });

  it("caps the chat history at 500 lines", () => {
    for (let index = 0; index < 520; index += 1) {
      store.apply({
        type: "chat",
        line: { seq: index, user_id: 1, name: "Me", text: `line ${index}`, kind: "talk" },
      });
    }
    expect(store.chat).toHaveLength(500);
    expect(store.chat[0].text).toBe("line 20");
    expect(store.chat[499].text).toBe("line 519");
  });
});
