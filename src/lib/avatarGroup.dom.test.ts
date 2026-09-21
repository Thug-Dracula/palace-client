// @vitest-environment jsdom
/**
 * Task 22's acceptance tests: the Avatar & prop behaviour group.
 *
 * Neither option in this group has backing behaviour in this build. The tests
 * here exist to make the honest state permanent: every control is disabled and
 * visibly marked "Not supported", the stored value is only ever displayed, and
 * no interaction can write anything to the settings file.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const state = vi.hoisted(() => ({
  stored: {} as Record<string, unknown>,
  invoke: vi.fn(async () => true),
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke: state.invoke }));

vi.mock("./api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./api")>();
  return {
    ...actual,
    getPrefs: vi.fn(async () => state.stored),
    setPrefs: vi.fn(async (patch: Record<string, unknown>) => {
      state.stored = { ...state.stored, ...patch };
      return state.stored;
    }),
    resetPrefs: vi.fn(async () => state.stored),
    onEvent: vi.fn(async () => () => {}),
  };
});

import { mount, tick, unmount } from "svelte";

import * as api from "./api";
import {
  AVATAR_OPTIONS,
  UNSUPPORTED_AVATAR_KEYS,
  avatarPrefsFrom,
  isAvatarOptionSupported,
} from "./avatarPrefs";
import AvatarGroup from "./components/prefs/AvatarGroup.svelte";
import { prefs } from "./prefsStore.svelte";

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
  mounted = mount(AvatarGroup, { target: document.body });
  await tick();
}

beforeEach(() => {
  vi.clearAllMocks();
  state.stored = {};
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

describe("avatarPrefsFrom", () => {
  it("resolves the specification's defaults", () => {
    expect(avatarPrefsFrom({})).toEqual({ propAnimation: true, savedAvatarSlots: [] });
  });

  it("reads the stored keys and discards wrong types", () => {
    expect(
      avatarPrefsFrom({ avatar: { prop_animation: false, saved_avatar_slots: ["a", "b"] } }),
    ).toEqual({ propAnimation: false, savedAvatarSlots: ["a", "b"] });
    expect(
      avatarPrefsFrom({ avatar: { prop_animation: "no", saved_avatar_slots: [1, "b"] } }),
    ).toEqual({ propAnimation: true, savedAvatarSlots: ["b"] });
  });

  it("marks both options unsupported, with a reason", () => {
    expect(UNSUPPORTED_AVATAR_KEYS).toEqual(["prop_animation", "saved_avatar_slots"]);
    expect(AVATAR_OPTIONS).toHaveLength(2);
    for (const option of AVATAR_OPTIONS) {
      expect(option.support).toBe("unsupported");
      expect(option.reason?.trim().length ?? 0).toBeGreaterThan(0);
      expect(isAvatarOptionSupported(option.key)).toBe(false);
    }
  });
});

describe("the Avatar & prop behaviour group", () => {
  it("disables every control and marks it Not supported", async () => {
    await renderGroup();

    expect(byTestId<HTMLInputElement>("avatar-prop-animation").disabled).toBe(true);
    expect(byTestId<HTMLInputElement>("avatar-saved-slots").disabled).toBe(true);
    expect(byTestId<HTMLButtonElement>("avatar-save-slot").disabled).toBe(true);
    expect(byTestId<HTMLButtonElement>("avatar-clear-slots").disabled).toBe(true);

    for (const marker of ["unsupported-prop_animation", "unsupported-saved_avatar_slots"]) {
      const chip = byTestId(marker);
      expect(chip.textContent?.trim()).toBe("Not supported");
      expect(chip.closest(".prefs-option")?.querySelector(".prefs-note")?.textContent?.trim())
        .toBeTruthy();
    }
  });

  it("shows a stored unsupported value without offering to change it, and writes nothing", async () => {
    state.stored = { avatar: { prop_animation: false, saved_avatar_slots: ["a", "b"] } };
    await renderGroup();

    expect(byTestId<HTMLInputElement>("avatar-prop-animation").checked).toBe(false);
    expect(byTestId<HTMLInputElement>("avatar-saved-slots").value).toBe("2 stored slots");

    const animation = byTestId<HTMLInputElement>("avatar-prop-animation");
    animation.checked = true;
    animation.dispatchEvent(new Event("change", { bubbles: true }));
    byTestId<HTMLButtonElement>("avatar-save-slot").click();
    byTestId<HTMLButtonElement>("avatar-clear-slots").click();
    await tick();

    expect(vi.mocked(api.setPrefs)).not.toHaveBeenCalled();
    expect(state.stored).toEqual({
      avatar: { prop_animation: false, saved_avatar_slots: ["a", "b"] },
    });
  });

  it("keeps the defaults visible when the file has no avatar block", async () => {
    await renderGroup();

    expect(byTestId<HTMLInputElement>("avatar-prop-animation").checked).toBe(true);
    expect(byTestId<HTMLInputElement>("avatar-saved-slots").value).toBe("0 stored slots");
    expect(vi.mocked(api.setPrefs)).not.toHaveBeenCalled();
  });
});
