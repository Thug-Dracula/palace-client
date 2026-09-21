import * as api from "./api";
import { FIRST_GROUP_ID, lastGroupFrom, type PrefGroupId, type PrefsDocument } from "./prefs";

/**
 * The Preferences window's state and the two apply paths.
 *
 * Live preferences go through {@link setLive}: the value is persisted and takes
 * effect at once. Connection preferences go through {@link saveConnection},
 * which persists the value but deliberately never reconnects; it raises
 * {@link reconnectRequired} so the UI can show the affordance instead. That
 * split is the apply-semantics contract from `PREFERENCES.md`, kept in one seam
 * so every group component uses the same one.
 */
class PrefsStore {
  /** The loaded `prefs` block, exactly as the backend returned it. */
  values = $state<PrefsDocument>({});
  /** The group the content area is showing. */
  activeGroup = $state<PrefGroupId>(FIRST_GROUP_ID);
  loaded = $state(false);
  busy = $state(false);
  error = $state<string | null>(null);
  notice = $state<string | null>(null);
  /** A connection change is saved but not yet in effect. */
  reconnectRequired = $state(false);

  /** Return to a clean slate (used by tests before mounting a window). */
  reset(): void {
    this.values = {};
    this.activeGroup = FIRST_GROUP_ID;
    this.loaded = false;
    this.busy = false;
    this.error = null;
    this.notice = null;
    this.reconnectRequired = false;
  }

  /** Read the current values and reopen on the remembered group. */
  async load(): Promise<void> {
    this.busy = true;
    this.error = null;
    try {
      this.values = await api.getPrefs();
      this.activeGroup = lastGroupFrom(this.values);
      this.loaded = true;
    } catch (cause) {
      this.error = `Could not read preferences: ${String(cause)}`;
    } finally {
      this.busy = false;
    }
  }

  /** Switch the content area and remember the group for next time. */
  async selectGroup(id: PrefGroupId): Promise<void> {
    this.activeGroup = id;
    await this.setLive({ shell: { last_group: id } }, "Group remembered.");
  }

  /** Persist a live-apply patch; it is in effect as soon as this resolves. */
  async setLive(patch: PrefsDocument, message: string | null = "Saved."): Promise<void> {
    this.error = null;
    try {
      this.values = await api.setPrefs(patch);
      if (message) {
        this.notice = message;
      }
    } catch (cause) {
      this.error = `Could not save preferences: ${String(cause)}`;
    }
  }

  /**
   * Save host/port/user for the next connection without reconnecting.
   *
   * Returns whether the save succeeded, so a group can keep the form in an
   * error state when it did not.
   */
  async saveConnection(host: string, port: number, username: string): Promise<boolean> {
    this.error = null;
    try {
      await api.setConnectionSettings(host, port, username);
      this.reconnectRequired = true;
      this.notice = "Connection settings saved for the next connection.";
      return true;
    } catch (cause) {
      this.error = String(cause);
      return false;
    }
  }

  /** Write this build's defaults back through the additive merge. */
  async restoreDefaults(): Promise<void> {
    this.busy = true;
    this.error = null;
    try {
      this.values = await api.resetPrefs();
      this.activeGroup = lastGroupFrom(this.values);
      this.reconnectRequired = false;
      this.notice = "Defaults restored.";
    } catch (cause) {
      this.error = `Could not restore defaults: ${String(cause)}`;
    } finally {
      this.busy = false;
    }
  }
}

/** The Preferences window's shared state for this webview. */
export const prefs = new PrefsStore();
