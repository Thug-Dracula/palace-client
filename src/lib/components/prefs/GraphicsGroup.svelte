<script lang="ts">
  import { BALLOON_DELAYS, graphicsPrefsFrom, graphicsOption } from "../../graphicsPrefs";
  import { prefs } from "../../prefsStore.svelte";
  import { store } from "../../store.svelte";

  // Mirrors of the stored values, so the controls keep what the user chose
  // while a save is in flight, then resync from the file when it echoes back.
  let showNames = $state(true);
  let showAvatars = $state(true);
  let showGuests = $state(true);
  let busy = $state(false);
  let error = $state<string | null>(null);

  const graphics = $derived(graphicsPrefsFrom(prefs.values));

  // Resync the controls whenever the stored graphics block changes (load,
  // save echo, restore defaults).
  $effect(() => {
    showNames = graphics.showNames;
    showAvatars = graphics.showAvatars;
    showGuests = graphics.showGuests;
  });

  // Live application: a stored change drives the same store state the rest of
  // the shell reads, so every open window follows with no reload.
  $effect(() => {
    store.applyGraphics(graphics);
  });

  /** The reason an option is disabled, straight from the support table. */
  function reason(key: string): string {
    return graphicsOption(key)?.reason ?? "Not supported in this build yet.";
  }

  /**
   * Persist a `graphics.*` patch, then apply it live.
   *
   * Nothing here reconnects or reloads. The write goes first: a failed write
   * leaves the controls (and the room view) exactly as they were, so a switch
   * can never show a state the file does not have.
   */
  async function save(patch: Record<string, unknown>, message: string): Promise<boolean> {
    if (busy) {
      return false;
    }
    busy = true;
    error = null;
    try {
      await prefs.setLive({ graphics: patch }, message);
      if (prefs.error) {
        error = prefs.error;
        return false;
      }
      return true;
    } finally {
      busy = false;
    }
  }

  async function onShowNames(event: Event): Promise<void> {
    const box = event.currentTarget as HTMLInputElement;
    const wanted = box.checked;
    if (!(await save({ show_names: wanted }, wanted ? "Names shown." : "Names hidden."))) {
      box.checked = graphics.showNames;
    }
  }

  async function onShowAvatars(event: Event): Promise<void> {
    const box = event.currentTarget as HTMLInputElement;
    const wanted = box.checked;
    if (!(await save({ show_avatars: wanted }, wanted ? "Avatars shown." : "Avatars hidden."))) {
      box.checked = graphics.showAvatars;
    }
  }

  async function onShowGuests(event: Event): Promise<void> {
    const box = event.currentTarget as HTMLInputElement;
    const wanted = box.checked;
    const message = wanted ? "Guests listed." : "Guests hidden from the list.";
    if (!(await save({ show_guests: wanted }, message))) {
      box.checked = graphics.showGuests;
    }
  }
</script>

<div class="prefs-group">
  {#if error}
    <p class="prefs-error" role="alert">{error}</p>
  {/if}

  <section class="prefs-section" aria-labelledby="prefs-graphics-room">
    <h2 id="prefs-graphics-room">Room view</h2>
    <p class="prefs-note">
      These three take effect immediately in every open room window, and are remembered for the
      next launch.
    </p>

    <label class="prefs-check">
      <input
        type="checkbox"
        checked={showNames}
        disabled={busy}
        data-testid="graphics-show-names"
        onchange={onShowNames}
      />
      <span>Show names</span>
    </label>

    <label class="prefs-check">
      <input
        type="checkbox"
        checked={showAvatars}
        disabled={busy}
        data-testid="graphics-show-avatars"
        onchange={onShowAvatars}
      />
      <span>Show avatars</span>
    </label>

    <label class="prefs-check">
      <input
        type="checkbox"
        checked={showGuests}
        disabled={busy}
        data-testid="graphics-show-guests"
        onchange={onShowGuests}
      />
      <span>Show guests in the user list</span>
    </label>
  </section>

  <section class="prefs-section" aria-labelledby="prefs-graphics-animation">
    <h2 id="prefs-graphics-animation">Animation &amp; balloons</h2>
    <p class="prefs-note">
      These options are from the classic client but are <b>not supported</b> in this build yet.
      They are shown disabled, and changing them would not do anything — so they cannot be changed.
    </p>

    <div class="prefs-option" data-testid="graphics-animation-row">
      <label class="prefs-check">
        <input
          type="checkbox"
          checked={graphics.animation}
          disabled
          data-testid="graphics-animation"
        />
        <span>Allow animation</span>
      </label>
      <span class="prefs-unsupported" data-testid="unsupported-animation">Not supported</span>
      <p class="prefs-note">{reason("animation")}</p>
    </div>

    <div class="prefs-option" data-testid="graphics-tinted-balloons-row">
      <label class="prefs-check">
        <input
          type="checkbox"
          checked={graphics.tintedBalloons}
          disabled
          data-testid="graphics-tinted-balloons"
        />
        <span>Tinted balloons</span>
      </label>
      <span class="prefs-unsupported" data-testid="unsupported-tinted_balloons">Not supported</span>
      <p class="prefs-note">{reason("tinted_balloons")}</p>
    </div>

    <div class="prefs-option" data-testid="graphics-balloon-delay-row">
      <label class="prefs-check">
        <span>Balloon delay</span>
        <select value={graphics.balloonDelay} disabled data-testid="graphics-balloon-delay">
          {#each BALLOON_DELAYS as delay (delay)}
            <option value={delay}>{delay}</option>
          {/each}
        </select>
      </label>
      <span class="prefs-unsupported" data-testid="unsupported-balloon_delay">Not supported</span>
      <p class="prefs-note">{reason("balloon_delay")}</p>
    </div>
  </section>
</div>

<style>
  .prefs-option {
    margin: var(--sp-3) 0;
  }

  .prefs-option .prefs-check {
    margin: 0;
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
