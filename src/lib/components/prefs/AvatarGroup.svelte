<script lang="ts">
  import { AVATAR_OPTIONS, avatarPrefsFrom } from "../../avatarPrefs";
  import { prefs } from "../../prefsStore.svelte";

  const avatar = $derived(avatarPrefsFrom(prefs.values));

  /** The reason an option is disabled, straight from the support table. */
  function reason(key: string): string {
    return AVATAR_OPTIONS.find((option) => option.key === key)?.reason ?? "Not supported in this build yet.";
  }

  /** A plain readout, so the stored value is visible without being editable. */
  const slotSummary = $derived(
    avatar.savedAvatarSlots.length === 1
      ? "1 stored slot"
      : `${avatar.savedAvatarSlots.length} stored slots`,
  );

  // Nothing in this group writes: every control below is disabled because
  // neither option has backing behaviour. There is deliberately no save
  // function here — a control that appeared to work would be a lie.
</script>

<div class="prefs-group">
  <p class="prefs-note">
    Both options in this group come from the classic client but are <b>not supported</b> in this
    build yet. They are shown disabled so nothing here pretends to work.
  </p>

  <section class="prefs-section" aria-labelledby="prefs-avatar-props">
    <h2 id="prefs-avatar-props">Props</h2>

    <div class="prefs-option" data-testid="avatar-prop-animation-row">
      <label class="prefs-check">
        <input
          type="checkbox"
          checked={avatar.propAnimation}
          disabled
          data-testid="avatar-prop-animation"
        />
        <span>Animate props</span>
      </label>
      <span class="prefs-unsupported" data-testid="unsupported-prop_animation">Not supported</span>
      <p class="prefs-note">{reason("prop_animation")}</p>
    </div>
  </section>

  <section class="prefs-section" aria-labelledby="prefs-avatar-slots">
    <h2 id="prefs-avatar-slots">Saved avatar slots</h2>

    <div class="prefs-option" data-testid="avatar-saved-slots-row">
      <label class="prefs-check">
        <span>Saved slots</span>
        <input value={slotSummary} disabled readonly data-testid="avatar-saved-slots" />
      </label>
      <span class="prefs-unsupported" data-testid="unsupported-saved_avatar_slots">Not supported</span>
      <p class="prefs-note">{reason("saved_avatar_slots")}</p>
    </div>

    <div class="prefs-actions">
      <button class="btn" type="button" disabled data-testid="avatar-save-slot">
        Save current avatar to a slot
      </button>
      <button class="btn ghost" type="button" disabled data-testid="avatar-clear-slots">
        Clear saved slots
      </button>
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
