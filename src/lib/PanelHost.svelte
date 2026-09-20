<script lang="ts">
  import * as api from "./api";
  import type { PanelId } from "./panels";
  import ChatPanel from "./components/ChatPanel.svelte";
  import PropBagDialog from "./components/PropBagDialog.svelte";
  import RoomList from "./components/RoomList.svelte";
  import UserList from "./components/UserList.svelte";
  import Viewport from "./components/Viewport.svelte";

  let { panel, detached = false }: { panel: PanelId; detached?: boolean } = $props();

  // Closing a detached panel window is a re-attach, never a discard: Rust
  // intercepts the close and tells the main window to dock the panel again.
  function reattach(): void {
    void api.closePanel(panel).catch(() => {});
  }
</script>

{#if detached}
  <div class="panel-placeholder" data-panel={panel}>
    <p>This panel is in its own window.</p>
    <button class="btn" type="button" onclick={reattach}>Reattach</button>
  </div>
{:else if panel === "room"}
  <Viewport />
{:else if panel === "users"}
  <UserList />
{:else if panel === "rooms"}
  <RoomList />
{:else if panel === "chat"}
  <ChatPanel />
{:else}
  <PropBagDialog onclose={reattach} />
{/if}

<style>
  .panel-placeholder {
    display: grid;
    place-content: center;
    gap: var(--sp-3);
    justify-items: center;
    min-height: 0;
    padding: var(--sp-4);
    background: var(--surface-1);
    color: var(--text-2);
    text-align: center;
  }

  .panel-placeholder p {
    margin: 0;
    font-size: var(--fs-sm);
  }
</style>
