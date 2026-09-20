import type { EditorState } from "../../../api";

/**
 * The panel-side contract for the prop editor.
 *
 * Every panel under `components/editor/panels/` is rendered by `EditorDialog`
 * with exactly these props, so a later worker can fill a panel in without
 * touching the shell. Panels are presentation + panel-local state only: they
 * must send work through `notify`/`refresh` and never reach into the api or the
 * document directly. Pixel work stays in Rust; panels describe intent.
 */

/** Severity of a panel's status message, matching the shell's status line. */
export type PanelNoticeKind = "info" | "warn" | "error";

/** What every editor panel receives. */
export interface PanelContract {
  /** The open document's snapshot, or null when nothing is open. */
  state: EditorState | null;
  /** True while any editor command is in flight; panels disable their controls. */
  busy: boolean;
  /** True when this panel is the visible tab. */
  active: boolean;
  /** Post a one-line message to the editor's status line. */
  notify: (message: string, kind?: PanelNoticeKind) => void;
  /** Re-read the session state and current frame after a mutation. */
  refresh: () => Promise<void>;
}
