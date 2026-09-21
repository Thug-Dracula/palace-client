/**
 * Task 22's pure rules for the Room & graphics group.
 *
 * The defaults, the validation and the guest predicate are all independent of
 * any window, so they are locked here; the DOM half (persist + apply live,
 * unsupported controls) lives in `graphicsGroup.dom.test.ts`. The key names are
 * asserted against the literal strings from `PREFERENCES.md`, so a rename
 * cannot drift away from the shared settings file unnoticed.
 */
import { describe, expect, it } from "vitest";

import {
  DEFAULT_ANIMATION,
  DEFAULT_BALLOON_DELAY,
  DEFAULT_SHOW_AVATARS,
  DEFAULT_SHOW_GUESTS,
  DEFAULT_SHOW_NAMES,
  GRAPHICS_OPTIONS,
  UNSUPPORTED_GRAPHICS_KEYS,
  graphicsOption,
  graphicsPrefsFrom,
  isBalloonDelay,
  isGraphicsOptionSupported,
  isGuestName,
} from "./graphicsPrefs";

describe("graphicsPrefsFrom", () => {
  it("returns the specification's defaults for an empty document", () => {
    expect(graphicsPrefsFrom({})).toEqual({
      showNames: true,
      showAvatars: true,
      showGuests: true,
      animation: true,
      tintedBalloons: false,
      balloonDelay: "medium",
    });
    expect(DEFAULT_SHOW_NAMES).toBe(true);
    expect(DEFAULT_SHOW_AVATARS).toBe(true);
    expect(DEFAULT_SHOW_GUESTS).toBe(true);
    expect(DEFAULT_ANIMATION).toBe(true);
    expect(DEFAULT_BALLOON_DELAY).toBe("medium");
  });

  it("reads every stored key by its PREFERENCES.md name", () => {
    const values = {
      graphics: {
        show_names: false,
        show_avatars: false,
        show_guests: false,
        animation: false,
        tinted_balloons: true,
        balloon_delay: "fast",
      },
    };
    expect(graphicsPrefsFrom(values)).toEqual({
      showNames: false,
      showAvatars: false,
      showGuests: false,
      animation: false,
      tintedBalloons: true,
      balloonDelay: "fast",
    });
  });

  it("falls back per key when a stored value has the wrong type", () => {
    const values = {
      graphics: {
        show_names: "yes",
        show_avatars: 0,
        show_guests: null,
        animation: "on",
        tinted_balloons: 1,
        balloon_delay: "instant",
      },
    };
    expect(graphicsPrefsFrom(values)).toEqual(graphicsPrefsFrom({}));
  });

  it("accepts only slow, medium and fast as a balloon delay", () => {
    for (const delay of ["slow", "medium", "fast"]) {
      expect(isBalloonDelay(delay)).toBe(true);
      expect(
        graphicsPrefsFrom({ graphics: { balloon_delay: delay } }).balloonDelay,
      ).toBe(delay);
    }
    expect(isBalloonDelay("instant")).toBe(false);
    expect(isBalloonDelay("")).toBe(false);
    expect(isBalloonDelay(3)).toBe(false);
    expect(isBalloonDelay(null)).toBe(false);
  });

  it("ignores a graphics block that is not an object", () => {
    expect(graphicsPrefsFrom({ graphics: "nope" })).toEqual(graphicsPrefsFrom({}));
  });
});

describe("isGuestName", () => {
  it("matches the guest names the classic client hides", () => {
    for (const name of ["Guest", "guest", "Guest1234", "Guest of Honour", "guest_42", " GUEST "]) {
      expect(isGuestName(name), name).toBe(true);
    }
  });

  it("does not match names that merely start with the same letters", () => {
    for (const name of ["Gustav", "Guesthouse", "G", "", "Ada", "Reguest"]) {
      expect(isGuestName(name), name).toBe(false);
    }
  });
});

describe("the support table", () => {
  it("marks exactly the backed options supported", () => {
    const supported = GRAPHICS_OPTIONS.filter((option) => option.support === "supported").map(
      (option) => option.key,
    );
    expect(supported).toEqual(["show_names", "show_avatars", "show_guests"]);
    expect(isGraphicsOptionSupported("show_names")).toBe(true);
    expect(isGraphicsOptionSupported("show_avatars")).toBe(true);
    expect(isGraphicsOptionSupported("show_guests")).toBe(true);
  });

  it("marks every unbacked option unsupported and explains why", () => {
    expect(UNSUPPORTED_GRAPHICS_KEYS).toEqual(["animation", "tinted_balloons", "balloon_delay"]);
    for (const key of UNSUPPORTED_GRAPHICS_KEYS) {
      const option = graphicsOption(key);
      expect(option?.support).toBe("unsupported");
      expect(option?.reason?.trim().length ?? 0).toBeGreaterThan(0);
      expect(isGraphicsOptionSupported(key)).toBe(false);
    }
    expect(UNSUPPORTED_GRAPHICS_KEYS).not.toContain("show_names");
    expect(UNSUPPORTED_GRAPHICS_KEYS).not.toContain("show_avatars");
    expect(UNSUPPORTED_GRAPHICS_KEYS).not.toContain("show_guests");
  });

  it("treats an unknown key as unsupported rather than assuming it works", () => {
    expect(graphicsOption("balloon_speed")).toBeNull();
    expect(isGraphicsOptionSupported("balloon_speed")).toBe(false);
  });

  it("names the six keys from PREFERENCES.md, without duplicates", () => {
    const keys = GRAPHICS_OPTIONS.map((option) => option.key);
    expect(keys).toEqual([
      "show_names",
      "show_avatars",
      "show_guests",
      "animation",
      "tinted_balloons",
      "balloon_delay",
    ]);
    expect(new Set(keys).size).toBe(keys.length);
  });
});
