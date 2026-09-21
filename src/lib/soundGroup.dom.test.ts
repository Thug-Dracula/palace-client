// @vitest-environment jsdom
/**
 * Task 23's acceptance tests: the Sound preference group.
 *
 * The plan asks for four things and each has a test that can fail:
 *
 * * mute, master volume and the SoundFont change live AND persist (the mocked
 *   backend below is the settings file: a value survives a remount);
 * * the effects/music split and the speech voice are disabled, visibly marked
 *   and write nothing;
 * * a missing SoundFont path degrades gracefully: it cannot be committed, a
 *   vanished file that is already stored is reported as the bundled-bank
 *   fallback rather than crashing or lying;
 * * the shell routes `sound` to the real component, not the placeholder.
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
  return {
    stored: {} as Record<string, unknown>,
    /** The mocked settings file's audio half, as the backend would hold it. */
    audio: {
      enabled: true,
      volume: 1,
      soundfont: null as string | null,
      soundfont_exists: false,
      soundfont_fallback: false,
      soundfont_bundled: false,
    },
    /** What the file picker returns next; `null` means the user cancelled. */
    pick: null as string | null,
    /** Paths the mocked backend accepts as real `.sf2` files. */
    knownFonts: new Set<string>(),
    failNextWrite: false,
    merge,
    invoke: vi.fn(async () => true),
  };
});

vi.mock("@tauri-apps/api/core", () => ({ invoke: state.invoke }));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(async () => state.pick),
}));

