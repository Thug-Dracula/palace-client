<script lang="ts">
  import { faceColor, store } from "../store.svelte";
  import DetachButton from "./DetachButton.svelte";
</script>

<div class="sidebar" data-panel="users">
  <div class="panel-head">
    <span>In room</span>
    <div class="panel-head-tools">
      <span class="count">{store.users.length}</span>
      <DetachButton panel="users" />
    </div>
  </div>
  <div class="scroll">
    {#each store.users as user (user.id)}
      <div class="row" class:self={user.is_self} class:away={user.away}>
        <span class="face" style="color:{faceColor(user.color)}"></span>
        <span class="name">{user.name}</span>
        {#if user.is_self}<span class="you-tag">you</span>{/if}
        <span class="meta">{user.away ? "away" : ""}</span>
      </div>
    {/each}
    {#if !store.users.length}
      <div class="empty-note">{store.connected ? "Nobody here." : "Not connected."}</div>
    {/if}
  </div>
</div>
