<script lang="ts">
  import {
    MUTE_CAPABILITIES,
    addMuteEntry,
    mutePrefsFrom,
    normaliseMuteName,
    removeMuteEntry,
    type MuteEntry,
  } from "../../mutePrefs";
  import { prefs } from "../../prefsStore.svelte";
  import { store } from "../../store.svelte";

  let draft = $state("");
  let entryError = $state<string | null>(null);
  let busy = $state(false);
  let error = $state<string | null>(null);

  const mute = $derived(mutePrefsFrom(prefs.values));

  // Live application: a stored change drives the store's rules, so a message
  // that arrives after the save is dropped in this window too.
  $effect(() => {
    store.applyMute(mute);
  });

  /** Persist a `mute.*` patch. A failed write leaves the list untouched. */
  async function save(patch: Record<string, unknown>, message: string): Promise<boolean> {
    if (busy) {
      return false;
    }
    busy = true;
    error = null;
    try {
      await prefs.setLive({ mute: patch }, message);
      if (prefs.error) {
        error = prefs.error;
        return false;
      }
      return true;
    } finally {
      busy = false;
    }
  }

  async function onAdd(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    entryError = null;
    const added = addMuteEntry(mute, draft);
    if (!added.ok) {
      entryError = added.error;
      return;
    }
    const name = draft.trim();
    if (await save({ identities: added.prefs.identities }, `${name} ignored.`)) {
      draft = "";
    }
  }

  async function onRemove(entry: MuteEntry): Promise<void> {
    entryError = null;
    const next = removeMuteEntry(mute, entry.name);
    await save({ identities: next.identities }, `${entry.name} is no longer ignored.`);
  }

  async function onIgnoreAll(event: Event): Promise<void> {
    const box = event.currentTarget as HTMLInputElement;
    const wanted = box.checked;
    const message = wanted ? "Ignoring everyone but yourself." : "Ignore-all is off.";
    if (!(await save({ ignore_all: wanted }, message))) {
      box.checked = mute.ignoreAll;
    }
  }
</script>

<div class="prefs-group" data-testid="mute-group">
  {#if error ?? entryError}
    <p class="prefs-error" role="alert" data-testid="mute-error">{error ?? entryError}</p>
  {/if}

  <section class="prefs-section" aria-labelledby="prefs-mute-list">
    <h2 id="prefs-mute-list">Ignore list</h2>
    <p class="prefs-note" data-testid="mute-classic-note">
      The classic PalaceChat client had no ignore list: an avatar's menu could mute someone for the
      current session only, and the choice was forgotten as soon as you left. A list that is
      remembered is an enhancement added by Palace Client.
    </p>
    <p class="prefs-note" data-testid="mute-local-note">
      This list is kept on this computer. Palace Client never asks the server to mute anyone, so no
      one else's view changes and nothing is stored on the server.
    </p>

    <label class="prefs-check">
      <input
        type="checkbox"
        checked={mute.ignoreAll}
        disabled={busy}
        data-testid="mute-ignore-all"
        onchange={onIgnoreAll}
      />
      <span>Ignore everyone (your own messages still show)</span>
    </label>

    <form class="prefs-actions" onsubmit={onAdd}>
      <label class="prefs-field grow">
        <span>Add a user by name</span>
        <input
          type="text"
          bind:value={draft}
          disabled={busy}
          placeholder="Ada"
          data-testid="mute-add-input"
        />
      </label>
      <button class="btn" type="submit" disabled={busy} data-testid="mute-add">Add</button>
    </form>

    {#if mute.identities.length === 0}
      <p class="prefs-note" data-testid="mute-empty">Nobody is ignored yet.</p>
    {:else}
      <ul class="mute-list" data-testid="mute-entries">
        {#each mute.identities as entry (normaliseMuteName(entry.name))}
          <li class="mute-entry" data-testid={`mute-entry-${normaliseMuteName(entry.name)}`}>
            <span class="mute-name">{entry.name}</span>
            <button
              class="btn"
              type="button"
              disabled={busy}
              aria-label={`Stop ignoring ${entry.name}`}
              onclick={() => onRemove(entry)}
              data-testid={`mute-remove-${normaliseMuteName(entry.name)}`}
            >
              Remove
            </button>
          </li>
        {/each}
      </ul>
    {/if}
  </section>

  <section class="prefs-section" aria-labelledby="prefs-mute-behaviour">
    <h2 id="prefs-mute-behaviour">What ignoring does</h2>
    <p class="prefs-note">
      Ignoring changes only what this client shows you. Anything marked
      <b>Not supported</b> is not done at all, and is listed so the limitation is visible rather
      than implied.
    </p>

    {#each MUTE_CAPABILITIES as capability (capability.key)}
      <div class="prefs-option" data-testid={`mute-capability-${capability.key}`}>
        <span class="mute-capability">{capability.label}</span>
        {#if capability.support === "unsupported"}
          <span class="prefs-unsupported" data-testid={`mute-unsupported-${capability.key}`}>
            Not supported
          </span>
        {/if}
        <p class="prefs-note">{capability.reason ?? capability.summary}</p>
      </div>
    {/each}
  </section>
</div>

<style>
  .prefs-section {
    margin-bottom: var(--sp-5);
  }

  .prefs-option {
    margin: var(--sp-3) 0;
  }

  .prefs-option .prefs-note {
    margin: var(--sp-1) 0 0;
    padding-left: var(--sp-4);
  }

  .prefs-check {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    margin: var(--sp-3) 0;
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

  .mute-capability {
    display: inline-block;
  }

  .mute-list {
    margin: var(--sp-2) 0;
    padding: 0;
    list-style: none;
  }

  .mute-entry {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    margin: var(--sp-1) 0;
  }

  .mute-name {
    min-width: 9rem;
  }
</style>
