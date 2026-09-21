/**
 * The Mute/ignore preference group's rules, independent of the window.
 *
 * `PREFERENCES.md` (Task 5) is the authority for this group's keys and their
 * defaults:
 *
 * | Key                       | Type             | Default |
 * |---------------------------|------------------|---------|
 * | `mute.ignore_all`         | boolean          | `false` |
 * | `mute.identities`         | array of objects | `[]`    |
 *
 * Each entry is `{name, identity}`. This build mints the identity from the
 * typed name (case-folded) because the protocol sends no stable per-user
 * identity: the user id is assigned per session and is reused by the server,
 * so it would silently mute the wrong person after a restart. A future build
 * (or a hand-edited file) may store a real identity, and matching uses it too.
 *
 * Suppression is **client-side only**: nothing here sends a command to the
 * server, and the list lives in this install's settings file. The classic
 * client had no persistent list at all — only a session mute from an avatar's
 * menu — which is why the group's UI says this is an enhancement.
 *
 * The module is deliberately free of Tauri imports so the rules are
 * unit-testable in a plain environment.
 */
import { readPref, type PrefsDocument } from "./prefs";

/** The `prefs.mute.ignore_all` key, as it appears in `settings.json`. */
export const MUTE_IGNORE_ALL_KEY = "ignore_all";

/** The `prefs.mute.identities` key, as it appears in `settings.json`. */
export const MUTE_IDENTITIES_KEY = "identities";

/** `prefs.mute.ignore_all` default: an untouched install ignores nobody. */
export const DEFAULT_IGNORE_ALL = false;

/**
 * The event Rust broadcasts whenever the `prefs` block is written.
 *
 * Every webview is a peer view with its own store, so a window that never
 * opened Preferences only learns about a new ignore list from this signal.
 */
export const PREFS_CHANGED_EVENT = "palace://prefs";

/** One ignored user: the display name, plus the identity used to match them. */
export interface MuteEntry {
  /** The user name as it was typed or shown. */
  name: string;
  /** The match key; empty when an entry has only a name. */
  identity: string;
}

/** The mute settings, resolved against the defaults. */
export interface MutePrefs {
  ignoreAll: boolean;
  identities: MuteEntry[];
}

/** Fold a name to its match key: trimmed and lower-cased. */
export function normaliseMuteName(value: string): string {
  return value.trim().toLowerCase();
}

/**
 * Read one stored entry, or `null` when the record cannot be used.
 *
 * A stored entry the user cannot mean — a non-object, or one with no usable
 * name — is dropped rather than repaired, so a hand-edited file can never
 * produce a rule that hides a user whose name was never given.
 */
export function muteEntryFrom(value: unknown): MuteEntry | null {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return null;
  }
  const record = value as Record<string, unknown>;
  const rawName = record.name;
  if (typeof rawName !== "string" || rawName.trim() === "") {
    return null;
  }
  const rawIdentity = record.identity;
  return {
    name: rawName.trim(),
    identity: typeof rawIdentity === "string" ? rawIdentity.trim() : "",
  };
}

/** Drop repeated entries, keeping the first, so the UI cannot show twins. */
function dedupe(entries: readonly MuteEntry[]): MuteEntry[] {
  const seen = new Set<string>();
  const unique: MuteEntry[] = [];
  for (const entry of entries) {
    const key = normaliseMuteName(entry.name);
    if (!seen.has(key)) {
      seen.add(key);
      unique.push(entry);
    }
  }
  return unique;
}

/** Resolve the stored mute block, falling back to the shipped defaults. */
export function mutePrefsFrom(values: PrefsDocument): MutePrefs {
  const rawAll = readPref<unknown>(values, ["mute", MUTE_IGNORE_ALL_KEY], DEFAULT_IGNORE_ALL);
  const rawList = readPref<unknown>(values, ["mute", MUTE_IDENTITIES_KEY], []);
  const entries: MuteEntry[] = [];
  if (Array.isArray(rawList)) {
    for (const item of rawList) {
      const entry = muteEntryFrom(item);
      if (entry) {
        entries.push(entry);
      }
    }
  }
  return {
    ignoreAll: typeof rawAll === "boolean" ? rawAll : DEFAULT_IGNORE_ALL,
    identities: dedupe(entries),
  };
}

/** The result of adding a name: the next rules, or why the name was refused. */
export type AddMuteResult =
  | { ok: true; prefs: MutePrefs }
  | { ok: false; error: string };

/**
 * Add a user, rejecting a blank name and a duplicate.
 *
 * The name is stored trimmed; the identity defaults to the folded name, which
 * is the key later messages are matched on. Nothing is mutated: the caller
 * gets the next rules and decides whether to persist them.
 */
