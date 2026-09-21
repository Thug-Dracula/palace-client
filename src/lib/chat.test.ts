/**
 * The chat domain helpers and the store's replay-safe mirror.
 *
 * `refresh()` replays the backend's retained scrollback to seed a late-opening
 * window, and the backend broadcasts that replay to every window. These tests
 * pin both halves: the input clamp, the identity rule, and the fact that a
 * second replay of lines the mirror already holds changes nothing.
 */
import { beforeEach, describe, expect, it } from "vitest";

import { CHAT_INPUT_LIMIT, chatLineAlreadySeen, clampChatDraft } from "./chat";
import type { ChatLine } from "./api";
import { store } from "./store.svelte";

const talk = (seq: number, text: string): ChatLine => ({
  seq,
  user_id: 1,
  name: "Me",
  text,
  kind: "talk",
});

beforeEach(() => {
  store.chat = [];
});

describe("clampChatDraft", () => {
  it("keeps a message at or under the limit unchanged", () => {
    const exact = "x".repeat(CHAT_INPUT_LIMIT);
    expect(CHAT_INPUT_LIMIT).toBe(254);
    expect(clampChatDraft("short")).toBe("short");
    expect(clampChatDraft(exact)).toHaveLength(CHAT_INPUT_LIMIT);
  });

  it("cuts an over-long message down to the limit", () => {
    const clamped = clampChatDraft("y".repeat(1000));
    expect(clamped).toHaveLength(CHAT_INPUT_LIMIT);
    expect(clamped).toBe("y".repeat(CHAT_INPUT_LIMIT));
  });
});

describe("chatLineAlreadySeen", () => {
  it("matches an existing backend seq and ignores a local echo", () => {
    const mirror = [talk(1, "one")];
    expect(chatLineAlreadySeen(mirror, talk(1, "one"))).toBe(true);
    expect(chatLineAlreadySeen(mirror, talk(2, "two"))).toBe(false);
    expect(chatLineAlreadySeen(mirror, { ...talk(0, "pending"), seq: -1 })).toBe(false);
  });
});

describe("store chat mirror under a replayed snapshot", () => {
  it("seeds every retained line into an empty mirror", () => {
    for (const line of [talk(1, "one"), talk(2, "two"), talk(3, "three")]) {
      store.pushLine(line);
    }
    expect(store.chat.map((line) => line.text)).toEqual(["one", "two", "three"]);
  });

  it("does not duplicate lines when the same replay is broadcast again", () => {
    const replay = [talk(1, "one"), talk(2, "two"), talk(3, "three")];
    for (const line of replay) {
      store.pushLine(line);
    }
    for (const line of replay) {
      store.pushLine(line);
    }
    expect(store.chat.map((line) => line.text)).toEqual(["one", "two", "three"]);
  });

  it("still appends a genuinely new line after a replay", () => {
    store.pushLine(talk(1, "one"));
    store.pushLine(talk(1, "one"));
    store.pushLine(talk(4, "four"));
    expect(store.chat.map((line) => line.text)).toEqual(["one", "four"]);
  });
});
