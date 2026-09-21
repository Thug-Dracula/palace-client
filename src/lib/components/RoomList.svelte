<script lang="ts">
  import * as api from "../api";
  import { store } from "../store.svelte";
  import DetachButton from "./DetachButton.svelte";

  // The filter is per-window view state, not session state: each webview runs
  // its own store instance, so a detached list and the docked one never share
  // a filter (WINDOWS-ARCHITECTURE.md §2.2).
  const query = $derived(store.roomFilter.trim());

  async function enter(roomId: number) {
    if (!store.connected) {
      return;
    }
    await api.gotoRoom(roomId).catch(() => {});
  }
</script>

<div class="sidebar" data-panel="rooms">
  <div class="panel-head">
    <span>Rooms</span>
    <div class="panel-head-tools">
      <span class="count">{store.filteredRooms.length}/{store.rooms.length || "—"}</span>
      <DetachButton panel="rooms" />
    </div>
  </div>
  <div class="filter">
    <input bind:value={store.roomFilter} placeholder="filter rooms…" spellcheck="false" aria-label="Filter rooms" />
  </div>
  <div class="scroll">
    {#each store.filteredRooms as room (room.id)}
      <button
        class="row"
        class:active={store.room?.id === room.id}
        type="button"
        onclick={() => enter(room.id)}
        disabled={!store.connected}
        title={`room ${room.id}`}
      >
        <span class="name">{room.name || `(room ${room.id})`}</span>
        {#if room.users > 0}<span class="meta">{room.users}</span>{/if}
      </button>
    {/each}
    {#if !store.rooms.length}
      <div class="empty-note">{store.connected ? "No room list yet." : "Not connected."}</div>
    {:else if !store.filteredRooms.length}
      <div class="empty-note">No rooms match “{query}”.</div>
    {/if}
  </div>
</div>
