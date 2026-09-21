<script lang="ts">
  import { onMount } from "svelte";

  import * as api from "../api";
  import { describeServer, launchOffer, sameServer, type ServerRef } from "../connectionPrefs";
  import { store } from "../store.svelte";

  /**
   * The launch offer for a home palace (Task 20).
   *
   * `prefs.connection.auto_connect` is a preference, never an instruction to
   * connect: at launch this banner *offers* the home palace and waits for a
   * click. "Not now" only dismisses the banner for this session — it writes
   * nothing. The offer stays hidden when the client is already connected to
   * that same palace, and if the preference cannot be read there is simply no
   * offer (a broken preference file must not take the shell down with it).
   */
  let offer = $state<ServerRef | null>(null);
  let ready = $state(false);
  let dismissed = $state(false);
  let busy = $state(false);
  let error = $state<string | null>(null);

  const visible = $derived(
    ready &&
      !dismissed &&
      offer !== null &&
      !(store.connected && sameServer(store.settings, offer)),
  );

  onMount(async () => {
    try {
      offer = launchOffer(await api.getPrefs());
    } catch {
      offer = null;
    } finally {
      ready = true;
    }
  });

  async function connect() {
    if (!offer || busy) {
      return;
    }
    busy = true;
    error = null;
    try {
      store.settings = { ...store.settings, ...offer };
      await api.connect(store.settings);
      dismissed = true;
    } catch (cause) {
      error = `Could not connect to the home palace: ${String(cause)}`;
    } finally {
      busy = false;
    }
  }
</script>

{#if visible && offer}
  <aside class="home-offer" aria-label="Home palace offer">
    <span class="home-offer-text">
      Home palace: <b>{describeServer(offer)}</b>
      as {offer.username}
    </span>
    <button class="btn primary" type="button" disabled={busy} onclick={connect}>Connect</button>
    <button class="btn ghost" type="button" disabled={busy} onclick={() => (dismissed = true)}>
      Not now
    </button>
    {#if error}
      <span class="home-offer-error" role="alert">{error}</span>
    {/if}
  </aside>
{/if}

<style>
  .home-offer {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: var(--sp-2);
    padding: var(--sp-2) var(--sp-4);
    background: var(--amber-soft);
    border-bottom: 1px solid var(--line-strong);
    font-size: var(--fs-sm);
    color: var(--text-1);
  }

  .home-offer-text {
    flex: 1;
    min-width: 0;
  }

  .home-offer-text b {
    font-family: var(--font-mono);
    color: var(--text-0);
  }

  .home-offer-error {
    flex-basis: 100%;
    color: var(--red);
  }
</style>
