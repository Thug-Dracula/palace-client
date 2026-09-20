import { beforeEach, describe, expect, it, vi } from "vitest";

import { MAX_PROPS, store } from "./store.svelte";
import type { BagPropEntry, UserInfo } from "./api";

const setProps = vi.spyOn(store, "setProps").mockImplementation(() => {});

const selfWith = (props: number[]): UserInfo => ({
  id: 1,
  name: "Me",
  face: 0,
  color: 0,
  room_id: 1,
  x: 0,
  y: 0,
  props,
  away: false,
  is_self: true,
  avatar_type: 0,
  avatar_flags: 0,
  avatar_hash: null,
});

const bagEntry = (id: number, crc: number): BagPropEntry => ({
  id,
  crc,
  name: null,
  w: 44,
  h: 44,
  flags: 0,
  fav: false,
  trash: false,
  collection: "My Bag",
  source: "bag",
});

beforeEach(() => {
  setProps.mockClear();
  store.users = [selfWith([])];
});

describe("wearing from the bag", () => {
  it("sends the entry's id through set_props", () => {
    const result = store.toggleBagProp(bagEntry(7, 0x505193c2));
    expect(result.ok).toBe(true);
    expect(store.self?.props).toEqual([7]);
    expect(setProps).toHaveBeenLastCalledWith([7]);
  });

  it("takes the same entry off again", () => {
    store.toggleBagProp(bagEntry(7, 1));
    const result = store.toggleBagProp(bagEntry(7, 1));
    expect(result.ok).toBe(true);
    expect(store.self?.props).toEqual([]);
    expect(setProps).toHaveBeenLastCalledWith([]);
  });

  it("wears a list in order", () => {
    const result = store.wearBagEntries([bagEntry(4, 1), bagEntry(5, 2), bagEntry(6, 3)]);
    expect(result.ok).toBe(true);
    expect(store.self?.props).toEqual([4, 5, 6]);
  });

  it(`rejects the ${MAX_PROPS + 1}th prop with the limit message`, () => {
    store.users = [selfWith([1, 2, 3, 4, 5, 6, 7, 8, 9])];
    const result = store.toggleBagProp(bagEntry(10, 1));
    expect(result.ok).toBe(false);
    expect(result.error).toContain(String(MAX_PROPS));
    expect(store.self?.props).toHaveLength(MAX_PROPS);
  });

  it("keeps takeOffAvatar clearing the whole set", () => {
    store.users = [selfWith([7, 8])];
    store.takeOffAvatar();
    expect(store.self?.props).toEqual([]);
    expect(setProps).toHaveBeenLastCalledWith([]);
  });
});
