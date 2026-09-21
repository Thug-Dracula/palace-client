<script lang="ts">
  import { onMount } from "svelte";
  import { save } from "@tauri-apps/plugin-dialog";

  import * as api from "../../api";
  import {
    DEFAULT_MAX_BYTES,
    DEFAULT_ROTATE_FILES,
    IN_MEMORY_LOG_LINES,
    MAX_MAX_BYTES,
    MAX_ROTATE_FILES,
    MIN_MAX_BYTES,
    MIN_ROTATE_FILES,
    chatLogPrefsFrom,
    formatBytes,
    isMaxBytesInRange,
    isRotateFilesInRange,
  } from "../../chatLogPrefs";
  import { prefs } from "../../prefsStore.svelte";

  // Mirrors of the stored values, so the controls keep what the user chose
  // while a save is in flight, then resync from the file when it echoes back.
  let toFile = $state(false);
  let pathDraft = $state("");
  let maxBytes = $state(DEFAULT_MAX_BYTES);
  let rotateFiles = $state(DEFAULT_ROTATE_FILES);
  let status = $state<api.ChatLogStatus | null>(null);
  let busy = $state(false);
  let error = $state<string | null>(null);

  const stored = $derived(chatLogPrefsFrom(prefs.values));
  const destination = $derived(status?.path ?? stored.path ?? "the default location");

  $effect(() => {
    toFile = stored.toFile;
    pathDraft = stored.path ?? "";
    maxBytes = stored.maxBytes;
    rotateFiles = stored.rotateFiles;
  });

  onMount(() => {
    void refreshStatus();
  });

  /** Re-read the writer's state, so the resolved path shown is the real one. */
  async function refreshStatus(): Promise<void> {
    try {
      status = await api.chatLogStatus();
    } catch {
      status = null;
    }
  }

  /**
   * Persist a `chat_log.*` patch, then resync.
   *
   * The write goes first: a failed write leaves the controls showing what the
   * file actually holds, so a switch can never display a state that is not
   * stored. The backend reconfigures the transcript writer as part of the same
   * command, so a change takes effect in the running session at once.
   */
  async function savePatch(patch: Record<string, unknown>, message: string): Promise<boolean> {
    if (busy) {
      return false;
    }
    busy = true;
    error = null;
    try {
      await prefs.setLive({ chat_log: patch }, message);
      if (prefs.error) {
        error = prefs.error;
        return false;
      }
      await refreshStatus();
      return true;
    } finally {
      busy = false;
    }
  }

  async function onToggle(event: Event): Promise<void> {
    const box = event.currentTarget as HTMLInputElement;
    const wanted = box.checked;
    const message = wanted
      ? "New chat lines will be written to the transcript file."
      : "Transcript file logging stopped.";
    if (!(await savePatch({ to_file: wanted }, message))) {
      box.checked = stored.toFile;
    }
  }

  async function commitPath(): Promise<void> {
    const wanted = pathDraft.trim();
    const current = stored.path ?? "";
    if (wanted === current) {
      return;
    }
    const saved = await savePatch(
      { path: wanted.length > 0 ? wanted : null },
      wanted.length > 0
        ? `Transcript destination set to ${wanted}.`
        : "Transcript destination reset to the default location.",
    );
    if (!saved) {
      pathDraft = current;
    }
  }

  async function pick(): Promise<void> {
    if (busy) {
      return;
    }
    error = null;
    let selection: string | null;
    try {
      selection = await save({
        title: "Choose where to save the chat transcript",
        defaultPath: destination,
      });
    } catch (cause) {
      error = `Could not open the file dialog: ${String(cause)}`;
      return;
    }
    if (selection === null) {
      return;
    }
    pathDraft = selection;
    if (!(await savePatch({ path: selection }, `Transcript destination set to ${selection}.`))) {
      pathDraft = stored.path ?? "";
    }
  }

  async function useDefaultPath(): Promise<void> {
    pathDraft = "";
    if (!(await savePatch({ path: null }, "Transcript destination reset to the default location."))) {
      pathDraft = stored.path ?? "";
    }
  }

  async function commitMaxBytes(event: Event): Promise<void> {
    const input = event.currentTarget as HTMLInputElement;
    const wanted = Number(input.value);
    if (!isMaxBytesInRange(wanted)) {
      error = `The size cap must be a whole number of bytes between ${MIN_MAX_BYTES} and ${MAX_MAX_BYTES}.`;
      input.value = String(stored.maxBytes);
      return;
    }
    if (wanted === stored.maxBytes) {
      return;
    }
    if (!(await savePatch({ max_bytes: wanted }, `Size cap set to ${formatBytes(wanted)}.`))) {
      input.value = String(stored.maxBytes);
    }
  }

  async function commitRotateFiles(event: Event): Promise<void> {
    const input = event.currentTarget as HTMLInputElement;
    const wanted = Number(input.value);
    if (!isRotateFilesInRange(wanted)) {
      error = `Kept rotations must be a whole number between ${MIN_ROTATE_FILES} and ${MAX_ROTATE_FILES}.`;
      input.value = String(stored.rotateFiles);
      return;
    }
    if (wanted === stored.rotateFiles) {
      return;
    }
    if (
      !(await savePatch(
        { rotate_files: wanted },
        wanted === 0
          ? "No rotations kept — the file is truncated at the cap."
          : `Keeping ${wanted} rotation${wanted === 1 ? "" : "s"}.`,
      ))
    ) {
      input.value = String(stored.rotateFiles);
    }
  }
