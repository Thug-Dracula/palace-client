/**
 * The Room & graphics preference group's rules, independent of the window.
 *
 * `PREFERENCES.md` (Task 5) is the authority for this group's keys and their
 * defaults. Six options live here, but only three have backing behaviour in
 * this build:
 *
 * | Key                          | Backing behaviour                                  |
 * |------------------------------|----------------------------------------------------|
 * | `graphics.show_names`        | yes — `set_visibility` re-composes the room frame   |
 * | `graphics.show_avatars`      | yes — same command                                  |
 * | `graphics.show_guests`       | yes — the user-list filter ({@link isGuestName})    |
 * | `graphics.animation`         | no — the renderer draws a still frame per prop      |
 * | `graphics.tinted_balloons`   | no — bubbles are not tinted per speaker             |
 * | `graphics.balloon_delay`     | no — bubbles are not clock-expired                  |
 *
 * The three unbacked options are still defined (so the stored defaults match
 * the specification and a future task can take them over), but the UI must
 * render them disabled and clearly marked, never as controls that appear to
 * work. {@link GRAPHICS_OPTIONS} is the one table that says which is which, so
 * the component cannot drift from this module.
 */
import { readPref, type PrefsDocument } from "./prefs";

/** `graphics.show_names` default, matching the in-memory `store.showNames`. */
export const DEFAULT_SHOW_NAMES = true;

/** `graphics.show_avatars` default, matching the in-memory `store.showAvatars`. */
export const DEFAULT_SHOW_AVATARS = true;

/** `graphics.show_guests` default. Classic PalaceChat showed guests in the list. */
export const DEFAULT_SHOW_GUESTS = true;

/** `graphics.animation` default. */
export const DEFAULT_ANIMATION = true;

/** `graphics.tinted_balloons` default. */
export const DEFAULT_TINTED_BALLOONS = false;

/** Every `graphics.balloon_delay` value this build accepts, slowest first. */
export const BALLOON_DELAYS = ["slow", "medium", "fast"] as const;

/** One accepted balloon delay. */
export type BalloonDelay = (typeof BALLOON_DELAYS)[number];

/** `graphics.balloon_delay` default. */
export const DEFAULT_BALLOON_DELAY: BalloonDelay = "medium";

/** The graphics settings, resolved against the defaults. */
export interface GraphicsPrefs {
  showNames: boolean;
  showAvatars: boolean;
  showGuests: boolean;
  animation: boolean;
  tintedBalloons: boolean;
  balloonDelay: BalloonDelay;
}

/** Whether `value` names an accepted balloon delay. */
export function isBalloonDelay(value: unknown): value is BalloonDelay {
  return typeof value === "string" && (BALLOON_DELAYS as readonly string[]).includes(value);
}

/** A boolean from a stored value; a missing or non-boolean value takes `fallback`. */
function booleanOr(value: unknown, fallback: boolean): boolean {
  return typeof value === "boolean" ? value : fallback;
}

/** Resolve the stored graphics block, falling back to the shipped defaults. */
export function graphicsPrefsFrom(values: PrefsDocument): GraphicsPrefs {
  const rawDelay = readPref<unknown>(values, ["graphics", "balloon_delay"], DEFAULT_BALLOON_DELAY);
  return {
    showNames: booleanOr(
      readPref<unknown>(values, ["graphics", "show_names"], DEFAULT_SHOW_NAMES),
      DEFAULT_SHOW_NAMES,
    ),
    showAvatars: booleanOr(
      readPref<unknown>(values, ["graphics", "show_avatars"], DEFAULT_SHOW_AVATARS),
      DEFAULT_SHOW_AVATARS,
    ),
    showGuests: booleanOr(
      readPref<unknown>(values, ["graphics", "show_guests"], DEFAULT_SHOW_GUESTS),
      DEFAULT_SHOW_GUESTS,
    ),
    animation: booleanOr(
      readPref<unknown>(values, ["graphics", "animation"], DEFAULT_ANIMATION),
      DEFAULT_ANIMATION,
    ),
    tintedBalloons: booleanOr(
      readPref<unknown>(values, ["graphics", "tinted_balloons"], DEFAULT_TINTED_BALLOONS),
      DEFAULT_TINTED_BALLOONS,
    ),
    balloonDelay: isBalloonDelay(rawDelay) ? rawDelay : DEFAULT_BALLOON_DELAY,
  };
}

