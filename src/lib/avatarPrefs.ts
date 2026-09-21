/**
 * The Avatar & prop behaviour preference group's rules.
 *
 * `PREFERENCES.md` (Task 5) defines two keys, and neither has backing behaviour
 * in this build:
 *
 * | Key                            | Why it is not supported                         |
 * |--------------------------------|-------------------------------------------------|
 * | `avatar.prop_animation`        | same still-frame renderer as `graphics.animation` |
 * | `avatar.saved_avatar_slots`    | there is no saved-avatar store to slot into     |
 *
 * Both are still defined here so the stored defaults match the specification
 * and a later task can take them over, but {@link AVATAR_OPTIONS} marks both
 * `unsupported`: the group must render them disabled, with a visible reason,
 * and must never write them. A control that appears to work but does nothing is
 * exactly what this module exists to prevent.
 */
import { readPref, type PrefsDocument } from "./prefs";

/** `avatar.prop_animation` default, stored for the future; not acted on yet. */
export const DEFAULT_PROP_ANIMATION = true;

/** `avatar.saved_avatar_slots` default: no slots saved. */
export const DEFAULT_SAVED_AVATAR_SLOTS: readonly string[] = [];

/** The avatar settings, resolved against the defaults. */
export interface AvatarPrefs {
  propAnimation: boolean;
  savedAvatarSlots: readonly string[];
}

/** Resolve the stored avatar block, falling back to the shipped defaults. */
export function avatarPrefsFrom(values: PrefsDocument): AvatarPrefs {
  const rawAnimation = readPref<unknown>(
    values,
    ["avatar", "prop_animation"],
    DEFAULT_PROP_ANIMATION,
  );
  const rawSlots = readPref<unknown>(
    values,
    ["avatar", "saved_avatar_slots"],
    DEFAULT_SAVED_AVATAR_SLOTS,
  );
  return {
    propAnimation:
      typeof rawAnimation === "boolean" ? rawAnimation : DEFAULT_PROP_ANIMATION,
    savedAvatarSlots: Array.isArray(rawSlots)
      ? rawSlots.filter((slot): slot is string => typeof slot === "string")
      : DEFAULT_SAVED_AVATAR_SLOTS,
  };
}

/** Whether an option's effect is actually implemented by this build. */
export type OptionSupport = "supported" | "unsupported";

/** One option in the Avatar & prop behaviour group. */
export interface AvatarOption {
  /** The stable option name, matching the key after `prefs.avatar.`. */
  key: "prop_animation" | "saved_avatar_slots";
  label: string;
  summary: string;
  support: OptionSupport;
  /** Why the option is not wired up. `null` when it is supported. */
  reason: string | null;
}

/**
 * The group's options in display order.
 *
 * Both are `unsupported` today. The UI derives its disabled state and its
 * "Not supported" marker from this table, so enabling an option later means
 * changing this one row — not hunting for a control.
 */
export const AVATAR_OPTIONS: readonly AvatarOption[] = [
  {
    key: "prop_animation",
    label: "Animate props",
    summary: "Play animated props in the room view.",
    support: "unsupported",
    reason: "This build draws one still frame per prop; there is no multi-frame playback yet.",
  },
  {
    key: "saved_avatar_slots",
    label: "Saved avatar slots",
    summary: "Keep shortcut slots for quickly switching avatars.",
    support: "unsupported",
    reason: "Saved avatar slots are not stored by this build yet; use Choose an Avatar instead.",
  },
];

/** The keys of this group's options that have no backing behaviour. */
export const UNSUPPORTED_AVATAR_KEYS: readonly string[] = AVATAR_OPTIONS.filter(
  (option) => option.support === "unsupported",
).map((option) => option.key);

/** The table entry for one option key, or `null` when the key is unknown. */
export function avatarOption(key: string): AvatarOption | null {
  return AVATAR_OPTIONS.find((option) => option.key === key) ?? null;
}

/** Whether this build acts on the option named `key` (unknown keys are not supported). */
export function isAvatarOptionSupported(key: string): boolean {
  return avatarOption(key)?.support === "supported";
}
