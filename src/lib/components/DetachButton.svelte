<script lang="ts">
  import { panelLayout } from "../panelLayout.svelte";
  import type { PanelId } from "../panels";
  import { isPanelWindow } from "../windowRole";

  let { panel }: { panel: PanelId } = $props();

  // Detaching is a `main`-shell action. Inside a panel window the way back is
  // closing that window, so the control is not rendered there at all — which
  // also keeps every panel header identical in both places.
  const detachable = !isPanelWindow();

  // The store holds the reason, not this component: a failed detach re-docks the
  // panel and remounts this button, so a local error would be lost. The catch
  // only stops the rejection being unhandled; the store does the reporting.
  const error = $derived(panelLayout.errorFor(panel));

  function onDetach(event: MouseEvent): void {
    // The header around this button is not interactive today, but stopping the
    // event keeps the control safe if a header ever becomes clickable.
    event.stopPropagation();
    void panelLayout.detach(panel).catch(() => {});
  }
</script>

{#if detachable}
  <button
    class="btn ghost detach-btn"
    class:failed={error !== null}
    type="button"
    data-detach={panel}
    title={error ? `Could not detach: ${error}` : "Detach to its own window"}
    aria-label={error
      ? `Could not detach the ${panel} panel: ${error}`
      : `Detach the ${panel} panel to its own window`}
    onclick={onDetach}
  >
    <!-- A box with an arrow leaving it: the conventional "open in its own window". -->
    <svg viewBox="0 0 12 12" width="13" height="13" aria-hidden="true" focusable="false">
      <path class="box" d="M4.5 1.5h-3v9h9v-3" />
      <path class="arrow" d="M6.5 1.5h4v4" />
      <path class="arrow" d="M10.5 1.5 5 7" />
    </svg>
  </button>
  {#if error}
    <span class="detach-error" role="alert">Couldn't detach: {error}</span>
  {/if}
{/if}

<style>
  .detach-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex: none;
    padding: 4px 5px;
    line-height: 1;
    color: var(--text-2);
  }

  .detach-btn:hover,
  .detach-btn:focus-visible {
    color: var(--amber);
  }

  .detach-btn svg {
    display: block;
  }

  .detach-btn.failed {
    color: var(--red);
  }

  .detach-error {
    color: var(--red);
    font-size: 0.7rem;
    line-height: 1.2;
  }

  .box,
  .arrow {
    fill: none;
    stroke: currentColor;
    stroke-width: 1.1;
    stroke-linecap: square;
  }
</style>
