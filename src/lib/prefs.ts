/**
 * The frontend half of the Preferences keyspace contract.
 *
 * `PREFERENCES.md` (Task 5) is the authority for the groups and their names,
 * and `src-tauri/src/prefs.rs` mirrors this list for validation. Keeping the
 * list here means the shell's navigation, the placeholder copy and the
 * "last group" rule all read one array instead of three.
 *
 * This module is deliberately free of Tauri imports, so the rules it encodes
 * are unit-testable in a plain environment.
 */

/** A preference group's stable id, exactly the ten in `PREFERENCES.md`. */
export type PrefGroupId =
  | "connection"
  | "appearance"
  | "graphics"
  | "sound"
  | "chat_log"
  | "avatar"
  | "mute"
  | "notifications"
  | "layout"
  | "shell";

/** One preference group: the stable id used in storage, and its display label. */
export interface PrefGroup {
  id: PrefGroupId;
  label: string;
  /** A one-line description, used by the navigation and the unbuilt-group view. */
  summary: string;
}

/** The ten groups, in the order `PREFERENCES.md` lists them. */
export const PREF_GROUPS: readonly PrefGroup[] = [
  {
    id: "connection",
    label: "Connection & identity",
    summary: "The server, the user name and the identity this install signs in with.",
  },
  {
    id: "appearance",
    label: "Appearance",
    summary: "Theme, fonts, text size and the interface scale.",
  },
  {
    id: "graphics",
    label: "Room & graphics behaviour",
    summary: "Name tags, avatars, animation and balloon timing.",
  },
  {
    id: "sound",
    label: "Sound",
    summary: "The SoundFont and the audio output levels.",
  },
  {
    id: "chat_log",
    label: "Chat logging",
    summary: "Write the room transcript to a file, with a size cap and rotation.",
  },
  {
    id: "avatar",
    label: "Avatar & prop behaviour",
    summary: "Prop animation and saved avatar shortcut slots.",
  },
  {
    id: "mute",
    label: "Mute/ignore",
    summary: "Hide messages and motion from chosen users.",
  },
  {
    id: "notifications",
    label: "Notifications",
    summary: "Alerts for mentions and private messages.",
  },
  {
    id: "layout",
    label: "Layout memory",
    summary: "Window positions and which panels are detached.",
  },
  {
    id: "shell",
    label: "Preferences shell",
    summary: "How this window remembers the group you were last viewing.",
  },
];

/** The group a fresh install opens on. */
export const FIRST_GROUP_ID: PrefGroupId = "connection";

/** The open `prefs` block, as read from and written to the settings file. */
export type PrefsDocument = Record<string, unknown>;

const GROUP_IDS: readonly string[] = PREF_GROUPS.map((group) => group.id);

/** Whether `value` names one of the ten groups. */
export function isPrefGroupId(value: unknown): value is PrefGroupId {
  return typeof value === "string" && GROUP_IDS.includes(value);
}

/** The group a stored id names, or `null` when it names none. */
export function prefGroupById(id: string): PrefGroup | null {
  return PREF_GROUPS.find((group) => group.id === id) ?? null;
}

/** Read a value out of the nested `prefs` block, returning `fallback` when absent. */
export function readPref<T>(values: PrefsDocument, path: readonly string[], fallback: T): T {
  let cursor: unknown = values;
  for (const key of path) {
    if (typeof cursor !== "object" || cursor === null) {
      return fallback;
    }
    cursor = (cursor as Record<string, unknown>)[key];
  }
  return cursor === undefined ? fallback : (cursor as T);
}

/**
 * The group to open on, from `prefs.shell.last_group`.
 *
 * A missing or unrecognised value falls back to the first group, so a stale
 * value written by an older build can never leave the window blank.
 */
export function lastGroupFrom(values: PrefsDocument): PrefGroupId {
  const candidate = readPref<unknown>(values, ["shell", "last_group"], FIRST_GROUP_ID);
  return isPrefGroupId(candidate) ? candidate : FIRST_GROUP_ID;
}
