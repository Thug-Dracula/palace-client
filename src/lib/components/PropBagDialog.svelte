<script lang="ts">
  import { onMount, tick } from "svelte";
  import * as api from "../api";
  import { store, MAX_PROPS } from "../store.svelte";

  let { onclose }: { onclose: () => void } = $props();

  type Filter = "all" | "fav" | "trash";

  let entries = $state<api.PropEntry[]>([]);
  let busy = $state(true);
  let error = $state<string | null>(null);
  let loadFailed = $state(false);
  let filter = $state<Filter>("all");
  let dialog: HTMLDivElement | undefined = $state();

  // Worn state always comes from the self user, so the grid can never disagree
  // with what the runtime actually has on the avatar.
  const worn = $derived(store.self?.props ?? []);
  const wornCount = $derived(worn.length);
  const hasTrash = $derived(entries.some((entry) => entry.trash));
  const visible = $derived(
    entries.filter((entry) => {
      if (filter === "fav") {
        return entry.fav;
      }
      if (filter === "trash") {
        return entry.trash;
      }
      return true;
    }),
  );

  function entryLabel(entry: api.PropEntry): string {
    return entry.name && entry.name.trim() !== "" ? entry.name : `#${entry.id}`;
  }

  function isWorn(id: number): boolean {
    return worn.includes(id);
  }

  function toggle(id: number): void {
    const result = store.toggleProp(id);
    if (!result.ok) {
      error = result.error ?? "That prop could not be worn.";
      return;
    }
    error = null;
  }

  async function load(): Promise<void> {
    busy = true;
    error = null;
    loadFailed = false;
    try {
      entries = await api.propsCatalog();
    } catch (cause) {
      entries = [];
      loadFailed = true;
      error = `Could not load your prop bag: ${String(cause)}`;
    } finally {
      busy = false;
    }
  }

  function onKeydown(event: KeyboardEvent) {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      onclose();
      return;
    }
    if (event.key !== "Tab" || !dialog) {
      return;
    }
    const focusables = [...dialog.querySelectorAll<HTMLElement>("button:not([disabled])")];
    const first = focusables.at(0);
    const last = focusables.at(-1);
    if (!first || !last) {
      return;
    }
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  }

  onMount(() => {
    void (async () => {
      await load();
      await tick();
      const target =
        dialog?.querySelector<HTMLButtonElement>(".prop-tile") ??
        dialog?.querySelector<HTMLButtonElement>(".dialog-x");
      target?.focus();
    })();
  });
</script>

<div class="dialog-backdrop">
  <div
    class="dialog prop-dialog"
    role="dialog"
    aria-modal="true"
    aria-label="Prop Bag"
    tabindex="-1"
    bind:this={dialog}
    onkeydown={onKeydown}
    oncontextmenu={(event) => event.preventDefault()}
  >
    <div class="dialog-head">
      <span>Prop Bag</span>
      <button class="dialog-x" type="button" aria-label="Close the prop bag" onclick={onclose}>×</button>
    </div>

    <div class="dialog-body">
      {#if busy}
        <p class="dialog-hint">Loading your prop bag…</p>
      {:else if loadFailed}
        <p class="dialog-hint">Your prop bag could not be loaded.</p>
        <button class="btn" type="button" onclick={load}>Try again</button>
      {:else if entries.length === 0}
        <p class="dialog-hint">No props in your bag yet.</p>
      {:else}
        <div class="prop-toolbar">
          <div class="seg prop-filters" role="group" aria-label="Filter props">
            <button
              class="btn"
              class:on={filter === "all"}
              type="button"
              aria-pressed={filter === "all"}
              onclick={() => (filter = "all")}
            >
              All
            </button>
            <button
              class="btn"
              class:on={filter === "fav"}
              type="button"
              aria-pressed={filter === "fav"}
              onclick={() => (filter = "fav")}
            >
              Favorites
            </button>
            {#if hasTrash}
              <button
                class="btn"
                class:on={filter === "trash"}
                type="button"
                aria-pressed={filter === "trash"}
                onclick={() => (filter = "trash")}
              >
                Trash
              </button>
            {/if}
          </div>
        </div>

        {#if visible.length === 0}
          <p class="dialog-hint">Nothing here.</p>
        {:else}
          <div class="prop-grid" role="group" aria-label="Props">
            {#each visible as entry (entry.id)}
              <button
                class="prop-tile"
                class:worn={isWorn(entry.id)}
                type="button"
                title={entryLabel(entry)}
                aria-label={`${isWorn(entry.id) ? "Take off" : "Wear"} ${entryLabel(entry)}`}
                aria-pressed={isWorn(entry.id)}
                onclick={() => toggle(entry.id)}
              >
                <img
                  class="prop-thumb"
                  src={api.propThumbUrl(entry.id)}
                  alt=""
                  draggable="false"
                  loading="lazy"
                />
              </button>
            {/each}
          </div>
        {/if}
      {/if}

      {#if error}<p class="dialog-error">{error}</p>{/if}

      <div class="dialog-actions">
        <span class="prop-count">Worn {wornCount} / {MAX_PROPS}</span>
        <button
          class="btn"
          type="button"
          onclick={() => store.takeOffAvatar()}
          disabled={busy || wornCount === 0}
        >
          Take all off
        </button>
        <button class="btn primary" type="button" onclick={onclose} disabled={busy}>Close</button>
      </div>
    </div>
  </div>
</div>
