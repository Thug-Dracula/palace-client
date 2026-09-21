// @vitest-environment jsdom
/**
 * Task 24 — the chat panel's file-logging indicator.
 *
 * File logging is a user-visible privacy-relevant state: while it is on, the
 * room is being written to disk. The chat panel therefore carries a small,
 * always-present-while-on marker naming the file, and nothing at all while
 * logging is off. These tests mount the real `ChatPanel` with the IPC layer
 * mocked, so the marker's behaviour is the panel's, not a reimplementation.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const unlisten = vi.fn();

vi.mock("./api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./api")>();
  return {
    ...actual,
    onEvent: vi.fn(async () => unlisten),
    refresh: vi.fn(async () => {}),
    getSettings: vi.fn(async () => ({
      host: "localhost",
      port: 9998,
      username: "Guest",
      soundfont: null,
    })),
    say: vi.fn(async () => {}),
    chatLogStatus: vi.fn(async () => ({
      enabled: false,
      path: "$HOME/.local/share/org.palace.client/logs/chat-transcript.log",
      max_bytes: 8 * 1024 * 1024,
      rotate_files: 3,
      bytes_written: 0,
    })),
  };
});

import { tick, mount, unmount } from "svelte";

import ChatPanel from "./components/ChatPanel.svelte";
import * as api from "./api";
import type { ChatLine } from "./api";
import { store } from "./store.svelte";

const talk = (seq: number, text: string): ChatLine => ({
  seq,
  user_id: 1,
  name: "Me",
  text,
  kind: "talk",
});

let mounted: Array<ReturnType<typeof mount>> = [];

function mountPanel(): void {
  const host = document.createElement("div");
  document.body.append(host);
  mounted.push(mount(ChatPanel, { target: host }));
}

function chip(): HTMLElement | null {
  return document.querySelector<HTMLElement>('[data-testid="chat-log-chip"]');
}

beforeEach(() => {
  vi.clearAllMocks();
  store.status = "connected";
  store.chat = [talk(1, "a line in memory")];
  window.location.hash = "";
  document.body.innerHTML = "";
});

afterEach(() => {
  for (const instance of mounted) {
    unmount(instance);
  }
  mounted = [];
  document.body.innerHTML = "";
});

describe("the chat panel's log marker", () => {
  it("is absent while file logging is off", async () => {
    mountPanel();
    await tick();
    await vi.waitFor(() => expect(vi.mocked(api.chatLogStatus)).toHaveBeenCalled());
    expect(chip()).toBeNull();
  });

  it("appears while file logging is on, naming the file it writes", async () => {
    vi.mocked(api.chatLogStatus).mockResolvedValue({
      enabled: true,
      path: "$HOME/notes/room.log",
      max_bytes: 4096,
      rotate_files: 1,
      bytes_written: 42,
    });

    mountPanel();
    await tick();

    await vi.waitFor(() => expect(chip()).not.toBeNull());
    expect(chip()?.getAttribute("title")).toContain("$HOME/notes/room.log");
  });

  it("does not disturb the transcript it sits above", async () => {
    vi.mocked(api.chatLogStatus).mockResolvedValue({
      enabled: true,
      path: "$HOME/notes/room.log",
      max_bytes: 4096,
      rotate_files: 1,
      bytes_written: 42,
    });

    mountPanel();
    await tick();

    await vi.waitFor(() => expect(chip()).not.toBeNull());
    const lines = [...document.querySelectorAll("section.chat .line .text")].map(
      (node) => node.textContent,
    );
    expect(lines).toEqual(["a line in memory"]);
  });

  it("stays absent when the backend cannot answer, rather than guessing", async () => {
    vi.mocked(api.chatLogStatus).mockRejectedValue(new Error("no such command"));

    mountPanel();
    await tick();
    await vi.waitFor(() => expect(vi.mocked(api.chatLogStatus)).toHaveBeenCalled());
    expect(chip()).toBeNull();
  });
});
