/**
 * Pure chat helpers shared by the store and the Chat/log panel.
 *
 * The chat transcript is owned by the backend (WINDOWS-ARCHITECTURE.md §5.1).
 * Every window keeps its own mirror in `store.chat`, and a late-opening window
 * fills that mirror from the replay `refresh()` triggers. The replay is
 * **broadcast** to every open window, so a window that already had the lines
 * sees them again — these helpers keep that replay idempotent and keep a
 * detached window's send path inside the protocol's message limit.
 */

import type { ChatLine } from "./api";

/** Longest chat message the protocol accepts; also the input's `maxlength`. */
export const CHAT_INPUT_LIMIT = 254;

/** Clamp a draft to the protocol limit (defensive against paste, IME, scripts). */
export function clampChatDraft(text: string): string {
  return text.length > CHAT_INPUT_LIMIT ? text.slice(0, CHAT_INPUT_LIMIT) : text;
}

/**
 * Whether `line` is already mirrored in `chat`.
 *
 * Identity is the backend's `seq`. Every line the backend emits carries a
 * unique, app-lifetime monotonic sequence, so re-seeing one means the same line
 * arrived twice — usually because a newly-opened panel asked for a replay that
 * was broadcast to all windows. A line with a negative seq is a local echo that
 * has not been attributed a backend seq yet, so it is never treated as a
 * duplicate.
 */
export function chatLineAlreadySeen(chat: readonly ChatLine[], line: ChatLine): boolean {
  if (line.seq < 0) {
    return false;
  }
  return chat.some((existing) => existing.seq === line.seq);
}