</script>

<div class="prefs-group">
  {#if error}
    <p class="prefs-error" role="alert">{error}</p>
  {/if}

  <section class="prefs-section" aria-labelledby="prefs-chat-log-memory">
    <h2 id="prefs-chat-log-memory">In-memory log</h2>
    <p class="prefs-note" data-testid="chat-log-memory-note">
      Always on, and it needs no setting. The backend keeps the last {IN_MEMORY_LOG_LINES} chat
      lines, and a chat window that opens late is given that history — whether or not a transcript
      file is being written. Closing the client clears it.
    </p>
  </section>

  <section class="prefs-section" aria-labelledby="prefs-chat-log-file">
    <h2 id="prefs-chat-log-file">Save the transcript to a file</h2>
    <p class="prefs-note">
      When this is on, every new chat line is written to disk <b>as it arrives</b> and flushed
      immediately, so the file is readable while the client is still running. Only the lines that
      appear in the chat window are written; your password and identity are never part of it.
    </p>

    <label class="prefs-check">
      <input
        type="checkbox"
        checked={toFile}
        disabled={busy}
        data-testid="chat-log-to-file"
        onchange={onToggle}
      />
      <span>Write the chat transcript to a file</span>
    </label>

    <p class="prefs-note" data-testid="chat-log-status">
      {#if status?.enabled}
        Writing to <code>{status.path}</code> — {formatBytes(status.bytes_written)} so far. Once the
        active file reaches {formatBytes(status.max_bytes)}, it is rotated and the oldest of the
        {" "}{status.rotate_files} kept file{status.rotate_files === 1 ? "" : "s"} is dropped.
      {:else if stored.toFile}
        File logging is on, but nothing is being written to <code>{destination}</code> — the file
        could not be opened. The diagnostic log says why; fixing the destination and saving again
        retries it.
      {:else}
        File logging is off. When it is on, lines are written to <code>{destination}</code>. Nothing
        is written while it is off.
      {/if}
    </p>

    <div class="chat-log-row">
      <label class="chat-log-label" for="prefs-chat-log-path">Destination</label>
      <input
        id="prefs-chat-log-path"
        class="chat-log-path"
        type="text"
        bind:value={pathDraft}
        onchange={commitPath}
        disabled={busy}
        spellcheck="false"
        placeholder="Default location"
        data-testid="chat-log-path"
      />
      <button class="btn" type="button" onclick={pick} disabled={busy} data-testid="chat-log-pick">
        Choose…
      </button>
      <button
        class="btn"
        type="button"
        onclick={useDefaultPath}
        disabled={busy || stored.path === null}
        data-testid="chat-log-default-path"
      >
        Default location
      </button>
    </div>
    <p class="prefs-note">
      Leaving this empty uses <code>{destination}</code>, beside the client's diagnostic log in its
      own file — the transcript and the diagnostic log never share a file.
    </p>

    <div class="chat-log-row">
      <label class="chat-log-label" for="prefs-chat-log-max-bytes">Size cap</label>
      <input
        id="prefs-chat-log-max-bytes"
        type="number"
        min={MIN_MAX_BYTES}
        max={MAX_MAX_BYTES}
        step="1024"
        value={maxBytes}
        onchange={commitMaxBytes}
        disabled={busy}
        data-testid="chat-log-max-bytes"
      />
      <span class="chat-log-readout" data-testid="chat-log-max-bytes-readout">
        bytes — {formatBytes(maxBytes)}
      </span>
    </div>

    <div class="chat-log-row">
      <label class="chat-log-label" for="prefs-chat-log-rotate-files">Kept rotations</label>
      <input
        id="prefs-chat-log-rotate-files"
        type="number"
        min={MIN_ROTATE_FILES}
        max={MAX_ROTATE_FILES}
        step="1"
        value={rotateFiles}
        onchange={commitRotateFiles}
        disabled={busy}
        data-testid="chat-log-rotate-files"
      />
      <span class="chat-log-readout">
        older file{rotateFiles === 1 ? "" : "s"} kept; 0 truncates instead
      </span>
    </div>
  </section>
</div>

<style>
  .chat-log-row {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    margin: var(--sp-3) 0;
  }

  .chat-log-label {
    min-width: 9rem;
  }

  .chat-log-path {
    flex: 1;
    min-width: 0;
  }

  .chat-log-readout {
    color: var(--fg-dim);
    font-size: var(--fs-sm);
  }
</style>
