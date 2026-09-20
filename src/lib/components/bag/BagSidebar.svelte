<script lang="ts">
  import type { BagOutfit } from "../../api";
  import { shelfHealth, type BagCollectionView } from "../../bagFilters";
  import { BAG } from "../editor/strings";
  import OutfitManager from "./OutfitManager.svelte";

  let {
    collections,
    activeCollection,
    totalCount,
    tab,
    outfits,
    wornCount,
    busy,
    onSelectCollection,
    onTab,
    onSaveOutfit,
    onApplyOutfit,
    onRenameOutfit,
    onDeleteOutfit,
  }: {
    collections: BagCollectionView[];
    activeCollection: string | null;
    totalCount: number;
    tab: "collections" | "outfits";
    outfits: BagOutfit[];
    wornCount: number;
    busy: boolean;
    onSelectCollection: (name: string | null) => void;
    onTab: (tab: "collections" | "outfits") => void;
    onSaveOutfit: (name: string) => void;
    onApplyOutfit: (name: string) => void;
    onRenameOutfit: (from: string, to: string) => void;
    onDeleteOutfit: (name: string) => void;
  } = $props();
</script>

<aside class="bag-rail" aria-label={BAG.sidebar.region}>
  <div class="seg bag-rail-tabs" role="group" aria-label={BAG.sidebar.panel}>
    <button
      class="btn"
      class:on={tab === "collections"}
      type="button"
      aria-pressed={tab === "collections"}
      onclick={() => onTab("collections")}
    >
      {BAG.sidebar.collections}
    </button>
    <button
      class="btn"
      class:on={tab === "outfits"}
      type="button"
      aria-pressed={tab === "outfits"}
      onclick={() => onTab("outfits")}
    >
      {BAG.sidebar.outfits}
    </button>
  </div>

  {#if tab === "collections"}
    <nav class="bag-collections" aria-label={BAG.sidebar.collectionsLabel}>
      <button
        class="bag-collection"
        class:on={activeCollection === null}
        type="button"
        aria-pressed={activeCollection === null}
        onclick={() => onSelectCollection(null)}
      >
        <span class="bag-collection-name">{BAG.sidebar.all}</span>
        <span class="bag-collection-count">{totalCount}</span>
      </button>
      {#each collections as collection (collection.name)}
        {@const health = collection.health ? shelfHealth(collection.health) : null}
        <button
          class="bag-collection"
          class:on={activeCollection === collection.name}
          type="button"
          title={collection.path}
          aria-pressed={activeCollection === collection.name}
          onclick={() => onSelectCollection(collection.name)}
        >
          <span class="bag-collection-name">{collection.name}</span>
          {#if collection.writable}
            <span class="bag-tag">{BAG.sidebar.writable}</span>
          {/if}
          <span class="bag-collection-count">{collection.count}</span>
        </button>
        {#if health}
          <p class="bag-shelf-health tone-{health.tone}">{health.detail}</p>
          {#if collection.health?.error}
            <p class="bag-shelf-health tone-bad">{collection.health.error}</p>
          {/if}
        {/if}
      {/each}
      {#if collections.length === 0}
        <p class="bag-rail-hint">{BAG.sidebar.none}</p>
      {/if}
    </nav>
  {:else}
    <OutfitManager
      {outfits}
      {wornCount}
      {busy}
      onSave={onSaveOutfit}
      onApply={onApplyOutfit}
      onRename={onRenameOutfit}
      onDelete={onDeleteOutfit}
    />
  {/if}
</aside>

<style>
  .bag-rail {
    display: flex;
    flex-direction: column;
    gap: var(--sp-3);
    min-width: 0;
    padding-right: var(--sp-3);
    border-right: 1px solid var(--line);
    overflow: auto;
  }

  .bag-rail-tabs {
    flex: none;
  }

  .bag-rail-tabs .btn {
    flex: 1;
  }

  .bag-collections {
    display: flex;
    flex-direction: column;
    gap: var(--sp-1);
    min-width: 0;
  }

  .bag-collection {
    display: flex;
    align-items: center;
    gap: var(--sp-1);
    width: 100%;
    padding: var(--sp-2);
    background: var(--surface-1);
    border: 1px solid transparent;
    border-radius: var(--r-sm);
    color: var(--text-1);
    font-size: var(--fs-sm);
    text-align: left;
    cursor: pointer;
    transition: background var(--t-fast), border-color var(--t-fast), color var(--t-fast);
  }

  .bag-collection:hover,
  .bag-collection:focus-visible {
    background: var(--surface-3);
    color: var(--text-0);
  }

  .bag-collection.on {
    background: var(--surface-3);
    border-color: var(--amber-dim);
    color: var(--text-0);
  }

  .bag-collection-name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .bag-collection-count {
    flex: none;
    color: var(--text-3);
    font-size: var(--fs-xs);
    font-variant-numeric: tabular-nums;
  }

  .bag-tag {
    flex: none;
    padding: 0 var(--sp-1);
    border: 1px solid var(--line-strong);
    border-radius: var(--r-xs);
    color: var(--amber);
    font-size: calc(var(--fs-xs) - 2px);
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }

  .bag-shelf-health {
    margin: 0 0 var(--sp-1) var(--sp-2);
    font-size: calc(var(--fs-xs) - 1px);
    line-height: 1.5;
    color: var(--text-3);
    font-variant-numeric: tabular-nums;
  }

  .bag-shelf-health.tone-ok {
    color: var(--green);
  }

  .bag-shelf-health.tone-warn {
    color: var(--amber);
  }

  .bag-shelf-health.tone-bad {
    color: var(--red);
  }

  .bag-rail-hint {
    margin: 0;
    font-size: var(--fs-xs);
    color: var(--text-3);
  }
</style>
