<script lang="ts">
  import { onMount } from "svelte";
  import { open } from "@tauri-apps/plugin-dialog";
  import { store } from "../store.svelte";

  let { onclose }: { onclose: () => void } = $props();

  let busy = $state(false);
  let error = $state<string | null>(null);
  let saved = $state<string | null>(null);
  let volume = $state(1);
  let dialog: HTMLDivElement | undefined = $state();

  onMount(async () => {
    try {
      await store.loadAudio();
      volume = store.audio.volume;
    } catch (cause) {
      error = `Could not read the audio state: ${String(cause)}`;
    }
    dialog?.querySelector<HTMLButtonElement>(".btn")?.focus();
  });

  function onKeydown(event: KeyboardEvent) {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      onclose();
      return;
    }
    if (event.key !== "Tab" || !dialog) {
      return;
    }
    const focusables = [...dialog.querySelectorAll<HTMLElement>("button:not([disabled]), input:not([disabled])")];
    const first = focusables.at(0);
    const last = focusables.at(-1);
    if (!first || !last) {
      return;
    }
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  }

  async function pick() {
    if (busy) {
      return;
    }
    error = null;
    saved = null;
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
      saved = "SoundFont applied.";
    } catch (cause) {
      error = String(cause);
    } finally {
      busy = false;
    }
  }

  async function clear() {
    if (busy) {
      return;
    }
    busy = true;
    error = null;
    saved = null;
    try {
      await store.chooseSoundfont(null);
      saved = "Cleared — MIDI plays the fallback tone.";
    } catch (cause) {
      error = String(cause);
    } finally {
      busy = false;
    }
  }

  async function toggleMute() {
    if (busy) {
      return;
    }
    busy = true;
    error = null;
    saved = null;
    const muting = store.audio.enabled;
    try {
      await store.setAudioEnabled(!muting);
      saved = muting ? "Audio muted." : "Audio unmuted.";
    } catch (cause) {
      error = String(cause);
    } finally {
      busy = false;
    }
  }

  async function commitVolume() {
    if (busy) {
      return;
    }
    busy = true;
    error = null;
    saved = null;
    try {
      const applied = await store.setAudioVolume(volume);
      volume = applied;
      saved = `Volume ${Math.round(applied * 100)}%.`;
    } catch (cause) {
      error = String(cause);
      volume = store.audio.volume;
    } finally {
      busy = false;
    }
  }
</script>

<div class="dialog-backdrop">
  <div
    class="dialog audio-dialog"
    role="dialog"
    aria-modal="true"
    aria-label="Audio and SoundFont"
    tabindex="-1"
    bind:this={dialog}
    onkeydown={onKeydown}
    oncontextmenu={(event) => event.preventDefault()}
  >
    <div class="dialog-head">
      <span>Audio &amp; SoundFont</span>
      <button class="dialog-x" type="button" aria-label="Close" onclick={onclose}>×</button>
    </div>

    <div class="dialog-body">
      <p class="dialog-hint">
        MIDI is synthesized with a SoundFont file. Pick an <code>.sf2</code> file from your disk;
        with none, MIDI plays a synthesized fallback tone.
      </p>

      <div class="block-label">Current SoundFont</div>
      <div class="soundfont">
        {#if store.audio.soundfont}
          <span class="soundfont-path" title={store.audio.soundfont}>{store.audio.soundfont}</span>
          {#if store.audio.soundfont_exists}
            <span class="audio-note ok">Selected.</span>
          {:else}
            <span class="audio-note warn">This file is missing on disk — MIDI plays the fallback tone.</span>
          {/if}
        {:else}
          <span class="audio-note">None — MIDI plays a fallback tone.</span>
        {/if}
      </div>

      <div class="audio-actions">
        <button class="btn primary" type="button" onclick={pick} disabled={busy}>
          Choose .sf2…
        </button>
        <button class="btn" type="button" onclick={clear} disabled={busy || !store.audio.soundfont}>
          Clear
        </button>
      </div>

      <div class="audio-block">
        <span class="block-label">Output</span>
        <div class="audio-row">
          <button
            class="btn"
            type="button"
            aria-pressed={!store.audio.enabled}
            onclick={toggleMute}
            disabled={busy}
          >
            {store.audio.enabled ? "Mute" : "Unmute"}
          </button>
          <input
            type="range"
            min="0"
            max="1"
            step="0.01"
            aria-label="Master volume"
            bind:value={volume}
            onchange={commitVolume}
            disabled={busy}
          />
          <span class="readout">{Math.round(volume * 100)}%</span>
        </div>
      </div>

      {#if error}<p class="dialog-error">{error}</p>{/if}
      {#if saved && !error}<p class="audio-note ok audio-saved">{saved}</p>{/if}

      <div class="dialog-actions">
        <button class="btn" type="button" onclick={onclose} disabled={busy}>Close</button>
      </div>
    </div>
  </div>
</div>
