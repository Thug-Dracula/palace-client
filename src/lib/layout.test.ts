/**
 * The grid reflow is arithmetic, so it is pinned here without a DOM:
 * the docked sizes, the rail a detached slot collapses to, the CSS custom
 * properties the shell applies, and the stable `data-detached` ordering.
 */
import { describe, expect, it } from "vitest";

import {
  CHAT_HEIGHT,
  RAIL,
  ROOMS_WIDTH,
  USERS_WIDTH,
  detachedAttr,
  trackStyle,
  workspaceTracks,
} from "./layout";
import { PANEL_IDS } from "./panels";

describe("workspaceTracks", () => {
  it("uses the docked sizes when nothing is detached", () => {
    expect(workspaceTracks([])).toEqual({
      rooms: ROOMS_WIDTH,
      users: USERS_WIDTH,
      chat: CHAT_HEIGHT,
    });
  });

  it("collapses only the detached panel's own track", () => {
    expect(workspaceTracks(["users"])).toEqual({
      rooms: ROOMS_WIDTH,
      users: RAIL,
      chat: CHAT_HEIGHT,
    });
    expect(workspaceTracks(["rooms"])).toEqual({
      rooms: RAIL,
      users: USERS_WIDTH,
      chat: CHAT_HEIGHT,
    });
    expect(workspaceTracks(["chat"])).toEqual({
      rooms: ROOMS_WIDTH,
      users: USERS_WIDTH,
      chat: RAIL,
    });
  });

  it("leaves the tracks alone when only the room view is detached", () => {
    // The room view fills `1fr`; detaching it swaps the component, not a track.
    expect(workspaceTracks(["room"])).toEqual({
      rooms: ROOMS_WIDTH,
      users: USERS_WIDTH,
      chat: CHAT_HEIGHT,
    });
  });

  it("collapses every track when several panels are out", () => {
    expect(workspaceTracks(["rooms", "users", "chat", "room"])).toEqual({
      rooms: RAIL,
      users: RAIL,
      chat: RAIL,
    });
  });

  it("gives the room more space as its neighbours collapse", () => {
    const docked = workspaceTracks([]);
    const detached = workspaceTracks(["rooms", "users", "chat"]);
    const sideSpace = (tracks: { rooms: number; users: number; chat: number }): number =>
      tracks.rooms + tracks.users + tracks.chat;
    expect(sideSpace(detached)).toBeLessThan(sideSpace(docked));
    expect(detached.rooms).toBe(RAIL);
    expect(detached.users).toBe(RAIL);
    expect(detached.chat).toBe(RAIL);
  });
});

describe("trackStyle", () => {
  it("emits the three custom properties the stylesheet consumes", () => {
    expect(trackStyle([])).toBe("--col-rooms:232px;--col-users:208px;--row-chat:236px");
  });

  it("reflects a collapsed rail", () => {
    expect(trackStyle(["users"])).toBe("--col-rooms:232px;--col-users:44px;--row-chat:236px");
    expect(trackStyle(["rooms", "chat"])).toBe(
      "--col-rooms:44px;--col-users:208px;--row-chat:44px",
    );
  });
});

describe("detachedAttr", () => {
  it("is empty when nothing is detached", () => {
    expect(detachedAttr([])).toBe("");
  });

  it("lists ids in the fixed panel order, not the input order", () => {
    expect(detachedAttr(["chat", "rooms"])).toBe("rooms chat");
    expect(detachedAttr(["props", "room", "users"])).toBe("room users props");
  });

  it("uses the same order as PANEL_IDS", () => {
    expect(detachedAttr([...PANEL_IDS].reverse())).toBe(PANEL_IDS.join(" "));
  });
});
