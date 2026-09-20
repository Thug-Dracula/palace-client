<script lang="ts">
  import * as api from "../../api";
  import type { BagPropEntry } from "../../api";
  import { bagEntryLabel, propBadges } from "../../bagFilters";
  import { BAG } from "../editor/strings";

  let {
    entry,
    worn,
    selected,
    showIndicators = true,
    thumbVersion = 0,
    onwear,
    onselect,
  }: {
    entry: BagPropEntry;
    worn: boolean;
    selected: boolean;
    showIndicators?: boolean;
    thumbVersion?: number;
    onwear: (entry: BagPropEntry) => void;
    onselect: (entry: BagPropEntry) => void;
  } = $props();

  const label = $derived(bagEntryLabel(entry));
  const badges = $derived(showIndicators ? propBadges(entry.flags) : []);
  const badgeWords = $derived(badges.map((badge) => badge.title).join(", "));
  const state = $derived([
    worn ? "worn" : "",
    selected ? "selected" : "",
    entry.trash ? "trashed" : "",
  ]
    .filter(Boolean)
    .join(" "));

  function activate(event: MouseEvent) {
    if (event.ctrlKey || event.metaKey) {
      onselect(entry);
      return;
    }
    onwear(entry);
  }
</script>

<div class="bag-tile" class:worn class:selected class:trashed={entry.trash} data-state={state}>
  <button
    class="bag-tile-wear"
    type="button"
    title={label}
    aria-label={`${worn ? BAG.tile.takeOff : BAG.tile.wear} ${label}${badgeWords ? ` (${badgeWords})` : ""}`}
    aria-pressed={worn}
    onclick={activate}
  >
    <img
      class="prop-thumb"
      src={api.bagThumbUrl(entry.collection, entry.id, entry.crc, thumbVersion)}
      alt=""
      draggable="false"
      loading="lazy"
      decoding="async"
    />
    <span class="bag-tile-name" aria-hidden="true">{label}</span>
    {#if badges.length}
      <span class="bag-tile-badges" aria-hidden="true">
        {#each badges as badge (badge.label)}
          <span class="bag-badge" title={badge.title}>{badge.label}</span>
        {/each}
      </span>
    {/if}
  </button>
  <button
    class="bag-tile-select"
    class:on={selected}
    type="button"
    aria-label={`${selected ? BAG.tile.deselect : BAG.tile.select} ${label}`}
    aria-pressed={selected}
    onclick={() => onselect(entry)}
  >
    <span aria-hidden="true">✓</span>
  </button>
</div>

<style>
  .bag-tile {
    position: relative;
    width: 56px;
    height: 56px;
  }

  .bag-tile-wear {
    width: 100%;
    height: 100%;
    padding: 0;
    display: grid;
    place-items: center;
    overflow: hidden;
    background-color: var(--bg-void);
    border: 1px solid var(--line);
    border-radius: var(--r-sm);
    cursor: pointer;
    transition: border-color var(--t-fast), box-shadow var(--t-fast), transform var(--t-fast);
  }

  .bag-tile-wear:hover,
  .bag-tile-wear:focus-visible {
    border-color: var(--line-strong);
    transform: translateY(-1px);
  }

  .bag-tile.worn .bag-tile-wear {
    border-color: var(--amber);
    box-shadow: 0 0 0 1px var(--amber), 0 0 10px var(--line-glow);
  }

  .bag-tile.selected .bag-tile-wear {
    border-color: var(--cyan);
    box-shadow: 0 0 0 1px var(--cyan), 0 0 10px var(--cyan-soft);
  }

  .bag-tile.trashed .bag-tile-wear {
    opacity: 0.55;
  }

  .bag-tile-name {
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
    padding: 1px var(--sp-1);
    background: rgba(5, 7, 10, 0.88);
    color: var(--text-0);
    font-size: calc(var(--fs-xs) - 2px);
    line-height: 1.3;
    text-align: center;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    opacity: 0;
    transition: opacity var(--t-fast);
    pointer-events: none;
  }

  .bag-tile:hover .bag-tile-name,
  .bag-tile-wear:focus-visible .bag-tile-name {
    opacity: 1;
  }

  .bag-tile-badges {
    position: absolute;
    top: 1px;
    left: 1px;
    display: flex;
    gap: 1px;
    pointer-events: none;
  }

  .bag-badge {
    min-width: 10px;
    padding: 0 2px;
    background: rgba(5, 7, 10, 0.85);
    border: 1px solid var(--line-strong);
    border-radius: var(--r-xs);
    color: var(--amber);
    font-size: calc(var(--fs-xs) - 3px);
    line-height: 1.4;
    text-align: center;
  }

  .bag-tile-select {
    position: absolute;
    top: 1px;
    right: 1px;
    z-index: 1;
    width: 16px;
    height: 16px;
    display: grid;
    place-items: center;
    padding: 0;
    background: var(--surface-3);
    border: 1px solid var(--line-strong);
    border-radius: 50%;
    color: var(--text-3);
    font-size: calc(var(--fs-xs) - 1px);
    line-height: 1;
    opacity: 0;
    cursor: pointer;
    transition: opacity var(--t-fast), background var(--t-fast), color var(--t-fast);
  }

  .bag-tile:hover .bag-tile-select,
  .bag-tile:focus-within .bag-tile-select,
  .bag-tile-select:focus-visible,
  .bag-tile-select.on {
    opacity: 1;
  }

  .bag-tile-select:hover,
  .bag-tile-select:focus-visible {
    background: var(--surface-4);
    color: var(--text-0);
  }

  .bag-tile-select.on {
    background: var(--cyan-soft);
    border-color: var(--cyan);
    color: var(--cyan);
  }

  .prop-thumb {
    display: block;
    width: 100%;
    height: 100%;
    object-fit: contain;
    image-rendering: pixelated;
  }
</style>
