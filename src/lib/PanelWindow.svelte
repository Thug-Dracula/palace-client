<script lang="ts">
  import { onMount } from "svelte";

  import PanelHost from "./PanelHost.svelte";
  import type { PanelId } from "./panels";
  import { startSession } from "./session";

  let { panel }: { panel: PanelId } = $props();

  onMount(() => {
    const session = startSession();
    return () => session.stop();
  });
</script>

<div class="panel-window">
  <PanelHost {panel} />
</div>

<style>
  .panel-window {
    display: grid;
    grid-template-rows: minmax(0, 1fr);
    height: 100vh;
  }
</style>
