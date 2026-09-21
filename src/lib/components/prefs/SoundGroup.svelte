<script lang="ts">
  import { onMount } from "svelte";
  import { open } from "@tauri-apps/plugin-dialog";

  import { SOUND_OPTIONS, soundPrefsFrom, soundfontSummary } from "../../soundPrefs";
  import { prefs } from "../../prefsStore.svelte";
  import { store } from "../../store.svelte";

  // Mirrors of the engine's master level, so the slider keeps what the user
  // chose while a save is in flight, then resyncs from the engine's echo.
  let volume = $state(1);
  let busy = $state(false);
  let error = $state<string | null>(null);
  let notice = $state<string | null>(null);

  const sound = $derived(soundPrefsFrom(prefs.values));
  const font = $derived(soundfontSummary(store.audio));

  onMount(async () => {
    try {
      await store.loadAudio();
      volume = store.audio.volume;
    } catch (cause) {
      error = `Could not read the audio state: ${String(cause)}`;
    }
  });

  /**
   * Mute or unmute the engine.
   *
   * The write goes through `set_audio_enabled`, which persists the choice and
   * hands it to the running engine in one step: no reload and no reconnect. A
   * failed write leaves the checkbox showing the engine's real state.
   */
  async function onMute(event: Event): Promise<void> {
    const box = event.currentTarget as HTMLInputElement;
    if (busy) {
      box.checked = !store.audio.enabled;
      return;
    }
    const muted = box.checked;
    busy = true;
    error = null;
    notice = null;
    try {
      await store.setAudioEnabled(!muted);
      notice = muted ? "Sound muted — in effect now." : "Sound unmuted — in effect now.";
    } catch (cause) {
      error = String(cause);
      box.checked = !store.audio.enabled;
    } finally {
      busy = false;
    }
  }

  /**
   * Apply the master level.
   *
   * Committed on release (`onchange`) rather than on every pixel of a drag, so
   * a slide is one write, not hundreds. The engine clamps and echoes the value
   * back, and the readout shows what was actually applied.
   */
  async function commitVolume(): Promise<void> {
    if (busy) {
      return;
    }
    busy = true;
    error = null;
    notice = null;
    try {
      const applied = await store.setAudioVolume(volume);
      volume = applied;
      notice = `Master volume ${Math.round(applied * 100)}% — in effect now.`;
    } catch (cause) {
      error = String(cause);
      volume = store.audio.volume;
    } finally {
      busy = false;
    }
  }

  async function pick(): Promise<void> {
    if (busy) {
      return;
    }
    error = null;
    notice = null;
    let selection: string | null;
    try {
      selection = await open({
        title: "Choose a SoundFont (.sf2)",
        multiple: false,
        directory: false,
        filters: [{ name: "SoundFont", extensions: ["sf2"] }],
      });
    } catch (cause) {
      error = `Could not open the file dialog: ${String(cause)}`;
      return;
    }
    if (selection === null) {
      return;
    }
    busy = true;
    try {
      await store.chooseSoundfont(selection);
      notice = "SoundFont applied to the running session.";
    } catch (cause) {
      error = String(cause);
    } finally {
      busy = false;
    }
  }

  /** Drop the chosen file and let the engine use the bundled bank again. */
  async function clear(): Promise<void> {
    if (busy) {
      return;
    }
    busy = true;
    error = null;
    notice = null;
    try {
      await store.chooseSoundfont(null);
      notice = "Back to the bundled bank — in effect now.";
    } catch (cause) {
      error = String(cause);
    } finally {
      busy = false;
    }
  }
</script>

