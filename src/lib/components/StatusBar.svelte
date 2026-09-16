<script lang="ts">
  import { store } from "../store.svelte";

  const LABELS: Record<string, string> = {
    connecting: "connecting",
    connected: "connected",
    disconnected: "disconnected",
    error: "error",
  };
</script>

<footer class="statusbar">
  <span class="dot {store.status}"></span>
  <span>{LABELS[store.status] ?? store.status}{store.statusMessage ? ` — ${store.statusMessage}` : ""}</span>

  <span class="kv"><span>server</span><b>{store.banner?.name ?? store.settings.host ?? "—"}</b></span>
  <span class="kv"><span>addr</span><b>{store.banner?.host ?? store.settings.host}:{store.banner?.port ?? store.settings.port}</b></span>
  {#if store.banner?.version}<span class="kv"><span>version</span><b>{store.banner.version}</b></span>{/if}
  {#if store.banner?.byte_order}<span class="kv"><span>endian</span><b>{store.banner.byte_order}</b></span>{/if}
  {#if store.room}
    <span class="kv"><span>room</span><b>{store.room.name}</b></span>
    <span class="kv"><span>here</span><b>{store.users.length}</b></span>
  {/if}
  {#if store.banner?.total_users != null}
    <span class="kv"><span>on server</span><b>{store.banner.total_users}</b></span>
  {/if}

  <span class="spacer"></span>
  <span class="kv"><span>zoom</span><b>×{store.zoom.toFixed(2)}</b></span>
  <span class="kv"><span>mode</span><b>{store.native ? "1:1" : "fit"}</b></span>
  {#if store.screen}<span class="kv"><span>frame</span><b>#{store.screen.version}</b></span>{/if}
</footer>
