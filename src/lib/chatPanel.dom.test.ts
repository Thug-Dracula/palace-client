// @vitest-environment jsdom
/**
 * Task 15 — the detached Chat/log panel.
 *
 * A late-opening `panel-chat` window seeds its log from the backend replay,
 * sends through the same `say` command as the main shell, and shows a line
 * echoed by the backend in both views. These tests mount the real
 * `PanelWindow`/`ChatPanel` under jsdom with the IPC layer mocked, so the
 * behaviour under test is the panel's, not a reimplementation.
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
  };
});

import { tick, mount, unmount } from "svelte";

import PanelWindow from "./PanelWindow.svelte";
import ChatPanel from "./components/ChatPanel.svelte";
import * as api from "./api";
import type { ChatLine, ServerBanner } from "./api";
import { store } from "./store.svelte";

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

const talk = (seq: number, text: string, over: Partial<ChatLine> = {}): ChatLine => ({
  seq,
  user_id: 1,
  name: "Me",
  text,
  kind: "talk",
  ...over,
});

let mounted: Array<ReturnType<typeof mount>> = [];
let panelHost: HTMLElement;

function mountIn(target: HTMLElement, component: unknown, props?: Record<string, unknown>) {
  const instance = mount(component as never, { target, props });
  mounted.push(instance);
  return instance;
}

function logLines(root: ParentNode): string[] {
  return [...root.querySelectorAll("section.chat .line .text")].map(
    (node) => node.textContent ?? "",
  );
}

function chatInput(root: ParentNode): HTMLInputElement {
  const input = root.querySelector<HTMLInputElement>('input[aria-label="Chat message"]');
  if (!input) {
    throw new Error("chat input not found");
  }
  return input;
}

function type(input: HTMLInputElement, text: string): void {
  input.value = text;
  input.dispatchEvent(new Event("input", { bubbles: true }));
}

function submit(input: HTMLInputElement): void {
  input.closest("form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
}

async function mountPanelWindow(): Promise<void> {
  window.location.hash = "#/panel/chat";
  panelHost = document.createElement("div");
  document.body.append(panelHost);
  mountIn(panelHost, PanelWindow, { panel: "chat" });
  await tick();
}

function seedOnRefresh(lines: ChatLine[]): void {
  vi.mocked(api.refresh).mockImplementation(async () => {
    store.apply({ type: "status", status: "connected", message: null });
    for (const line of lines) {
      store.apply({ type: "chat", line });
    }
  });
}

function resetStore(): void {
  store.status = "connecting";
  store.statusMessage = null;
  store.banner = null;
  store.users = [];
  store.chat = [];
  store.settings = { host: "localhost", port: 9998, username: "Guest", soundfont: null };
}

beforeEach(() => {
  resetStore();
  vi.clearAllMocks();
  vi.mocked(api.refresh).mockImplementation(async () => {});
  vi.mocked(api.getSettings).mockResolvedValue({
    host: "localhost",
    port: 9998,
    username: "Guest",
    soundfont: null,
  });
  window.location.hash = "";
  document.body.innerHTML = "";
});

afterEach(() => {
  for (const instance of mounted) {
    unmount(instance);
  }
  mounted = [];
  window.location.hash = "";
  document.body.innerHTML = "";
});

describe("detached chat panel — late-open seeding", () => {
  it("renders the retained scrollback the backend replays on open", async () => {
    seedOnRefresh([talk(1, "one"), talk(2, "two"), talk(3, "three")]);

    await mountPanelWindow();

    await vi.waitFor(() => expect(logLines(panelHost)).toHaveLength(3));
    expect(logLines(panelHost)).toEqual(["one", "two", "three"]);
    expect(panelHost.querySelector(".empty-note")).toBeNull();
  });

  it("keeps the log pinned to the newest line as history grows", async () => {
    seedOnRefresh([talk(1, "one")]);

    await mountPanelWindow();
    await vi.waitFor(() => expect(logLines(panelHost)).toHaveLength(1));

    const log = panelHost.querySelector<HTMLDivElement>("section.chat .chat-log")!;
    // jsdom has no layout; give the log a scrollable height and assert the
    // effect pins scrollTop to it when a new line lands.
    Object.defineProperty(log, "scrollHeight", { configurable: true, value: 1234 });
    store.apply({ type: "chat", line: talk(2, "two") });
    await tick();
    expect(log.scrollTop).toBe(1234);
  });
});

describe("detached chat panel — sending", () => {
  it("sends through the same command the main shell uses, echoing once", async () => {
    store.status = "connected";
    store.banner = banner;
    await mountPanelWindow();

    const input = chatInput(panelHost);
    type(input, "hello from the panel");
    submit(input);
    await tick();

    expect(vi.mocked(api.say)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(api.say)).toHaveBeenCalledWith("hello from the panel");

    store.apply({ type: "chat", line: talk(42, "hello from the panel") });
    await tick();
    expect(store.chat.filter((line) => line.text === "hello from the panel")).toHaveLength(1);
    expect(logLines(panelHost)).toEqual(["hello from the panel"]);
  });

  it("shows a message sent from the detached window in the main view too", async () => {
    store.status = "connected";
    store.banner = banner;

    const mainHost = document.createElement("div");
    document.body.append(mainHost);
    mountIn(mainHost, ChatPanel);
    await mountPanelWindow();
    await tick();

    const input = chatInput(panelHost);
    type(input, "broadcast me");
    submit(input);
    await tick();

    store.apply({ type: "chat", line: talk(7, "broadcast me") });
    await tick();

    expect(mainHost.querySelectorAll(".line")).toHaveLength(1);
    expect(mainHost.querySelector(".line .text")?.textContent).toBe("broadcast me");
    expect(logLines(panelHost)).toEqual(["broadcast me"]);
  });

  it("seeds settings in the panel so a sent line echoes the signed-in name", async () => {
    store.status = "connected";
    store.banner = banner;
    vi.mocked(api.getSettings).mockResolvedValue({
      host: "localhost",
      port: 9998,
      username: "Zed",
      soundfont: null,
    });

    await mountPanelWindow();
    await vi.waitFor(() => expect(store.settings.username).toBe("Zed"));

    const input = chatInput(panelHost);
    type(input, "hi");
    submit(input);
    await tick();

    expect(store.chat.find((line) => line.pending)?.name).toBe("Zed");
  });

  it("does not send while the session is not connected", async () => {
    await mountPanelWindow();
    const input = chatInput(panelHost);
    type(input, "nope");
    submit(input);
    await tick();
    expect(vi.mocked(api.say)).not.toHaveBeenCalled();
  });
});

describe("detached chat panel — input limit and focus", () => {
  it("caps an over-long message at 254 characters", async () => {
    store.status = "connected";
    await mountPanelWindow();

    const input = chatInput(panelHost);
    expect(input.maxLength).toBe(254);
    type(input, "x".repeat(300));
    submit(input);
    await tick();

    expect(vi.mocked(api.say)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(api.say).mock.calls[0][0]).toHaveLength(254);
  });

  it("focuses the chat input in a detached window once connected", async () => {
    store.status = "connected";
    store.banner = banner;

    await mountPanelWindow();

    const input = chatInput(panelHost);
    await vi.waitFor(() => expect(document.activeElement).toBe(input));
  });

  it("keeps the panel input focused after sending", async () => {
    store.status = "connected";
    store.banner = banner;
    await mountPanelWindow();
    const input = chatInput(panelHost);
    await vi.waitFor(() => expect(document.activeElement).toBe(input));

    input.blur();
    type(input, "stay focused");
    submit(input);
    await tick();

    expect(document.activeElement).toBe(input);
  });

  it("does not steal focus in the main shell", async () => {
    store.status = "connected";
    window.location.hash = "";
    const host = document.createElement("div");
    document.body.append(host);
    mountIn(host, ChatPanel);
    await tick();

    const input = chatInput(host);
    expect(document.activeElement).not.toBe(input);
  });
});
