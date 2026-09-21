<script lang="ts">
  import * as api from "../../api";
  import {
    CRT_DARK_TOKENS,
    DEFAULT_FONT_FAMILY,
    DEFAULT_FONT_SIZE_PX,
    DEFAULT_THEME_ID,
    DEFAULT_UI_SCALE,
    EDITABLE_COLOR_TOKENS,
    MAX_FONT_SIZE_PX,
    MAX_UI_SCALE,
    MIN_FONT_SIZE_PX,
    MIN_UI_SCALE,
    applyAppearance,
    appearanceFrom,
    type EditableColorToken,
  } from "../../appearance";
  import { setFullscreen } from "../../appearanceEffects";
  import { prefs } from "../../prefsStore.svelte";

  // Mirrors of the stored values, so the inputs keep what the user typed while
  // a save is in flight, then resync from the file when it echoes back.
  let fontFamily = $state(DEFAULT_FONT_FAMILY);
  let fontSizePx = $state(DEFAULT_FONT_SIZE_PX);
  let uiScale = $state(DEFAULT_UI_SCALE);
  let fullscreen = $state(false);
  let busy = $state(false);
  let error = $state<string | null>(null);

  const appearance = $derived(appearanceFrom(prefs.values));

  // Resync the inputs whenever the stored appearance changes (load, save echo).
  $effect(() => {
    fontFamily = appearance.fontFamily;
    fontSizePx = appearance.fontSizePx;
    uiScale = appearance.uiScale;
    fullscreen = appearance.fullscreen;
  });

  // Live application: a stored change repaints at once — no reload, no
  // reconnect. Values left at their default remove the inline property, so the
  // stylesheet still owns the default look.
  $effect(() => {
    applyAppearance(appearance);
  });

  /** The colour a swatch should show: the override, else the palette value. */
  function colourOf(token: EditableColorToken): string {
    return appearance.tokens[token] ?? CRT_DARK_TOKENS[token] ?? "#000000";
  }

  /** Whether a token currently carries an override (so "Reset" is meaningful). */
  function isOverridden(token: EditableColorToken): boolean {
    return appearance.tokens[token] !== undefined;
  }

  /**
   * Persist an `appearance.*` patch. Nothing here reconnects or reloads, and a
   * failed write leaves the file and the rendered style alone.
   */
  async function save(patch: Record<string, unknown>, message: string): Promise<void> {
    if (busy) {
      return;
    }
    busy = true;
    error = null;
    try {
      await prefs.setLive({ appearance: patch }, message);
      if (prefs.error) {
        error = prefs.error;
      }
    } finally {
      busy = false;
    }
  }

  async function onColour(token: EditableColorToken, event: Event): Promise<void> {
    const input = event.currentTarget as HTMLInputElement;
    const value = input.value;
    await save({ tokens: { [token]: value } }, `${token} colour saved.`);
    if (prefs.error) {
      // The write failed, so put the swatch back where it was.
      input.value = colourOf(token);
    }
  }

  async function resetColour(token: EditableColorToken): Promise<void> {
    // Writing the palette value back drops the override: the applier removes
    // the inline property and the stylesheet's own colour returns.
    await save(
      { tokens: { [token]: CRT_DARK_TOKENS[token] } },
      `${token} colour reset to the theme default.`,
    );
  }

  async function resetAllColours(): Promise<void> {
    const tokens: Record<string, string> = {};
    for (const token of EDITABLE_COLOR_TOKENS) {
      tokens[token] = CRT_DARK_TOKENS[token];
    }
    await save({ tokens }, "Colours reset to the theme default.");
  }

  async function onTheme(event: Event): Promise<void> {
    const value = (event.currentTarget as HTMLSelectElement).value;
    await save({ theme: value }, "Theme saved.");
  }

  async function onFontFamily(event: Event): Promise<void> {
    const input = event.currentTarget as HTMLInputElement;
    const value = input.value.trim() || DEFAULT_FONT_FAMILY;
    await save({ font_family: value }, "Font family saved.");
    if (prefs.error) {
      input.value = appearance.fontFamily;
    }
  }

  function clamp(value: number, min: number, max: number): number {
    return Math.min(max, Math.max(min, value));
  }

  async function onFontSize(event: Event): Promise<void> {
    const input = event.currentTarget as HTMLInputElement;
    const parsed = Number(input.value);
    const size = Number.isFinite(parsed)
      ? Math.round(clamp(parsed, MIN_FONT_SIZE_PX, MAX_FONT_SIZE_PX))
      : DEFAULT_FONT_SIZE_PX;
    await save({ font_size_px: size }, `Text size ${size}px.`);
    if (prefs.error) {
      input.value = String(appearance.fontSizePx);
    } else {
      input.value = String(size);
    }
  }

  async function onScale(event: Event): Promise<void> {
    const input = event.currentTarget as HTMLInputElement;
    const value = clamp(Number(input.value), MIN_UI_SCALE, MAX_UI_SCALE);
    await save({ ui_scale: value }, `Interface scale ${Math.round(value * 100)}%.`);
    if (!prefs.error) {
      // The scale resizes the main window through the existing command; a
      // failure only means this host has no main window (unit tests), and the
      // saved value is still applied on the next launch.
      try {
        await api.setUiScale(value);
      } catch {
        // The preference is saved; the live resize is best-effort.
      }
    }
  }

  async function onFullscreen(event: Event): Promise<void> {
    const box = event.currentTarget as HTMLInputElement;
    const wanted = box.checked;
    await save({ fullscreen: wanted }, wanted ? "Full screen on." : "Full screen off.");
    if (prefs.error) {
      box.checked = appearance.fullscreen;
      return;
    }
    try {
      await setFullscreen(wanted);
    } catch (cause) {
      error = `Full screen could not be changed: ${String(cause)}`;
    }
  }
