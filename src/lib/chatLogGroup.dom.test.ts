// @vitest-environment jsdom
/**
 * Task 24's acceptance tests: the Chat logging preference group.
 *
 * The plan asks for three things and each has a test that can fail:
 *
 * * the toggle, the destination and the cap persist, and the writer's state
 *   (resolved path, bytes so far) is shown from the backend rather than guessed;
 * * a bad value or a failed write leaves the controls showing what the file
 *   actually holds;
 * * the shell routes `chat_log` to the real component, not the placeholder.
 *
 * The in-memory half is proved separately: it is the backend transcript, and
 * `chatPanel.dom.test.ts` already covers a late-opening window being seeded
 * from the replay with no file logging involved. Here the note names it.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const state = vi.hoisted(() => {
  const isRecord = (value: unknown): value is Record<string, unknown> =>
    typeof value === "object" && value !== null && !Array.isArray(value);
  const merge = (
    base: Record<string, unknown>,
    patch: Record<string, unknown>,
  ): Record<string, unknown> => {
    const out: Record<string, unknown> = { ...base };
    for (const [key, value] of Object.entries(patch)) {
      out[key] = isRecord(value) && isRecord(out[key]) ? merge(out[key], value) : value;
    }
    return out;
  };
  const DEFAULT_PATH = "$HOME/.local/share/org.palace.client/logs/chat-transcript.log";
  const chatLog = () => (state.stored.chat_log ?? {}) as Record<string, unknown>;
  const syncStatus = (): void => {
    const stored = chatLog();
    state.status = {
      enabled: stored.to_file === true,
      path: typeof stored.path === "string" ? stored.path : DEFAULT_PATH,
      max_bytes: typeof stored.max_bytes === "number" ? stored.max_bytes : 8 * 1024 * 1024,
      rotate_files: typeof stored.rotate_files === "number" ? stored.rotate_files : 3,
      bytes_written: state.status.bytes_written,
    };
  };
  const state = {
    stored: {} as Record<string, unknown>,
    failNextSetPrefs: false,
    status: {
      enabled: false,
      path: DEFAULT_PATH,
      max_bytes: 8 * 1024 * 1024,
      rotate_files: 3,
      bytes_written: 0,
    },
    pick: null as string | null,
    merge,
    syncStatus,
    invoke: vi.fn(async () => true),
  };
  return state;
});

vi.mock("@tauri-apps/api/core", () => ({ invoke: state.invoke }));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  save: vi.fn(async () => state.pick),
}));

vi.mock("./api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./api")>();
  return {
    ...actual,
    getPrefs: vi.fn(async () => state.stored),
    setPrefs: vi.fn(async (patch: Record<string, unknown>) => {
      if (state.failNextSetPrefs) {
        state.failNextSetPrefs = false;
        throw new Error("the disk is full");
      }
      state.stored = state.merge(state.stored, patch);
      state.syncStatus();
      return state.stored;
    }),
    resetPrefs: vi.fn(async () => {
      state.stored = {};
      state.syncStatus();
      return state.stored;
    }),
    chatLogStatus: vi.fn(async () => ({ ...state.status })),
    getSettings: vi.fn(async () => ({
      host: "localhost",
      port: 9998,
      username: "Guest",
      soundfont: null,
    })),
    openPreferences: vi.fn(async () => "created"),
    closePreferences: vi.fn(async () => "closed"),
    openPanel: vi.fn(async () => "created"),
    closePanel: vi.fn(async () => "closed"),
    onEvent: vi.fn(async () => () => {}),
    refresh: vi.fn(async () => {}),
    setViewport: vi.fn(async () => {}),
  };
});

import { mount, tick, unmount } from "svelte";

import * as api from "./api";
import ChatLogGroup from "./components/prefs/ChatLogGroup.svelte";
import PreferencesWindow from "./components/prefs/PreferencesWindow.svelte";
import { prefs } from "./prefsStore.svelte";

let mounted: ReturnType<typeof mount> | undefined;

function byTestId<T extends HTMLElement>(id: string): T {
  const node = document.querySelector<T>(`[data-testid="${id}"]`);
  if (!node) {
    throw new Error(`no element with data-testid ${id}`);
  }
  return node;
}

function storedChatLog(): Record<string, unknown> {
  return (state.stored.chat_log ?? {}) as Record<string, unknown>;
}

function statusText(): string {
  return (byTestId("chat-log-status").textContent ?? "").replace(/\s+/g, " ");
}

async function renderGroup(): Promise<void> {
  await prefs.load();
  mounted = mount(ChatLogGroup, { target: document.body });
  await tick();
  await vi.waitFor(() => expect(vi.mocked(api.chatLogStatus)).toHaveBeenCalled());
}

async function clickBox(id: string): Promise<HTMLInputElement> {
  const box = byTestId<HTMLInputElement>(id);
  box.click();
  await tick();
  return box;
}

async function setText(id: string, value: string): Promise<HTMLInputElement> {
  const input = byTestId<HTMLInputElement>(id);
  input.value = value;
  input.dispatchEvent(new Event("input", { bubbles: true }));
  input.dispatchEvent(new Event("change", { bubbles: true }));
  await tick();
  return input;
}

beforeEach(() => {
  vi.clearAllMocks();
  state.stored = {};
  state.failNextSetPrefs = false;
  state.pick = null;
  state.status = {
    enabled: false,
    path: "$HOME/.local/share/org.palace.client/logs/chat-transcript.log",
    max_bytes: 8 * 1024 * 1024,
    rotate_files: 3,
    bytes_written: 0,
  };
  prefs.reset();
  document.body.innerHTML = "";
});

afterEach(() => {
  if (mounted) {
    unmount(mounted);
    mounted = undefined;
  }
  document.body.innerHTML = "";
});

describe("the shell routes the group", () => {
  it("shows the real Chat logging group, not the placeholder", async () => {
    mounted = mount(PreferencesWindow, { target: document.body });
    await tick();
    await vi.waitFor(() =>
      expect(document.querySelectorAll(".prefs-nav-item").length).toBeGreaterThan(0),
    );

    const nav = [...document.querySelectorAll<HTMLButtonElement>(".prefs-nav-item")].find(
      (button) => button.textContent?.trim() === "Chat logging",
    );
    expect(nav).toBeDefined();
    nav!.click();
    await tick();

    expect(byTestId("chat-log-memory-note")).toBeTruthy();
    expect(byTestId("chat-log-to-file")).toBeTruthy();
    expect(document.querySelector(".prefs-placeholder")).toBeNull();
  });
});

describe("the two capabilities stay separate", () => {
  it("names the in-memory log as always on, with the retained line count", async () => {
    await renderGroup();
    const note = byTestId("chat-log-memory-note").textContent ?? "";
    expect(note).toContain("Always on");
    expect(note).toContain("500");
  });

  it("leaves file logging off by default and says nothing is written", async () => {
    await renderGroup();
    expect(byTestId<HTMLInputElement>("chat-log-to-file").checked).toBe(false);
    expect(statusText()).toContain("File logging is off");
    expect(statusText()).toContain("Nothing is written while it is off");
  });

  it("turns file logging on, persists it, and shows the resolved destination", async () => {
    await renderGroup();

    await clickBox("chat-log-to-file");

    await vi.waitFor(() => expect(storedChatLog().to_file).toBe(true));
    await vi.waitFor(() => expect(statusText()).toContain("Writing to"));
    expect(statusText()).toContain("chat-transcript.log");
  });

  it("stops file logging again when the switch is cleared", async () => {
    state.stored = { chat_log: { to_file: true } };
    state.syncStatus();
    await renderGroup();
    expect(byTestId<HTMLInputElement>("chat-log-to-file").checked).toBe(true);

    await clickBox("chat-log-to-file");

    await vi.waitFor(() => expect(storedChatLog().to_file).toBe(false));
    await vi.waitFor(() => expect(statusText()).toContain("File logging is off"));
  });

  it("a failed write leaves the switch off instead of lying", async () => {
    await renderGroup();
    state.failNextSetPrefs = true;

    const box = await clickBox("chat-log-to-file");

    await vi.waitFor(() => expect(box.checked).toBe(false));
    expect(document.querySelector(".prefs-error")?.textContent).toContain("Could not save");
    expect(storedChatLog().to_file).toBeUndefined();
  });

  it("says so when the switch is on but the file could not be opened", async () => {
    state.stored = { chat_log: { to_file: true, path: "/nope/read-only/room.log" } };
    state.syncStatus();
    // The backend stores the preference but could not open the file, so the
    // writer is not enabled: the UI must say that rather than claim success.
    state.status = { ...state.status, enabled: false };
    await renderGroup();

    await vi.waitFor(() => expect(statusText()).toContain("could not be opened"));
    expect(statusText()).toContain("/nope/read-only/room.log");
  });
});

describe("the destination", () => {
  it("persists a typed destination and reports it as the live one", async () => {
    await renderGroup();

    await setText("chat-log-path", "$HOME/notes/room.log");

    await vi.waitFor(() => expect(storedChatLog().path).toBe("$HOME/notes/room.log"));
    await vi.waitFor(() => expect(statusText()).toContain("$HOME/notes/room.log"));
  });

  it("persists the file dialog's choice", async () => {
    state.pick = "$HOME/Gallery/logs/room.log";
    await renderGroup();

    byTestId<HTMLButtonElement>("chat-log-pick").click();
    await tick();

    await vi.waitFor(() => expect(storedChatLog().path).toBe("$HOME/Gallery/logs/room.log"));
  });

  it("clearing the field returns to the default location", async () => {
    state.stored = { chat_log: { path: "$HOME/notes/room.log" } };
    state.syncStatus();
    await renderGroup();

    await setText("chat-log-path", "   ");

    await vi.waitFor(() => expect(storedChatLog().path).toBeNull());
    await vi.waitFor(() => expect(statusText()).toContain("chat-transcript.log"));
  });

  it("the Default location button clears a chosen destination", async () => {
    state.stored = { chat_log: { path: "$HOME/notes/room.log" } };
    state.syncStatus();
    await renderGroup();

    byTestId<HTMLButtonElement>("chat-log-default-path").click();
    await tick();

    await vi.waitFor(() => expect(storedChatLog().path).toBeNull());
  });
});

describe("the size cap and rotations", () => {
  it("persists a size cap in bytes and reads it back as a whole unit", async () => {
    await renderGroup();
    expect(byTestId("chat-log-max-bytes-readout").textContent).toContain("8 MiB");

    await setText("chat-log-max-bytes", "2097152");

    await vi.waitFor(() => expect(storedChatLog().max_bytes).toBe(2 * 1024 * 1024));
    await vi.waitFor(() =>
      expect(byTestId("chat-log-max-bytes-readout").textContent).toContain("2 MiB"),
    );
  });

  it("refuses a cap outside the accepted range and restores the stored one", async () => {
    await renderGroup();

    const input = await setText("chat-log-max-bytes", "1");

    await vi.waitFor(() =>
      expect(document.querySelector(".prefs-error")?.textContent).toContain("size cap"),
    );
    expect(storedChatLog().max_bytes).toBeUndefined();
    expect(input.value).toBe(String(8 * 1024 * 1024));
  });

  it("persists a rotation count and refuses one outside the accepted range", async () => {
    await renderGroup();

    await setText("chat-log-rotate-files", "0");
    await vi.waitFor(() => expect(storedChatLog().rotate_files).toBe(0));

    const input = await setText("chat-log-rotate-files", "99");
    await vi.waitFor(() =>
      expect(document.querySelector(".prefs-error")?.textContent).toContain("rotations"),
    );
    expect(input.value).toBe("0");
    expect(storedChatLog().rotate_files).toBe(0);
  });

  it("shows the backend's cap and rotation count, not the defaults", async () => {
    state.stored = { chat_log: { to_file: true, max_bytes: 4096, rotate_files: 1 } };
    state.syncStatus();
    await renderGroup();

    await vi.waitFor(() => expect(statusText()).toContain("4 KiB"));
    expect(statusText()).toContain("1 kept file");
  });
});