vi.mock("./api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./api")>();
  return {
    ...actual,
    getPrefs: vi.fn(async () => state.stored),
    setPrefs: vi.fn(async (patch: Record<string, unknown>) => {
      if (state.failNextWrite) {
        state.failNextWrite = false;
        throw new Error("the disk is full");
      }
      state.stored = state.merge(state.stored, patch);
      return state.stored;
    }),
    resetPrefs: vi.fn(async () => {
      state.stored = {};
      return state.stored;
    }),
    getAudioState: vi.fn(async () => ({ ...state.audio })),
    setVolume: vi.fn(async (volume: number) => {
      if (state.failNextWrite) {
        state.failNextWrite = false;
        throw new Error("the disk is full");
      }
      const applied = Math.min(1, Math.max(0, volume));
      state.audio = { ...state.audio, volume: applied };
      return applied;
    }),
    setAudioEnabled: vi.fn(async (enabled: boolean) => {
      if (state.failNextWrite) {
        state.failNextWrite = false;
        throw new Error("the disk is full");
      }
      state.audio = { ...state.audio, enabled };
    }),
    setSoundfont: vi.fn(async (path: string | null) => {
      if (path !== null && !state.knownFonts.has(path)) {
        throw new Error(`${path} does not exist`);
      }
      state.audio =
        path === null
          ? {
              ...state.audio,
              soundfont: "/bundle/GeneralUser-GS.sf2",
              soundfont_exists: true,
              soundfont_fallback: false,
              soundfont_bundled: true,
            }
          : {
              ...state.audio,
              soundfont: path,
              soundfont_exists: true,
              soundfont_fallback: false,
              soundfont_bundled: false,
            };
    }),
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
import PreferencesWindow from "./components/prefs/PreferencesWindow.svelte";
import SoundGroup from "./components/prefs/SoundGroup.svelte";
import { prefs } from "./prefsStore.svelte";
import { store } from "./store.svelte";

const BUNDLED = "/bundle/GeneralUser-GS.sf2";

let mounted: ReturnType<typeof mount> | undefined;

function byTestId<T extends HTMLElement>(id: string): T {
  const node = document.querySelector<T>(`[data-testid="${id}"]`);
  if (!node) {
    throw new Error(`no element with data-testid ${id}`);
  }
  return node;
}

async function renderGroup(): Promise<void> {
  await prefs.load();
  mounted = mount(SoundGroup, { target: document.body });
  await tick();
  await vi.waitFor(() => expect(api.getAudioState).toHaveBeenCalled());
}

/** Move a range input and commit it the way a pointer release does. */
async function slide(id: string, value: number): Promise<HTMLInputElement> {
  const slider = byTestId<HTMLInputElement>(id);
  slider.value = String(value);
  slider.dispatchEvent(new Event("input", { bubbles: true }));
  slider.dispatchEvent(new Event("change", { bubbles: true }));
  await tick();
  return slider;
}

async function remount(): Promise<void> {
  if (mounted) {
    unmount(mounted);
    mounted = undefined;
  }
  document.body.innerHTML = "";
  prefs.reset();
  await renderGroup();
}

beforeEach(() => {
  vi.clearAllMocks();
  state.stored = {};
  state.pick = null;
  state.knownFonts = new Set<string>();
  state.failNextWrite = false;
  state.audio = {
    enabled: true,
    volume: 1,
    soundfont: null,
    soundfont_exists: false,
    soundfont_fallback: false,
    soundfont_bundled: false,
  };
  prefs.reset();
  store.audio = {
    enabled: true,
    volume: 1,
    soundfont: null,
    soundfont_exists: false,
  };
  document.body.innerHTML = "";
});

afterEach(() => {
  if (mounted) {
    unmount(mounted);
    mounted = undefined;
  }
  document.body.innerHTML = "";
});

describe("live apply and persistence", () => {
  it("applies the master volume to the engine and remembers it", async () => {
    await renderGroup();
    expect(byTestId("sound-volume-readout").textContent).toBe("100%");

    await slide("sound-volume", 0.4);

    await vi.waitFor(() => expect(vi.mocked(api.setVolume)).toHaveBeenCalledWith(0.4));
    await vi.waitFor(() => expect(byTestId("sound-volume-readout").textContent).toBe("40%"));
    expect(state.audio.volume).toBe(0.4);
    expect(byTestId("sound-notice").textContent).toContain("40%");

    await remount();
    await vi.waitFor(() => expect(byTestId("sound-volume-readout").textContent).toBe("40%"));
    expect(byTestId<HTMLInputElement>("sound-volume").value).toBe("0.4");
  });

  it("mutes and unmutes the engine and remembers the choice", async () => {
    await renderGroup();
    const box = byTestId<HTMLInputElement>("sound-mute");
    expect(box.checked).toBe(false);

    box.click();
    await vi.waitFor(() => expect(vi.mocked(api.setAudioEnabled)).toHaveBeenCalledWith(false));
    await vi.waitFor(() => expect(box.checked).toBe(true));
    expect(state.audio.enabled).toBe(false);
    await vi.waitFor(() =>
      expect(byTestId("sound-notice").textContent).toContain("muted"),
    );

    await remount();
    expect(byTestId<HTMLInputElement>("sound-mute").checked).toBe(true);

    byTestId<HTMLInputElement>("sound-mute").click();
    await vi.waitFor(() => expect(vi.mocked(api.setAudioEnabled)).toHaveBeenLastCalledWith(true));
    expect(state.audio.enabled).toBe(true);
    await tick();
    expect(byTestId<HTMLInputElement>("sound-mute").checked).toBe(false);
  });

  it("chooses a SoundFont, keeps it, and can return to the bundled bank", async () => {
    await renderGroup();
    const mine = "/fonts/mine.sf2";
    state.pick = mine;
    state.knownFonts.add(mine);

    byTestId("sound-choose").click();
    await vi.waitFor(() => expect(vi.mocked(api.setSoundfont)).toHaveBeenCalledWith(mine));
    await vi.waitFor(() =>
      expect(byTestId("sound-soundfont").textContent).toContain(mine),
    );
    expect(state.audio.soundfont).toBe(mine);

    await remount();
    expect(byTestId("sound-soundfont").textContent).toContain(mine);
    expect(byTestId<HTMLInputElement>("sound-clear").disabled).toBe(false);

    byTestId("sound-clear").click();
    await vi.waitFor(() => expect(vi.mocked(api.setSoundfont)).toHaveBeenLastCalledWith(null));
    await vi.waitFor(() =>
      expect(byTestId("sound-soundfont").textContent).toContain("bundled bank"),
    );
    expect(state.audio.soundfont).toBe(BUNDLED);
  });

  it("keeps the control honest when the volume write fails", async () => {
    await renderGroup();
    state.failNextWrite = true;

    await slide("sound-volume", 0.3);

    await vi.waitFor(() =>
      expect(document.querySelector(".prefs-error")?.textContent).toContain("disk is full"),
    );
    expect(byTestId<HTMLInputElement>("sound-volume").value).toBe("1");
    expect(byTestId("sound-volume-readout").textContent).toBe("100%");
    expect(state.audio.volume).toBe(1);
  });
});

describe("a SoundFont path that is not usable", () => {
  it("refuses a file that vanished before it could be saved, and stays on the engine's font", async () => {
    await renderGroup();
    state.pick = "/gone/deleted.sf2";

    byTestId("sound-choose").click();

    await vi.waitFor(() =>
      expect(document.querySelector(".prefs-error")?.textContent).toContain("does not exist"),
    );
    expect(state.audio.soundfont).toBeNull();
    expect(byTestId("sound-soundfont").textContent).toContain("fallback tone");
  });

  it("reports a stored file that has gone missing as the bundled-bank fallback", async () => {
    state.audio = {
      enabled: true,
      volume: 1,
      soundfont: BUNDLED,
      soundfont_exists: true,
      soundfont_fallback: true,
      soundfont_bundled: false,
    };

    await renderGroup();

    const note = byTestId("sound-soundfont");
    expect(note.textContent).toContain("missing");
    expect(note.textContent).toContain(BUNDLED);
    await tick();
    expect(document.querySelector(".prefs-error")).toBeNull();
    expect(store.audio.soundfont_fallback).toBe(true);

    byTestId("sound-clear").click();
    await vi.waitFor(() => expect(vi.mocked(api.setSoundfont)).toHaveBeenLastCalledWith(null));
    expect(state.audio.soundfont_fallback).toBe(false);
  });
});

describe("unsupported options", () => {
  it("renders the effects/music split and the speech voice disabled, marked and explained", async () => {
    await renderGroup();

    for (const [control, marker] of [
      ["sound-sfx-volume", "unsupported-sfx_volume"],
      ["sound-music-volume", "unsupported-music_volume"],
      ["sound-speech-voice", "unsupported-speech_voice"],
    ] as const) {
      expect(byTestId<HTMLInputElement | HTMLSelectElement>(control).disabled).toBe(true);
      const chip = byTestId(marker);
      expect(chip.textContent?.trim()).toBe("Not supported");
      const row = chip.closest(".prefs-option");
      expect(row?.querySelector(".prefs-note")?.textContent?.trim().length ?? 0).toBeGreaterThan(
        0,
      );
    }
  });

  it("writes nothing when an unsupported control is poked", async () => {
    await renderGroup();

    const sfx = byTestId<HTMLInputElement>("sound-sfx-volume");
    sfx.value = "0.2";
    sfx.dispatchEvent(new Event("input", { bubbles: true }));
    sfx.dispatchEvent(new Event("change", { bubbles: true }));
    const voice = byTestId<HTMLSelectElement>("sound-speech-voice");
    voice.value = "";
    voice.dispatchEvent(new Event("change", { bubbles: true }));
    await tick();

    expect(vi.mocked(api.setPrefs)).not.toHaveBeenCalled();
    expect(state.stored.sound).toBeUndefined();
  });
});

describe("the Preferences window shell", () => {
  it("renders the real Sound group when Sound is chosen", async () => {
    mounted = mount(PreferencesWindow, { target: document.body });
    await tick();
    await vi.waitFor(() =>
      expect(document.querySelectorAll(".prefs-nav-item").length).toBeGreaterThan(0),
    );

    const nav = [...document.querySelectorAll<HTMLButtonElement>(".prefs-nav-item")].find(
      (node) => node.textContent?.trim() === "Sound",
    );
    expect(nav).toBeDefined();
    nav?.click();
    await tick();

    expect(byTestId("sound-volume")).toBeTruthy();
    expect(byTestId("sound-mute")).toBeTruthy();
    expect(document.querySelector(".prefs-placeholder")).toBeNull();
    expect(document.querySelector(".prefs-content h1")?.textContent).toBe("Sound");
  });
});
