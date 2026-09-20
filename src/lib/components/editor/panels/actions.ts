import type { EditorOutcome } from "../../../api";
import type { PanelNoticeKind } from "./contract";

/** Turn an unknown thrown value into a one-line message. */
export function causeMessage(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}

/** Human-readable hex for an RGBA byte array. */
export function rgbaHex(color: number[] | null): string {
  if (!color || color.length < 3) {
    return "—";
  }
  const [r, g, b] = color;
  return `#${[r, g, b].map((channel) => channel.toString(16).padStart(2, "0")).join("")}`;
}

/**
 * Run an editor mutation: report the result to the status line, then refresh.
 *
 * The panels never touch the document directly; this is the single path from a
 * control to a command. A refusal is reported with its message instead of
 * throwing, so the panel stays usable.
 */
export async function runOutcome(
  action: () => Promise<EditorOutcome>,
  notify: (message: string, kind?: PanelNoticeKind) => void,
  refresh: () => Promise<void>,
  success: string,
): Promise<void> {
  try {
    const result = await action();
    notify(result.changed ? success : "Nothing changed.");
    await refresh();
  } catch (cause) {
    notify(causeMessage(cause), "error");
  }
}

/** Save bytes to the user's downloads as a file the user explicitly asked for. */
export function downloadBytes(bytes: number[], filename: string, mime: string): void {
  const blob = new Blob([new Uint8Array(bytes)], { type: mime });
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = filename;
  document.body.appendChild(link);
  link.click();
  link.remove();
  URL.revokeObjectURL(url);
}