/**
 * Whether a user name is a guest name, for the "Show guests in user list"
 * filter.
 *
 * The classic client treated a name beginning with `Guest` as a guest, because
 * the server mints `Guest`, `Guest1234` and similar names for anonymous
 * visitors. The match is the word `guest` followed by a digit, a separator or
 * the end of the name, case-insensitive and after trimming, so `Guest`,
 * `guest1234` and `Guest of Honour` are guests while `Guesthouse` and `Ada` are
 * not. Names are only display labels here; nothing is sent to the server
 * because of this predicate.
 */
const GUEST_NAME = /^guest(\d|[\s_\-.]|$)/;

export function isGuestName(name: string): boolean {
  return GUEST_NAME.test(name.trim().toLowerCase());
}

/** Whether an option's effect is actually implemented by this build. */
export type OptionSupport = "supported" | "unsupported";

/** One option in the Room & graphics group, and whether this build acts on it. */
export interface GraphicsOption {
  /** The stable option name, matching the key after `prefs.graphics.`. */
  key: "show_names" | "show_avatars" | "show_guests" | "animation" | "tinted_balloons" | "balloon_delay";
  /** The human label shown next to the control. */
  label: string;
  /** One line explaining what the option does. */
  summary: string;
  support: OptionSupport;
  /** Why the option is not wired up. `null` when it is supported. */
  reason: string | null;
}

/**
 * The group's options in display order.
 *
 * Anything with `support: "unsupported"` must be rendered disabled and carry a
 * visible "Not supported" marker with its `reason`; it must not be written.
 */
export const GRAPHICS_OPTIONS: readonly GraphicsOption[] = [
  {
    key: "show_names",
    label: "Show names",
    summary: "Draw every user's name tag in the room view.",
    support: "supported",
    reason: null,
  },
  {
    key: "show_avatars",
    label: "Show avatars",
    summary: "Draw every user's avatar in the room view.",
    support: "supported",
    reason: null,
  },
  {
    key: "show_guests",
    label: "Show guests in the user list",
    summary: "Include anonymous Guest visitors in the In room list.",
    support: "supported",
    reason: null,
  },
  {
    key: "animation",
    label: "Allow animation",
    summary: "Play animated props rather than a still frame.",
    support: "unsupported",
    reason: "This build draws one still frame per prop; there is no multi-frame playback yet.",
  },
  {
    key: "tinted_balloons",
    label: "Tinted balloons",
    summary: "Tint each speech bubble with the speaker's colour.",
    support: "unsupported",
    reason: "Speech bubbles are not tinted per speaker in this build.",
  },
  {
    key: "balloon_delay",
    label: "Balloon delay",
    summary: "How long a speech bubble stays on screen.",
    support: "unsupported",
    reason: "Speech bubbles are not clock-expired in this build, so no delay is honoured.",
  },
];

/** The keys of this group's options that have no backing behaviour. */
export const UNSUPPORTED_GRAPHICS_KEYS: readonly string[] = GRAPHICS_OPTIONS.filter(
  (option) => option.support === "unsupported",
).map((option) => option.key);

/** The table entry for one option key, or `null` when the key is unknown. */
export function graphicsOption(key: string): GraphicsOption | null {
  return GRAPHICS_OPTIONS.find((option) => option.key === key) ?? null;
}

/** Whether this build acts on the option named `key` (unknown keys are not supported). */
export function isGraphicsOptionSupported(key: string): boolean {
  return graphicsOption(key)?.support === "supported";
}
