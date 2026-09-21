/**
 * The Notifications preference group's rules, independent of the window.
 *
 * `PREFERENCES.md` (Task 5) is the authority for this group's keys and their
 * defaults:
 *
 * | Key                            | Type    | Default |
 * |--------------------------------|---------|---------|
 * | `notifications.enabled`        | boolean | `false` |
 * | `notifications.on_mention`     | boolean | `true`  |
 * | `notifications.private_message`| boolean | `true`  |
 * | `notifications.sound`          | boolean | `false` |
 *
 * Desktop notifications are **opt-in**: `enabled` is `false` on a fresh
 * install, and the Rust side (`src-tauri/src/notify.rs`) is what actually
 * raises one. This module only resolves the stored block and turns the two
 * scope flags into the single "scope" the window shows.
 *
 * The three scope flags below are the only combinations of `on_mention` and
 * `private_message` the UI presents; `sound` is defined so the stored defaults
 * match the specification, but this build never plays a notification sound, so
 * it must be rendered disabled and marked, never as a control that appears to
 * work.
 */
import { readPref, type PrefsDocument } from "./prefs";

/** `notifications.enabled` default: notifications are opt-in, so off. */
export const DEFAULT_NOTIFICATIONS_ENABLED = false;

/** `notifications.on_mention` default. */
export const DEFAULT_ON_MENTION = true;

/** `notifications.private_message` default. */
export const DEFAULT_PRIVATE_MESSAGE = true;

/** `notifications.sound` default. */
export const DEFAULT_NOTIFICATION_SOUND = false;

/** The notifications settings, resolved against the defaults. */
export interface NotificationPrefs {
  enabled: boolean;
  onMention: boolean;
  privateMessage: boolean;
  sound: boolean;
}

/** The one scope control, collapsed from the two stored flags. */
export type NotificationScope = "mentions" | "private" | "all" | "none";

/** A boolean from a stored value; a missing or non-boolean value takes `fallback`. */
function booleanOr(value: unknown, fallback: boolean): boolean {
  return typeof value === "boolean" ? value : fallback;
}

/** Resolve the stored notifications block, falling back to the shipped defaults. */
export function notificationsPrefsFrom(values: PrefsDocument): NotificationPrefs {
  return {
    enabled: booleanOr(
      readPref<unknown>(values, ["notifications", "enabled"], DEFAULT_NOTIFICATIONS_ENABLED),
      DEFAULT_NOTIFICATIONS_ENABLED,
    ),
    onMention: booleanOr(
      readPref<unknown>(values, ["notifications", "on_mention"], DEFAULT_ON_MENTION),
      DEFAULT_ON_MENTION,
    ),
    privateMessage: booleanOr(
      readPref<unknown>(values, ["notifications", "private_message"], DEFAULT_PRIVATE_MESSAGE),
      DEFAULT_PRIVATE_MESSAGE,
    ),
    sound: booleanOr(
      readPref<unknown>(values, ["notifications", "sound"], DEFAULT_NOTIFICATION_SOUND),
      DEFAULT_NOTIFICATION_SOUND,
    ),
  };
}

/** The scope a stored pair of flags expresses. */
export function notificationScopeFrom(prefs: NotificationPrefs): NotificationScope {
  if (prefs.onMention && prefs.privateMessage) {
    return "all";
  }
  if (prefs.onMention) {
    return "mentions";
  }
  if (prefs.privateMessage) {
    return "private";
  }
  return "none";
}

/** The stored flags a chosen scope means. */
export function scopePatch(scope: NotificationScope): {
  on_mention: boolean;
  private_message: boolean;
} {
  return {
    on_mention: scope === "mentions" || scope === "all",
    private_message: scope === "private" || scope === "all",
  };
}

/** One selectable scope, in display order. */
export interface NotificationScopeOption {
  id: NotificationScope;
  label: string;
  summary: string;
}

/** The scopes the window offers; `none` is not offered because it would just
 * turn notifications off, which the master toggle already does. */
export const NOTIFICATION_SCOPE_OPTIONS: readonly NotificationScopeOption[] = [
  {
    id: "private",
    label: "Direct messages only",
    summary: "Notify only for whispers sent to you.",
  },
  {
    id: "mentions",
    label: "Mentions only",
    summary: "Notify only when a room message names you.",
  },
  {
    id: "all",
    label: "Mentions and direct messages",
    summary: "Notify for whispers and for messages that name you.",
  },
];

/** Whether a chosen scope is accepted as a stored value. */
export function isNotificationScope(value: unknown): value is NotificationScope {
  return (
    value === "mentions" ||
    value === "private" ||
    value === "all" ||
    value === "none"
  );
}

/** Whether an option's effect is actually implemented by this build. */
export type OptionSupport = "supported" | "unsupported";

/** One option in the Notifications group, and whether this build acts on it. */
export interface NotificationOption {
  /** The stable option name, matching the key after `prefs.notifications.`. */
  key: "sound";
  /** The human label shown next to the control. */
  label: string;
  /** One line explaining what the option would do. */
  summary: string;
  support: OptionSupport;
  /** Why the option is not wired up. `null` when it is supported. */
  reason: string | null;
}

/**
 * The options this group stores but cannot back.
 *
 * Anything with `support: "unsupported"` must be rendered disabled and carry a
 * visible "Not supported" marker with its `reason`; it must not be written.
 */
export const NOTIFICATION_OPTIONS: readonly NotificationOption[] = [
  {
    key: "sound",
    label: "Play a sound",
    summary: "Ask the system to play its notification sound.",
    support: "unsupported",
    reason:
      "The desktop notification service plays its own sound (or none) and exposes no way for this build to choose one, so the option cannot be honoured.",
  },
];

/** The table entry for one option key, or `null` when the key is unknown. */
export function notificationOption(key: string): NotificationOption | null {
  return NOTIFICATION_OPTIONS.find((option) => option.key === key) ?? null;
}

/** Whether this build acts on the option named `key` (unknown keys are not supported). */
export function isNotificationOptionSupported(key: string): boolean {
  return notificationOption(key)?.support === "supported";
}
