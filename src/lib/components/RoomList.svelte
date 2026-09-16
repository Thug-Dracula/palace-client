<script lang="ts">
  import * as api from "../api";
  import { store } from "../store.svelte";

  async function enter(roomId: number) {
    if (!store.connected) {
      return;
    }
    await api.gotoRoom(roomId).catch(() => {});
  }
</script>

<div class="sidebar">
  <div class="panel-head">
    <span>Rooms</span>
    <span class="count">{store.filteredRooms.length}/{store.rooms.length || "—"}</span>
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
    {/if}
  </div>
</div>