export function addMuteEntry(
  prefs: MutePrefs,
  rawName: string,
  rawIdentity = "",
): AddMuteResult {
  const name = rawName.trim();
  if (name === "") {
    return { ok: false, error: "Type a user name before adding it to the ignore list." };
  }
  const key = normaliseMuteName(name);
  if (prefs.identities.some((entry) => normaliseMuteName(entry.name) === key)) {
    return { ok: false, error: `${name} is already on the ignore list.` };
  }
  const identity = rawIdentity.trim() === "" ? key : rawIdentity.trim();
  return { ok: true, prefs: { ...prefs, identities: [...prefs.identities, { name, identity }] } };
}

/** Remove every entry whose name matches `name`, case-insensitively. */
export function removeMuteEntry(prefs: MutePrefs, name: string): MutePrefs {
  const key = normaliseMuteName(name);
  return {
    ...prefs,
    identities: prefs.identities.filter((entry) => normaliseMuteName(entry.name) !== key),
  };
}

/**
 * Whether `name` is ignored.
 *
 * A name matches when it equals an entry's name or its identity, both folded,
 * so `Bob`, `bob` and an entry added as `Julia` with identity `julia` all
 * behave as the user expects. With "Ignore all" on, everyone matches.
 */
export function isMutedUser(prefs: MutePrefs, name: string): boolean {
  if (prefs.ignoreAll) {
    return true;
  }
  const key = normaliseMuteName(name);
  if (key === "") {
    return false;
  }
  return prefs.identities.some(
    (entry) =>
      normaliseMuteName(entry.name) === key ||
      (entry.identity !== "" && normaliseMuteName(entry.identity) === key),
  );
}

/** The identity a chat line or roster entry carries into the check. */
export interface MuteSubject {
  name: string;
  /** The session user id, when the caller has one. */
  user_id?: number;
  /** The chat kind; server notices (`system`/`error`) are never hidden. */
  kind?: string;
}

/** The signed-in user, so their own lines are never hidden from them. */
export interface MuteSelf {
  name?: string;
  user_id?: number;
}

/**
 * Whether a line must be dropped from this window's chat mirror.
 *
 * Three rules, in order: the server's own notices are never hidden (they are
 * not a user's messages), the signed-in user is never hidden from themselves,
 * and everyone else is checked against the list.
 */
export function suppressesMessage(
  prefs: MutePrefs,
  message: MuteSubject,
  self: MuteSelf = {},
): boolean {
  if (message.kind === "system" || message.kind === "error") {
    return false;
  }
  if (self.user_id !== undefined && message.user_id === self.user_id) {
    return false;
  }
  if (self.name !== undefined && normaliseMuteName(self.name) === normaliseMuteName(message.name)) {
    return false;
  }
  return isMutedUser(prefs, message.name);
}

/** Whether an option's effect is actually implemented by this build. */
export type MuteSupport = "supported" | "unsupported";

/** One capability of the ignore feature, and whether this build has it. */
export interface MuteCapability {
  /** A stable capability name, used as the test id suffix. */
  key: "message_hiding" | "name_matching" | "account_identity" | "server_mute" | "motion";
  /** The human label shown next to the marker. */
  label: string;
  /** One line explaining what the capability would do. */
  summary: string;
  support: MuteSupport;
  /** Why the capability is absent. `null` when it is supported. */
  reason: string | null;
}

/**
 * What this build does, and what it deliberately does not.
 *
 * The group renders every entry from this table, so the copy cannot drift from
 * the rules. Anything `unsupported` is shown marked, never as a control that
 * appears to work.
 */
export const MUTE_CAPABILITIES: readonly MuteCapability[] = [
  {
    key: "message_hiding",
    label: "Hide their messages here",
    summary: "Messages from an ignored user are dropped from every chat view in this window.",
    support: "supported",
    reason: null,
  },
  {
    key: "name_matching",
    label: "Match by user name",
    summary: "An entry matches the person whose user name it carries, ignoring letter case.",
    support: "supported",
    reason: null,
  },
  {
    key: "account_identity",
    label: "Match by account identity",
    summary: "Follow a person even when they change their user name.",
    support: "unsupported",
    reason:
      "This protocol hands out a fresh session id on every connect and sends no stable account id, so a stored id could mute the wrong person after a restart. Entries match by name.",
  },
  {
    key: "server_mute",
    label: "Tell the server to mute them",
    summary: "A server-side mute would apply to every client on the server.",
    support: "unsupported",
    reason:
      "No mute command is sent to the server. The classic client's mute was session-only and local too, and this list never leaves this computer.",
  },
  {
    key: "motion",
    label: "Hide their avatar and motion",
    summary: "Stop drawing the ignored user's avatar in the room view.",
    support: "unsupported",
    reason: "Their avatar is still drawn; this build hides messages only.",
  },
];

/** The table entry for one capability, or `null` when the key is unknown. */
export function muteCapability(key: string): MuteCapability | null {
  return MUTE_CAPABILITIES.find((capability) => capability.key === key) ?? null;
}

/** Whether this build implements the capability named `key`. */
export function isMuteCapabilitySupported(key: string): boolean {
  return muteCapability(key)?.support === "supported";
}
