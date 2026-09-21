<script lang="ts">
  import {
    NOTIFICATION_SCOPE_OPTIONS,
    notificationOption,
    notificationScopeFrom,
    notificationsPrefsFrom,
    scopePatch,
    type NotificationScope,
  } from "../../notificationsPrefs";
  import { prefs } from "../../prefsStore.svelte";

  // Mirrors of the stored values, so the controls keep what the user chose
  // while a save is in flight, then resync from the file when it echoes back.
  let enabled = $state(false);
  let scope = $state<NotificationScope>("all");
  let busy = $state(false);
  let error = $state<string | null>(null);

  const notifications = $derived(notificationsPrefsFrom(prefs.values));

  // Resync the controls whenever the stored notifications block changes (load,
  // save echo, restore defaults).
  $effect(() => {
    enabled = notifications.enabled;
    scope = notificationScopeFrom(notifications);
  });

  /** The reason an option is disabled, straight from the support table. */
  function reason(key: string): string {
    return notificationOption(key)?.reason ?? "Not supported in this build yet.";
  }

  /**
   * Persist a `notifications.*` patch.
   *
   * The write goes first: a failed write leaves the controls exactly as they
   * were, so a switch can never show a state the file does not have. The Rust
   * event pump reads the stored block, so the change takes effect on the next
   * message with no reconnect.
   */
  async function save(patch: Record<string, unknown>, message: string): Promise<boolean> {
    if (busy) {
      return false;
    }
    busy = true;
    error = null;
    try {
      await prefs.setLive({ notifications: patch }, message);
      if (prefs.error) {
        error = prefs.error;
        return false;
      }
      return true;
    } finally {
      busy = false;
    }
  }

  async function onEnabled(event: Event): Promise<void> {
    const box = event.currentTarget as HTMLInputElement;
    const wanted = box.checked;
    const ok = await save(
      { enabled: wanted },
      wanted ? "Desktop notifications on." : "Desktop notifications off.",
    );
    if (!ok) {
      box.checked = notifications.enabled;
    }
  }

  async function onScope(event: Event): Promise<void> {
    const select = event.currentTarget as HTMLSelectElement;
    const wanted = select.value as NotificationScope;
    if (!(await save(scopePatch(wanted), "Notification scope saved."))) {
      select.value = notificationScopeFrom(notifications);
    }
  }
</script>

<div class="prefs-group" data-testid="notifications-group">
  {#if error}
    <p class="prefs-error" role="alert">{error}</p>
  {/if}

  <section class="prefs-section" aria-labelledby="prefs-notifications-alerts">
    <h2 id="prefs-notifications-alerts">Desktop alerts</h2>
    <p class="prefs-note">
      Alerts are <b>off until you switch them on</b>. When on, this client asks your desktop to
      show a notification for a message that names you, for a private message, or both. The
      in-app notices you already see in the room are separate and always keep working, whether or
      not these alerts are on.
    </p>

    <label class="prefs-check">
      <input
        type="checkbox"
        checked={enabled}
        disabled={busy}
        data-testid="notifications-enabled"
        onchange={onEnabled}
      />
      <span>Show desktop notifications</span>
    </label>

    <label class="prefs-check">
      <span>Notify me for</span>
      <select
        value={scope}
        disabled={busy || !enabled}
        data-testid="notifications-scope"
        onchange={onScope}
      >
        {#each NOTIFICATION_SCOPE_OPTIONS as option (option.id)}
          <option value={option.id}>{option.label}</option>
        {/each}
      </select>
    </label>
    <p class="prefs-note">
      A mention is a room message that names you as a whole word. A user on the Mute/ignore list
      never raises an alert, and neither do the server's own notices.
    </p>
  </section>

  <section class="prefs-section" aria-labelledby="prefs-notifications-extra">
    <h2 id="prefs-notifications-extra">Alert details</h2>
    <p class="prefs-note">
      This option is from the specification but is <b>not supported</b> in this build yet. It is
      shown disabled, and changing it would not do anything — so it cannot be changed.
    </p>

    <div class="prefs-option" data-testid="notifications-sound-row">
      <label class="prefs-check">
        <input
          type="checkbox"
          checked={notifications.sound}
          disabled
          data-testid="notifications-sound"
        />
        <span>Play a sound</span>
      </label>
      <span class="prefs-unsupported" data-testid="unsupported-sound">Not supported</span>
      <p class="prefs-note">{reason("sound")}</p>
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
