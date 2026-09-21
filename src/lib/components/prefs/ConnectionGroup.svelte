<script lang="ts">
  import { onMount } from "svelte";

  import * as api from "../../api";
  import {
    LAST_SERVERS_LIMIT,
    autoConnectFrom,
    describeServer,
    forgetServer,
    homePalaceFrom,
    identityFingerprint,
    lastServersFrom,
    rememberServer,
    type ServerRef,
  } from "../../connectionPrefs";
  import { prefs } from "../../prefsStore.svelte";

  /** `api.Settings` plus the identity the backend sends and the type omits. */
  interface SettingsWithIdentity extends api.Settings {
    identity?: unknown;
  }

  let host = $state("");
  let port = $state(9998);
  let username = $state("");
  let identity = $state<string | null>(null);
  let error = $state<string | null>(null);
  let ready = $state(false);
  let busy = $state(false);

  // The connection preferences come from the shell's already-loaded block, so
  // the group never reads the same file twice. `prefs.load()` runs when the
  // window mounts; these derivations update when a write echoes back.
  const lastServers = $derived(lastServersFrom(prefs.values));
  const homePalace = $derived(homePalaceFrom(prefs.values));
  const autoConnect = $derived(autoConnectFrom(prefs.values));

  onMount(async () => {
    try {
      const settings = (await api.getSettings()) as SettingsWithIdentity;
      host = settings.host;
      port = settings.port;
      username = settings.username;
      identity = identityFingerprint(settings.identity);
    } catch (cause) {
      error = `Could not read the current connection settings: ${String(cause)}`;
    } finally {
      ready = true;
    }
  });

  function validate(): string | null {
    if (!host.trim()) {
      return "Enter a server name.";
    }
    if (!Number.isInteger(port) || port < 1 || port > 65535) {
      return "The port must be a whole number between 1 and 65535.";
    }
    return null;
  }

  /** The server the fields currently describe, with the same rules the save uses. */
  function currentServer(): ServerRef {
    return { host: host.trim(), port, username: username.trim() || "Guest" };
  }

  /**
   * Persist a `prefs.connection.*` value that changes the next launch or the
   * next connection. Nothing here reconnects: the store's existing
   * "reconnect required" affordance is raised instead, and the user decides
   * when to act on it from the top bar.
   */
  async function saveConnectionPref(patch: Record<string, unknown>, message: string): Promise<void> {
    await prefs.setLive({ connection: patch }, message);
    if (!prefs.error) {
      prefs.reconnectRequired = true;
    }
  }

  async function commit() {
    if (!ready || busy) {
      return;
    }
    const problem = validate();
    if (problem) {
      error = problem;
      return;
    }
    error = null;
    busy = true;
    try {
      const saved = await prefs.saveConnection(host.trim(), port, username.trim() || "Guest");
      if (!saved) {
        return;
      }
      // The server just became the most recent one; the cap lives in the helper.
      const next = rememberServer(lastServers, currentServer());
      await prefs.setLive({ connection: { last_servers: next } }, null);
    } finally {
      busy = false;
    }
  }

  async function useServer(server: ServerRef) {
    if (busy) {
      return;
    }
    host = server.host;
    port = server.port;
    username = server.username;
    await commit();
  }

  async function forget(server: ServerRef) {
    if (busy) {
      return;
    }
    busy = true;
    try {
      await prefs.setLive(
        { connection: { last_servers: forgetServer(lastServers, server) } },
        `Forgot ${describeServer(server)}.`,
      );
    } finally {
      busy = false;
    }
  }

  async function setHomePalace() {
    if (busy) {
      return;
    }
    const problem = validate();
    if (problem) {
      error = problem;
      return;
    }
    error = null;
    busy = true;
    try {
      await saveConnectionPref({ home_palace: currentServer() }, "Home palace saved.");
    } finally {
      busy = false;
    }
  }

  async function clearHomePalace() {
    if (busy) {
      return;
    }
    busy = true;
    try {
      await saveConnectionPref({ home_palace: null }, "Home palace cleared.");
    } finally {
      busy = false;
    }
  }

  async function toggleAutoConnect(event: Event) {
    const box = event.currentTarget as HTMLInputElement;
    if (busy) {
      return;
    }
    busy = true;
    try {
      await saveConnectionPref(
        { auto_connect: box.checked },
        box.checked ? "The launch offer is on." : "The launch offer is off.",
      );
      if (prefs.error) {
        // The write failed, so put the box back where it was.
        box.checked = autoConnect;
      }
    } finally {
      busy = false;
    }
  }
</script>