<div class="prefs-group">
  {#if error}
    <p class="prefs-error" role="alert">{error}</p>
  {/if}

  <section class="prefs-section" aria-labelledby="prefs-sound-output">
    <h2 id="prefs-sound-output">Output</h2>
    <p class="prefs-note">
      Mute and the master level are the audio engine's own settings. A change takes effect in the
      running session at once and is remembered for the next launch — nothing here reconnects or
      reloads.
    </p>

    <label class="prefs-check">
      <input
        type="checkbox"
        checked={!store.audio.enabled}
        disabled={busy}
        data-testid="sound-mute"
        onchange={onMute}
      />
      <span>Mute sound</span>
    </label>

    <div class="sound-row">
      <label class="sound-label" for="prefs-sound-volume">Master volume</label>
      <input
        id="prefs-sound-volume"
        type="range"
        min="0"
        max="1"
        step="0.01"
        aria-label="Master volume"
        bind:value={volume}
        onchange={commitVolume}
        disabled={busy}
        data-testid="sound-volume"
      />
      <span class="readout" data-testid="sound-volume-readout">{Math.round(volume * 100)}%</span>
    </div>
  </section>

  <section class="prefs-section" aria-labelledby="prefs-sound-font">
    <h2 id="prefs-sound-font">SoundFont</h2>
    <p class="prefs-note">
      MIDI is synthesized with a SoundFont (<code>.sf2</code>). The bundled bank is the default;
      choosing a file replaces it, and clearing the choice returns to the bundled bank. A file that
      has gone missing falls back to the bundled bank rather than leaving MIDI silent.
    </p>

    <p class="sound-note {font.tone}" data-testid="sound-soundfont">{font.text}</p>

    <div class="sound-actions">
      <button class="btn primary" type="button" onclick={pick} disabled={busy} data-testid="sound-choose">
        Choose .sf2…
      </button>
      <button
        class="btn"
        type="button"
        onclick={clear}
        disabled={busy || !store.audio.soundfont}
        data-testid="sound-clear"
      >
        Use bundled bank
      </button>
    </div>
  </section>

  <section class="prefs-section" aria-labelledby="prefs-sound-split">
    <h2 id="prefs-sound-split">Effects &amp; music levels</h2>
    <p class="prefs-note">
      The classic client could set sound effects and music to different levels. This build cannot:
      the audio engine mixes every sound into one master output, so a separate split is
      <b>not supported</b>. These controls are shown disabled, and changing them would not do
      anything — so they cannot be changed.
    </p>

    {#each SOUND_OPTIONS as option (option.key)}
      <div class="prefs-option" data-testid="sound-{option.key}-row">
        <label class="prefs-check">
          <span>{option.label}</span>
          {#if option.key === "speech_voice"}
            <select value={sound.speechVoice ?? ""} disabled data-testid="sound-speech-voice">
              <option value="">System default</option>
            </select>
          {:else}
            <input
              type="range"
              min="0"
              max="1"
              step="0.01"
              value={option.key === "sfx_volume" ? sound.sfxVolume : sound.musicVolume}
              disabled
              data-testid="sound-{option.key === "sfx_volume" ? "sfx-volume" : "music-volume"}"
            />
          {/if}
        </label>
        <span class="prefs-unsupported" data-testid="unsupported-{option.key}">Not supported</span>
        <p class="prefs-note">{option.reason ?? ""}</p>
      </div>
    {/each}
  </section>

  {#if notice}
    <p class="prefs-notice" role="status" data-testid="sound-notice">{notice}</p>
  {/if}
</div>

<style>
  .sound-row {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    margin: var(--sp-3) 0;
  }

  .sound-label {
    min-width: 9rem;
  }

  .sound-row input[type="range"] {
    flex: 1;
  }

  .sound-actions {
    display: flex;
    gap: var(--sp-2);
    margin-top: var(--sp-3);
  }

  .sound-note {
    margin: var(--sp-2) 0;
    word-break: break-all;
  }

  .sound-note.ok {
    color: var(--fg-dim);
  }

  .sound-note.warn {
    color: var(--amber);
  }

  .sound-note.none {
    color: var(--fg-dim);
  }

  .prefs-option {
    margin: var(--sp-3) 0;
  }

  .prefs-option .prefs-check {
    margin: 0;
    display: flex;
    align-items: center;
    gap: var(--sp-2);
  }

  .prefs-option .prefs-check input[type="range"] {
    flex: 1;
    max-width: 16rem;
  }

  .prefs-option .prefs-note {
    margin: var(--sp-1) 0 0;
    padding-left: var(--sp-4);
  }

  .prefs-unsupported {
    display: inline-block;
    margin-left: var(--sp-2);
    padding: 0 var(--sp-1);
    border: 1px solid var(--amber-dim);
    border-radius: var(--r-xs);
    color: var(--amber-dim);
    font-size: var(--fs-xs);
    letter-spacing: 0.08em;
    text-transform: uppercase;
    vertical-align: middle;
  }
</style>