</script>

<div class="prefs-group">
  {#if error}
    <p class="prefs-error" role="alert">{error}</p>
  {/if}

  <section class="prefs-section" aria-labelledby="prefs-theme">
    <h2 id="prefs-theme">Theme</h2>
    <p class="prefs-note">
      This build ships one palette, the dark CRT theme it has always used. Nothing here changes the
      look until you edit a colour below.
    </p>
    <div class="prefs-field-row">
      <label class="prefs-field grow">
        <span>Theme</span>
        <select
          value={appearance.theme}
          disabled={busy}
          onchange={onTheme}
          data-testid="appearance-theme"
        >
          <option value={DEFAULT_THEME_ID}>Dark CRT (built in)</option>
        </select>
      </label>
    </div>

    <div class="prefs-colours">
      {#each EDITABLE_COLOR_TOKENS as token (token)}
        <div class="prefs-colour" data-testid="colour-row-{token}">
          <input
            class="prefs-swatch"
            type="color"
            value={colourOf(token)}
            disabled={busy}
            aria-label="{token} colour"
            title="Change the {token} accent"
            data-testid="colour-{token}"
            onchange={(event) => onColour(token, event)}
          />
          <span class="prefs-colour-name">{token}</span>
          {#if isOverridden(token)}
            <button
              class="btn ghost prefs-colour-reset"
              type="button"
              disabled={busy}
              onclick={() => resetColour(token)}
            >
              Reset
            </button>
          {:else}
            <span class="prefs-colour-default">theme default</span>
          {/if}
        </div>
      {/each}
    </div>
    <div class="prefs-actions">
      <button class="btn" type="button" disabled={busy} onclick={resetAllColours}>
        Reset colours
      </button>
    </div>
  </section>

  <section class="prefs-section" aria-labelledby="prefs-type">
    <h2 id="prefs-type">Font</h2>
    <div class="prefs-field-row">
      <label class="prefs-field grow">
        <span>Font family</span>
        <input
          value={fontFamily}
          spellcheck="false"
          autocomplete="off"
          disabled={busy}
          data-testid="font-family"
          onchange={onFontFamily}
        />
      </label>
      <label class="prefs-field">
        <span>Text size (px)</span>
        <input
          type="number"
          min={MIN_FONT_SIZE_PX}
          max={MAX_FONT_SIZE_PX}
          step="1"
          value={fontSizePx}
          disabled={busy}
          data-testid="font-size"
          onchange={onFontSize}
        />
      </label>
    </div>
    <p class="prefs-note">
      The family is any CSS font stack, so keep a monospace fallback at the end. An empty field
      restores the shipped stack; the size is clamped to {MIN_FONT_SIZE_PX}–{MAX_FONT_SIZE_PX}px.
    </p>
  </section>

  <section class="prefs-section" aria-labelledby="prefs-scale">
    <h2 id="prefs-scale">Interface scale</h2>
    <div class="prefs-field-row prefs-scale-row">
      <input
        type="range"
        min={MIN_UI_SCALE}
        max={MAX_UI_SCALE}
        step="0.05"
        value={uiScale}
        disabled={busy}
        aria-label="Interface scale"
        data-testid="ui-scale"
        oninput={onScale}
      />
      <span class="readout" data-testid="ui-scale-readout">{Math.round(uiScale * 100)}%</span>
    </div>
    <p class="prefs-note">
      The window and its text grow together, from {Math.round(MIN_UI_SCALE * 100)}% to
      {Math.round(MAX_UI_SCALE * 100)}%. The same range the zoom keys already use.
    </p>
  </section>

  <section class="prefs-section" aria-labelledby="prefs-fullscreen">
    <h2 id="prefs-fullscreen">Full screen</h2>
    <label class="prefs-check">
      <input
        type="checkbox"
        checked={fullscreen}
        disabled={busy}
        data-testid="fullscreen"
        onchange={onFullscreen}
      />
      <span>Use full screen for the main window</span>
    </label>
    <p class="prefs-note">
      Applied to the main window, not to this Preferences window. It can also be left on for the
      next launch without connecting again.
    </p>
  </section>
</div>

<style>
  .prefs-section {
    margin-top: var(--sp-5);
  }

  .prefs-section h2 {
    margin: 0 0 var(--sp-2);
    font-size: var(--fs-sm);
    font-weight: 700;
    letter-spacing: 0.12em;
    text-transform: uppercase;
    color: var(--text-3);
  }

  .prefs-field select {
    width: 100%;
    min-width: 0;
    padding: 6px var(--sp-2);
    background: var(--bg-void);
    border: 1px solid var(--line-strong);
    border-radius: var(--r-sm);
    color: inherit;
    outline: none;
  }

  .prefs-field select:focus {
    border-color: var(--amber-dim);
    box-shadow: 0 0 0 2px var(--amber-soft);
  }

  .prefs-colours {
    display: flex;
    flex-direction: column;
    gap: 1px;
    margin-top: var(--sp-2);
    border: 1px solid var(--line-strong);
    border-radius: var(--r-sm);
    overflow: hidden;
  }

  .prefs-colour {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    padding: 5px var(--sp-2);
    background: var(--surface-1);
  }

  .prefs-colour + .prefs-colour {
    border-top: 1px solid var(--line);
  }

  .prefs-swatch {
    flex: none;
    width: 30px;
    height: 22px;
    padding: 0;
    background: none;
    border: 1px solid var(--line-strong);
    border-radius: var(--r-xs);
  }

  .prefs-colour-name {
    flex: 1;
    min-width: 0;
    font-size: var(--fs-sm);
    color: var(--text-1);
  }

  .prefs-colour-default {
    flex: none;
    font-size: var(--fs-xs);
    color: var(--text-3);
  }

  .prefs-colour-reset {
    flex: none;
  }

  .prefs-scale-row {
    align-items: center;
    gap: var(--sp-3);
  }

  .prefs-check {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    margin: var(--sp-3) 0 var(--sp-2);
    font-size: var(--fs-sm);
    color: var(--text-1);
  }

  .prefs-check input {
    flex: none;
  }
</style>
