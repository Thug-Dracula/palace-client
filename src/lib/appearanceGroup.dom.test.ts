// @vitest-environment jsdom
/**
 * Task 21's acceptance tests: the Appearance preference group.
 *
 * The plan names three behaviours and each has a test here that can fail:
 *
 * * changing a colour token updates the rendered value live (no reload);
 * * a font family/size change applies live and survives a restart;
 * * the default appearance writes nothing inline, so the shipped look is
 *   byte-for-byte what it was (the stylesheet stays authoritative).
 *
 * The group is mounted directly: the shell routes the `appearance` group to it
 * with a one-line branch, so the group owns the whole save/apply path under
 * test here.
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
    failNextSetPrefs: false,
    merge,
    invoke: vi.fn(async () => true),
  };
});

vi.mock("@tauri-apps/api/core", () => ({ invoke: state.invoke }));

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
      return state.stored;
    }),
    resetPrefs: vi.fn(async () => {
      state.stored = {};
      return state.stored;
    }),
    setUiScale: vi.fn(async (scale: number) => scale),
    onEvent: vi.fn(async () => () => {}),
  };
});

import { mount, tick, unmount } from "svelte";

import * as api from "./api";
import { CRT_DARK_TOKENS, DEFAULT_FONT_FAMILY } from "./appearance";
import AppearanceGroup from "./components/prefs/AppearanceGroup.svelte";
import { prefs } from "./prefsStore.svelte";

let mounted: ReturnType<typeof mount> | undefined;

function storedAppearance(): {
  theme?: string;
  font_family?: string;
  font_size_px?: number;
  ui_scale?: number;
  fullscreen?: boolean;
  tokens?: Record<string, string>;
} {
  return (state.stored.appearance ?? {}) as {
    theme?: string;
    font_family?: string;
    font_size_px?: number;
    ui_scale?: number;
    fullscreen?: boolean;
    tokens?: Record<string, string>;
  };
}

function byTestId<T extends HTMLElement>(id: string): T {
  const node = document.querySelector<T>(`[data-testid="${id}"]`);
  if (!node) {
    throw new Error(`no element with data-testid ${id}`);
  }
  return node;
}

async function renderGroup(): Promise<void> {
  await prefs.load();
  mounted = mount(AppearanceGroup, { target: document.body });
  await tick();
}

async function changeInput(input: HTMLInputElement, value: string): Promise<void> {
  input.value = value;
  input.dispatchEvent(new Event("input", { bubbles: true }));
  input.dispatchEvent(new Event("change", { bubbles: true }));
  await tick();
}

beforeEach(() => {
  vi.clearAllMocks();
  state.stored = {};
  state.failNextSetPrefs = false;
  prefs.reset();
  document.body.innerHTML = "";
  document.documentElement.removeAttribute("style");
});

afterEach(() => {
  if (mounted) {
    unmount(mounted);
    mounted = undefined;
  }
  document.body.innerHTML = "";
  document.documentElement.removeAttribute("style");
});

describe("the default appearance", () => {
  it("writes nothing inline, so tokens.css keeps the shipped look", async () => {
    await renderGroup();

    expect(document.documentElement.style.getPropertyValue("--amber")).toBe("");
    expect(document.documentElement.style.getPropertyValue("--font-mono")).toBe("");
    expect(document.documentElement.style.getPropertyValue("--fs-md")).toBe("");
    expect(document.documentElement.getAttribute("style")).toBeNull();

    // The inputs still show the shipped values.
    expect(byTestId<HTMLInputElement>("font-family").value).toBe(DEFAULT_FONT_FAMILY);
    expect(byTestId<HTMLInputElement>("font-size").value).toBe("13");
    expect(byTestId<HTMLSelectElement>("appearance-theme").value).toBe("crt-dark");
  });
});

describe("colour tokens", () => {
  it("updates the rendered value live and persists it", async () => {
    await renderGroup();

    const swatch = byTestId<HTMLInputElement>("colour-amber");
    await changeInput(swatch, "#00ff00");

    await vi.waitFor(() =>
      expect(document.documentElement.style.getPropertyValue("--amber")).toBe("#00ff00"),
    );
    await vi.waitFor(() =>
      expect(storedAppearance().tokens?.amber).toBe("#00ff00"),
    );
    // A sibling token the user did not touch stays inline-free.
    expect(document.documentElement.style.getPropertyValue("--cyan")).toBe("");
  });

  it("resets one token back to the stylesheet's value", async () => {
    state.stored = { appearance: { tokens: { amber: "#00ff00" } } };
    await renderGroup();
    expect(document.documentElement.style.getPropertyValue("--amber")).toBe("#00ff00");

    const reset = byTestId<HTMLDivElement>("colour-row-amber").querySelector("button");
    expect(reset?.textContent?.trim()).toBe("Reset");
    reset?.click();

    await vi.waitFor(() =>
      expect(document.documentElement.style.getPropertyValue("--amber")).toBe(""),
    );
    expect(storedAppearance().tokens?.amber).toBe(CRT_DARK_TOKENS.amber);
  });

  it("keeps the rendered style unchanged when the write fails", async () => {
    await renderGroup();
    state.failNextSetPrefs = true;

    await changeInput(byTestId<HTMLInputElement>("colour-red"), "#123456");

    await vi.waitFor(() =>
      expect(document.querySelector(".prefs-error")?.textContent).toContain("disk is full"),
    );
    expect(document.documentElement.style.getPropertyValue("--red")).toBe("");
    expect(storedAppearance().tokens?.red).toBeUndefined();
  });
});

describe("font family and size", () => {
  it("applies live and persists across a restart", async () => {
    await renderGroup();

    await changeInput(byTestId<HTMLInputElement>("font-family"), "IBM Plex Mono");
    await vi.waitFor(() =>
      expect(document.documentElement.style.getPropertyValue("--font-mono")).toBe("IBM Plex Mono"),
    );
    await vi.waitFor(() => expect(storedAppearance().font_family).toBe("IBM Plex Mono"));

    await changeInput(byTestId<HTMLInputElement>("font-size"), "16");
    await vi.waitFor(() =>
      expect(document.documentElement.style.getPropertyValue("--fs-md")).toBe("16px"),
    );
    await vi.waitFor(() => expect(storedAppearance().font_size_px).toBe(16));

    // Restart: the same file, a fresh mount, the same rendered values.
    unmount(mounted as ReturnType<typeof mount>);
    mounted = undefined;
    document.documentElement.removeAttribute("style");
    prefs.reset();
    await renderGroup();

    expect(byTestId<HTMLInputElement>("font-family").value).toBe("IBM Plex Mono");
    expect(byTestId<HTMLInputElement>("font-size").value).toBe("16");
    expect(document.documentElement.style.getPropertyValue("--font-mono")).toBe("IBM Plex Mono");
    expect(document.documentElement.style.getPropertyValue("--fs-md")).toBe("16px");
  });

  it("clamps a text size out of range instead of saving it", async () => {
    await renderGroup();

    await changeInput(byTestId<HTMLInputElement>("font-size"), "99");

    await vi.waitFor(() => expect(storedAppearance().font_size_px).toBe(24));
    expect(document.documentElement.style.getPropertyValue("--fs-md")).toBe("24px");
  });

  it("treats an emptied family as the shipped stack", async () => {
    await renderGroup();

    await changeInput(byTestId<HTMLInputElement>("font-family"), "   ");

    await vi.waitFor(() => expect(storedAppearance().font_family).toBe(DEFAULT_FONT_FAMILY));
    expect(document.documentElement.style.getPropertyValue("--font-mono")).toBe("");
  });
});

describe("interface scale", () => {
  it("persists the value and applies it through the existing scale command", async () => {
    await renderGroup();

    await changeInput(byTestId<HTMLInputElement>("ui-scale"), "1.5");

    await vi.waitFor(() => expect(storedAppearance().ui_scale).toBe(1.5));
    await vi.waitFor(() => expect(vi.mocked(api.setUiScale)).toHaveBeenCalledWith(1.5));
    expect(byTestId<HTMLSpanElement>("ui-scale-readout").textContent).toBe("150%");
  });
});

describe("full screen", () => {
  it("persists the switch and applies it to the main window", async () => {
    await renderGroup();

    byTestId<HTMLInputElement>("fullscreen").click();

    await vi.waitFor(() => expect(storedAppearance().fullscreen).toBe(true));
    await vi.waitFor(() =>
      expect(state.invoke).toHaveBeenCalledWith("set_fullscreen", { fullscreen: true }),
    );
  });
});
