/**
 * The Chat logging preference group's rules, independent of the window.
 *
 * `PREFERENCES.md` (Task 5) is the authority for this group's keys and their
 * defaults. The group has **two capabilities** and they must not be conflated:
 *
 * 1. **The in-memory log** is the backend transcript (Task 10's scrollback,
 *    capped at {@link IN_MEMORY_LOG_LINES} lines and replayed to a window that
 *    opens late). It is always on, it needs no preference key, and it must keep
 *    working with file logging off.
 * 2. **Saving the transcript to a file** is this group's four keys:
 *
 * | Key                  | Backing behaviour                                       |
 * |----------------------|---------------------------------------------------------|
 * | `chat_log.to_file`   | yes — turns the backend transcript writer on and off     |
 * | `chat_log.path`      | yes — the chosen destination (`null` = the default file) |
 * | `chat_log.max_bytes` | yes — rotates once the active file reaches the cap       |
 * | `chat_log.rotate_files` | yes — how many archives are kept                     |
 *
 * Every option here is backed, so there is no "Not supported" marker in this
 * group; {@link CHAT_LOG_OPTIONS} still names them all, so the component cannot
 * invent a control the backend does not honour.
 */
import { readPref, type PrefsDocument } from "./prefs";

/** How many transcript lines the backend keeps in memory, from `CHAT_SCROLLBACK_CAP`. */
export const IN_MEMORY_LOG_LINES = 500;

/** `chat_log.to_file` default: nothing is written until the user asks. */
export const DEFAULT_TO_FILE = false;

/** `chat_log.path` default: `null` means the default transcript location. */
export const DEFAULT_PATH: string | null = null;

/** `chat_log.max_bytes` default: 8 MiB. */
export const DEFAULT_MAX_BYTES = 8 * 1024 * 1024;

/** Smallest cap this build accepts (4 KiB). */
export const MIN_MAX_BYTES = 4 * 1024;

/** Largest cap this build accepts (1 GiB). */
export const MAX_MAX_BYTES = 1024 * 1024 * 1024;

/** `chat_log.rotate_files` default: keep three archives. */
export const DEFAULT_ROTATE_FILES = 3;

/** Fewest archives kept; `0` truncates the active file instead. */
export const MIN_ROTATE_FILES = 0;

/** Most archives kept. */
export const MAX_ROTATE_FILES = 20;

/** The chat logging settings, resolved against the defaults. */
export interface ChatLogPrefs {
  toFile: boolean;
  path: string | null;
  maxBytes: number;
  rotateFiles: number;
}

/** A boolean from a stored value; a missing or non-boolean value takes `fallback`. */
function booleanOr(value: unknown, fallback: boolean): boolean {
  return typeof value === "boolean" ? value : fallback;
}

/** A destination from a stored value; blank, missing or non-string means the default. */
function pathOr(value: unknown): string | null {
  if (typeof value !== "string") {
    return DEFAULT_PATH;
  }
  const trimmed = value.trim();
  return trimmed.length > 0 ? trimmed : DEFAULT_PATH;
}

/** A whole number in range from a stored value; anything else takes `fallback`. */
function wholeNumberOr(value: unknown, fallback: number, min: number, max: number): number {
  return typeof value === "number" && Number.isInteger(value) && value >= min && value <= max
    ? value
    : fallback;
}

/** Resolve the stored chat log block, falling back to the shipped defaults. */
export function chatLogPrefsFrom(values: PrefsDocument): ChatLogPrefs {
  return {
    toFile: booleanOr(
      readPref<unknown>(values, ["chat_log", "to_file"], DEFAULT_TO_FILE),
      DEFAULT_TO_FILE,
    ),
    path: pathOr(readPref<unknown>(values, ["chat_log", "path"], DEFAULT_PATH)),
    maxBytes: wholeNumberOr(
      readPref<unknown>(values, ["chat_log", "max_bytes"], DEFAULT_MAX_BYTES),
      DEFAULT_MAX_BYTES,
      MIN_MAX_BYTES,
      MAX_MAX_BYTES,
    ),
    rotateFiles: wholeNumberOr(
      readPref<unknown>(values, ["chat_log", "rotate_files"], DEFAULT_ROTATE_FILES),
      DEFAULT_ROTATE_FILES,
      MIN_ROTATE_FILES,
      MAX_ROTATE_FILES,
    ),
  };
}

/** Whether a size is one the backend accepts. */
export function isMaxBytesInRange(bytes: number): boolean {
  return Number.isInteger(bytes) && bytes >= MIN_MAX_BYTES && bytes <= MAX_MAX_BYTES;
}

/** Whether a rotation count is one the backend accepts. */
export function isRotateFilesInRange(count: number): boolean {
  return Number.isInteger(count) && count >= MIN_ROTATE_FILES && count <= MAX_ROTATE_FILES;
}

/** A byte count as the largest whole unit that divides it, e.g. `8 MiB`. */
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) {
    return "0 B";
  }
  if (bytes % (1024 * 1024) === 0) {
    return `${bytes / (1024 * 1024)} MiB`;
  }
  if (bytes % 1024 === 0) {
    return `${bytes / 1024} KiB`;
  }
  return `${bytes} B`;
}

/** Whether an option's effect is actually implemented by this build. */
export type OptionSupport = "supported" | "unsupported";

/** One option in the Chat logging group, and whether this build acts on it. */
export interface ChatLogOption {
  /** The stable option name, matching the key after `prefs.chat_log.`. */
  key: "to_file" | "path" | "max_bytes" | "rotate_files";
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
 * All four are backed by the transcript writer, so none carries a "Not
 * supported" marker. Anything this build could not honour would have to be
 * rendered disabled and marked rather than written.
 */
export const CHAT_LOG_OPTIONS: readonly ChatLogOption[] = [
  {
    key: "to_file",
    label: "Save the transcript to a file",
    summary: "Write every chat line to disk as it arrives, while this is on.",
    support: "supported",
    reason: null,
  },
  {
    key: "path",
    label: "Destination",
    summary: "Where the transcript is written; the default is beside the app's logs.",
    support: "supported",
    reason: null,
  },
  {
    key: "max_bytes",
    label: "Size cap",
    summary: "Rotate the file once it reaches this size, so it can never grow without bound.",
    support: "supported",
    reason: null,
  },
  {
    key: "rotate_files",
    label: "Kept rotations",
    summary: "How many older files are kept; zero truncates instead of archiving.",
    support: "supported",
    reason: null,
  },
];

/** The keys of this group's options that have no backing behaviour (none today). */
export const UNSUPPORTED_CHAT_LOG_KEYS: readonly string[] = CHAT_LOG_OPTIONS.filter(
  (option) => option.support === "unsupported",
).map((option) => option.key);

/** The table entry for one option key, or `null` when the key is unknown. */
export function chatLogOption(key: string): ChatLogOption | null {
  return CHAT_LOG_OPTIONS.find((option) => option.key === key) ?? null;
}

/** Whether this build acts on the option named `key` (unknown keys are not supported). */
export function isChatLogOptionSupported(key: string): boolean {
  return chatLogOption(key)?.support === "supported";
}