<div class="prefs-group">
  <div class="prefs-field-row">
    <label class="prefs-field grow">
      <span>Server</span>
      <input bind:value={host} spellcheck="false" autocomplete="off" onchange={commit} />
    </label>
    <label class="prefs-field port">
      <span>Port</span>
      <input type="number" min="1" max="65535" bind:value={port} onchange={commit} />
    </label>
    <label class="prefs-field grow">
      <span>User name</span>
      <input bind:value={username} spellcheck="false" autocomplete="off" onchange={commit} />
    </label>
  </div>

  {#if error}
    <p class="prefs-error" role="alert">{error}</p>
  {/if}

  {#if prefs.reconnectRequired}
    <p class="prefs-reconnect" role="status">
      Reconnect required — these settings take effect on your next connection. The current session
      is left alone, so nothing is dropped underneath you.
    </p>
  {:else}
    <p class="prefs-note">
      These options change what a future connection uses. They never reconnect on their own; after
      a change you decide when to reconnect from the top bar.
    </p>
  {/if}

  <section class="prefs-section" aria-labelledby="prefs-last-servers">
    <h2 id="prefs-last-servers">Last servers</h2>
    {#if lastServers.length === 0}
      <p class="prefs-note" data-testid="last-servers-empty">
        No servers yet. Every server you save above is remembered here, most recent first.
      </p>
    {:else}
      <ul class="prefs-server-list">
        {#each lastServers as server (describeServer(server))}
          <li class="prefs-server">
            <button
              class="prefs-server-load"
              type="button"
              disabled={busy}
              onclick={() => useServer(server)}
              title="Use {describeServer(server)} for the next connection"
            >
              <span class="prefs-server-host">{describeServer(server)}</span>
              <span class="prefs-server-user">{server.username}</span>
            </button>
            <button
              class="btn ghost prefs-server-forget"
              type="button"
              disabled={busy}
              onclick={() => forget(server)}
              title="Forget {describeServer(server)}"
            >
              Forget
            </button>
          </li>
        {/each}
      </ul>
      <p class="prefs-note">
        The {LAST_SERVERS_LIMIT} most recent servers, newest first. Choosing one loads it above and
        saves it for the next connection.
      </p>
    {/if}
  </section>

  <section class="prefs-section" aria-labelledby="prefs-home-palace">
    <h2 id="prefs-home-palace">Home palace</h2>
    {#if homePalace}
      <p class="prefs-note" data-testid="home-palace">
        Home palace: <b>{describeServer(homePalace)}</b>
        as {homePalace.username}.
      </p>
    {:else}
      <p class="prefs-note" data-testid="home-palace">No home palace is set.</p>
    {/if}
    <div class="prefs-actions">
      <button class="btn" type="button" disabled={busy || !ready} onclick={setHomePalace}>
        Use the fields above as my home palace
      </button>
      {#if homePalace}
        <button class="btn ghost" type="button" disabled={busy} onclick={clearHomePalace}>
          Forget home palace
        </button>
      {/if}
    </div>
    <label class="prefs-check">
      <input
        type="checkbox"
        checked={autoConnect}
        disabled={busy || !homePalace}
        onchange={toggleAutoConnect}
        data-testid="auto-connect"
      />
      <span>Offer my home palace when this client starts</span>
    </label>
    <p class="prefs-note">
      {#if homePalace}
        On the next launch this client offers the home palace in a banner; it never connects on its own.
      {:else}
        Set a home palace first — the launch offer needs one to name.
      {/if}
    </p>
  </section>

  <section class="prefs-section" aria-labelledby="prefs-identity">
    <h2 id="prefs-identity">This install's identity</h2>
    <p class="prefs-note" data-testid="identity">
      {#if identity}
        Identity <b>{identity}</b> — read only. It is minted once on the first run and is never
        edited here.
      {:else}
        No identity is recorded yet; one is minted on the next launch.
      {/if}
    </p>
    <p class="prefs-note">
      Passwords are never stored. This group has no password field, and nothing on this page can
      write one to the settings file.
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

  .prefs-server-list {
    margin: 0;
    padding: 0;
    list-style: none;
    border: 1px solid var(--line-strong);
    border-radius: var(--r-sm);
    overflow: hidden;
  }

  .prefs-server {
    display: flex;
    align-items: stretch;
    gap: 1px;
    background: var(--line);
  }

  .prefs-server + .prefs-server {
    border-top: 1px solid var(--line);
  }

  .prefs-server-load {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: var(--sp-3);
    padding: 6px var(--sp-3);
    background: var(--surface-1);
    border: 0;
    text-align: left;
    color: var(--text-1);
  }

  .prefs-server-load:hover,
  .prefs-server-load:focus-visible {
    background: var(--surface-2);
    color: var(--text-0);
    outline: none;
  }

  .prefs-server-host {
    font-family: var(--font-mono);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .prefs-server-user {
    flex: none;
    font-size: var(--fs-xs);
    color: var(--text-3);
  }

  .prefs-server-forget {
    flex: none;
    border-radius: 0;
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
