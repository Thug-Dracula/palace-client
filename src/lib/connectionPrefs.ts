/**
 * The rules behind the Connection & identity preference group (Task 20).
 *
 * `PREFERENCES.md` is the authority for the keys this module reads:
 *
 * * `prefs.connection.last_servers` — the servers used recently, most recent
 *   first, capped at eight (the classic client's "Palace Starts With" block).
 * * `prefs.connection.home_palace` — the one server the launch offer names.
 * * `prefs.connection.auto_connect` — whether the offer is made at all. It is
 *   only meaningful with a home palace, and it never connects on its own: the
 *   launch affordance is a banner the user has to accept.
 *
 * The module deliberately has no Tauri import, so every rule below is
 * unit-testable in a plain environment. `host`, `port` and `username` are
 * *not* read from here: they are the top-level settings fields the backend
 * owns, mirrored by `api.getSettings` / `api.setConnectionSettings`.
 */

import { readPref, type PrefsDocument } from "./prefs";

/** One server the client has used, as `PREFERENCES.md` stores it. */
export interface ServerRef {
  host: string;
  port: number;
  /** The user name last used there; `Guest` when the entry did not carry one. */
  username: string;
}

/** The cap from `PREFERENCES.md`: `prefs.connection.last_servers` keeps eight. */
export const LAST_SERVERS_LIMIT = 8;

/**
 * Read one stored server entry.
 *
 * Anything that is not a usable `{host, port, username}` (a foreign value, a
 * port outside 1-65535, a missing host) reads as `null`, so a hand-edited or
 * older settings file can never break the group.
 */
export function parseServerRef(value: unknown): ServerRef | null {
  if (typeof value !== "object" || value === null) {
    return null;
  }
  const record = value as Record<string, unknown>;
  const host = typeof record.host === "string" ? record.host.trim() : "";
  const username = typeof record.username === "string" ? record.username.trim() : "";
  const port = record.port;
  if (host.length === 0) {
    return null;
  }
  if (typeof port !== "number" || !Number.isInteger(port) || port < 1 || port > 65535) {
    return null;
  }
  return { host, port, username: username.length > 0 ? username : "Guest" };
}

/** The stored last-servers list, validated and capped at eight, most recent first. */
export function lastServersFrom(values: PrefsDocument): ServerRef[] {
  const raw = readPref<unknown>(values, ["connection", "last_servers"], []);
  if (!Array.isArray(raw)) {
    return [];
  }
  const servers: ServerRef[] = [];
  for (const entry of raw) {
    const server = parseServerRef(entry);
    if (server) {
      servers.push(server);
    }
    if (servers.length === LAST_SERVERS_LIMIT) {
      break;
    }
  }
  return servers;
}

/** The stored home palace, or `null` when none is set or it is unusable. */
export function homePalaceFrom(values: PrefsDocument): ServerRef | null {
  return parseServerRef(readPref<unknown>(values, ["connection", "home_palace"], null));
}

/** Whether the launch offer is switched on. Absent or non-boolean reads as off. */
export function autoConnectFrom(values: PrefsDocument): boolean {
  return readPref<unknown>(values, ["connection", "auto_connect"], false) === true;
}

/**
 * Whether two entries name the same palace.
 *
 * The palace is the address (host and port), not the name used there, so
 * connecting to one server under two names keeps one row; the most recent name
 * wins when the entry is re-remembered.
 */
export function sameServer(left: ServerRef, right: ServerRef): boolean {
  return left.port === right.port && left.host.toLowerCase() === right.host.toLowerCase();
}

/**
 * Record `entry` as the most recently used server.
 *
 * The entry moves to the front (an existing row for the same palace is
 * replaced, not duplicated) and the list is cut back to
 * {@link LAST_SERVERS_LIMIT}, so it can never grow without bound.
 */
export function rememberServer(servers: readonly ServerRef[], entry: ServerRef): ServerRef[] {
  const rest = servers.filter((server) => !sameServer(server, entry));
  return [entry, ...rest].slice(0, LAST_SERVERS_LIMIT);
}

/** Drop `entry` (by palace) from the list, keeping the order of the rest. */
export function forgetServer(servers: readonly ServerRef[], entry: ServerRef): ServerRef[] {
  return servers.filter((server) => !sameServer(server, entry));
}

/**
 * The server the launch offer names, or `null` when there is nothing to offer.
 *
 * The offer needs both halves: a home palace to connect to and the
 * `auto_connect` switch turned on. This function only decides *whether* to
 * offer — the connection itself stays a deliberate click, never a silent one.
 */
export function launchOffer(values: PrefsDocument): ServerRef | null {
  return autoConnectFrom(values) ? homePalaceFrom(values) : null;
}

/** `host:port`, the way a server is named in the UI. */
export function describeServer(server: ServerRef): string {
  return `${server.host}:${server.port}`;
}

/**
 * The identity as the read-only indicator prints it.
 *
 * The shape is the stored `identity` object from `settings.json`
 * (`settings.rs`): a registration pair and the PUID. It is read-only by
 * design — the indicator exists so a user can tell two installs apart, and
 * nothing in the Preferences window may edit or replace it. `null` covers an
 * absent or unrecognised shape, so the UI says "not minted yet" instead of
 * printing nonsense.
 */
export function identityFingerprint(identity: unknown): string | null {
  if (typeof identity !== "object" || identity === null) {
    return null;
  }
  const puid = (identity as Record<string, unknown>).puid;
  if (typeof puid !== "object" || puid === null) {
    return null;
  }
  const { ctr, crc } = puid as Record<string, unknown>;
  if (
    typeof ctr !== "number" ||
    typeof crc !== "number" ||
    !Number.isInteger(ctr) ||
    !Number.isInteger(crc)
  ) {
    return null;
  }
  const hex = (value: number): string => (value >>> 0).toString(16).padStart(8, "0");
  return `${hex(ctr)}-${hex(crc)}`;
}
