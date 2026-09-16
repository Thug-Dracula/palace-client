<script lang="ts">
  import { onMount } from "svelte";
  import * as api from "../api";
  import { store } from "../store.svelte";

  let busy = $state(false);
  let error = $state<string | null>(null);

  onMount(() => {
    void api
      .getSettings()
      .then((settings) => {
        store.settings = settings;
      })
      .catch(() => {});
  });

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (busy) {
      return;
    }
    busy = true;
    error = null;
    try {
      await api.connect(store.settings);
    } catch (cause) {
      error = String(cause);
    } finally {
      busy = false;
    }
  }

  async function drop() {
    error = null;
    try {
      await api.disconnect();
    } catch (cause) {
      error = String(cause);
    }
  }
</script>

<header class="topbar">
  <div class="brand">
    PALACE<span class="sub">client</span>
  </div>

  <form class="connect-form" onsubmit={submit}>
    <div class="field host">
      <label for="host">host</label>
      <input id="host" bind:value={store.settings.host} spellcheck="false" autocomplete="off" />
    </div>
    <div class="field port">
      <label for="port">port</label>
      <input id="port" type="number" min="1" max="65535" bind:value={store.settings.port} />
    </div>
    <div class="field user">
      <label for="user">name</label>
      <input id="user" bind:value={store.settings.username} spellcheck="false" autocomplete="off" />
    </div>
    <button class="btn primary" type="submit" disabled={busy}>
      {store.connected ? "Reconnect" : "Connect"}
    </button>
    <button class="btn" type="button" onclick={drop} disabled={!store.connected}>Disconnect</button>
    <button class="btn ghost" type="button" onclick={() => api.refresh()} disabled={!store.connected}>
      Refresh
    </button>
    {#if error}<span class="topbar-error">{error}</span>{/if}
  </form>
</header>
