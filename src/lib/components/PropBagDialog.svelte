<script lang="ts">
  import { onMount, tick } from "svelte";
  import * as api from "../api";
  import type { BagPropEntry } from "../api";
  import { MAX_PROPS, store } from "../store.svelte";
  import {
    BAG_PAGE_SIZE,
    allWorn,
    bagEntryKey,
    buildCollectionView,
    defaultBagFilterState,
    filterBagEntries,
    nextLimit,
    toggleSelection,
    visibleWindow,
    type BagFilterState,
    type BagSort,
  } from "../bagFilters";
  import BagSidebar from "./bag/BagSidebar.svelte";
  import BagTile from "./bag/BagTile.svelte";
  import PropActionBar from "./bag/PropActionBar.svelte";
  import { focusFirst, trapTabKey } from "./editor/a11y";
  import { BAG, MESSAGES } from "./editor/strings";
  import "./editor/ui.css";

  let { onclose }: { onclose: () => void } = $props();

  let loading = $state(!store.bagLoaded);
  let busy = $state(false);
  let error = $state<string | null>(null);
  let notice = $state<string | null>(null);
  let filters = $state<BagFilterState>(defaultBagFilterState());
  let selected = $state<Set<string>>(new Set());
  let limit = $state(BAG_PAGE_SIZE);
  let showIndicators = $state(true);
  let thumbVersion = $state(0);
  let sidebarTab = $state<"collections" | "outfits">("collections");
  let confirmPurge = $state(false);
  let dialog: HTMLDivElement | undefined = $state();

  const wornIds = $derived(store.self?.props ?? []);
  const catalog = $derived(store.bagCatalog);
  const visible = $derived(filterBagEntries(catalog, filters));
  const windowed = $derived(visibleWindow(visible, limit));
  const selectedEntries = $derived(catalog.filter((entry) => selected.has(bagEntryKey(entry))));
  const collectionsView = $derived(
    buildCollectionView(store.bagCollections, store.bagShelves, catalog),
  );
  const totalCount = $derived(catalog.filter((entry) => entry.source === "bag" && !entry.trash).length);
  const trashCount = $derived(catalog.filter((entry) => entry.trash).length);
  const moveTargets = $derived(collectionsView.filter((collection) => collection.writable));
  const writableNames = $derived(
    new Set(store.bagCollections.filter((collection) => collection.writable).map((collection) => collection.name)),
  );
  const pathByName = $derived(
    new Map(store.bagCollections.map((collection) => [collection.name, collection.path])),
  );
  const single = $derived(selectedEntries.length === 1 ? selectedEntries[0] : null);
  const canRename = $derived(single !== null && writableNames.has(single.collection));
  const canTrash = $derived(
    selectedEntries.length > 0 &&
      selectedEntries.every((entry) => !entry.trash && writableNames.has(entry.collection)),
  );
  const canMove = $derived(single !== null && writableNames.has(single.collection));
  const allWornNow = $derived(allWorn(selectedEntries, wornIds));
  const largeCollection = $derived(visible.length > BAG_PAGE_SIZE);

  function message(cause: unknown): string {
    return cause instanceof Error ? cause.message : String(cause);
  }

  async function refresh(): Promise<void> {
    await store.loadBag();
    if (store.bagError) {
      error = store.bagError;
    }
    const keys = new Set(store.bagCatalog.map(bagEntryKey));
    const pruned = new Set([...selected].filter((key) => keys.has(key)));
    if (pruned.size !== selected.size) {
      selected = pruned;
    }
  }

  async function guard(action: () => Promise<void> | void): Promise<void> {
    busy = true;
    error = null;
    notice = null;
    try {
      await action();
    } catch (cause) {
      error = message(cause);
    } finally {
      busy = false;
    }
  }

  async function load(): Promise<void> {
    loading = !store.bagLoaded;
    error = null;
    try {
      await store.loadBag();
      if (store.bagError) {
        error = store.bagError;
      }
    } finally {
      loading = false;
    }
  }

  function patchFilters(patch: Partial<BagFilterState>): void {
    filters = { ...filters, ...patch };
    limit = BAG_PAGE_SIZE;
    confirmPurge = false;
  }

  function onWear(entry: BagPropEntry): void {
    const result = store.toggleBagProp(entry);
    error = result.ok ? null : (result.error ?? MESSAGES.couldNotWear);
  }

  function toggleSelected(entry: BagPropEntry): void {
    selected = toggleSelection(selected, bagEntryKey(entry));
  }

  async function wearSelection(): Promise<void> {
    await guard(() => {
      const result = store.wearBagEntries(selectedEntries.filter((entry) => !entry.trash));
      if (!result.ok) {
        throw new Error(result.error ?? MESSAGES.thosePropsNotWorn);
      }
    });
  }

  async function takeOffSelection(): Promise<void> {
    await guard(() => {
      for (const entry of selectedEntries) {
        if (wornIds.includes(entry.id)) {
          store.toggleBagProp(entry);
        }
      }
    });
  }

  async function favouriteSelection(favourite: boolean): Promise<void> {
    await guard(async () => {
      for (const entry of selectedEntries) {
        await api.bagFavourite(entry.id, entry.crc, favourite);
      }
      await refresh();
    });
  }

  async function renameSelection(name: string): Promise<void> {
    if (!single) {
      return;
    }
    await guard(async () => {
      await api.bagRename(single.id, single.crc, name === "" ? null : name);
      await refresh();
    });
  }

  async function duplicateSelection(): Promise<void> {
    await guard(async () => {
      let copied = 0;
      for (const entry of selectedEntries) {
        const from = pathByName.get(entry.collection);
        if (!from) {
          throw new Error(MESSAGES.noBagPath(entry.collection));
        }
        const outcome = await api.bagDuplicate(from, entry.id, entry.crc, null);
        if (outcome === "added" || outcome === "duplicated") {
          copied += 1;
        }
      }
      await refresh();
      notice = copied > 0 ? MESSAGES.copiedIntoBag(copied) : MESSAGES.nothingNewToCopy;
    });
  }

  async function rebuildSelectedThumbnails(): Promise<void> {
    await guard(async () => {
      let rebuilt = 0;
      for (const entry of selectedEntries) {
        const result = await api.bagRebuildThumbnail(entry.id, entry.crc);
        if (result.rebuilt) {
          rebuilt += 1;
        }
      }
      thumbVersion += 1;
      notice = MESSAGES.rebuiltThumbnails(rebuilt);
    });
  }

  async function rebuildAllThumbnails(): Promise<void> {
    await guard(async () => {
      const result = await api.bagRebuildThumbnails();
      thumbVersion += 1;
      notice = MESSAGES.rebuiltThumbCache(result.removed);
    });
  }

  async function moveSelection(target: string): Promise<void> {
    if (!single) {
      return;
    }
    await guard(async () => {
      const from = pathByName.get(single.collection);
      const to = pathByName.get(target);
      if (!from || !to) {
        throw new Error(MESSAGES.moveNeedsPaths);
      }
      await api.bagMove(from, to, single.id, single.crc);
      await refresh();
    });
  }

  async function copySelectionId(): Promise<void> {
    if (!single) {
      return;
    }
    try {
      await navigator.clipboard.writeText(String(single.id));
      error = null;
      notice = MESSAGES.copiedPropId(single.id);
    } catch {
      error = MESSAGES.clipboardUnavailable(single.id);
    }
  }

  async function trashSelection(): Promise<void> {
    await guard(async () => {
      for (const entry of selectedEntries) {
        await api.bagTrash(entry.id, entry.crc);
      }
      selected = new Set();
      await refresh();
    });
  }

  async function restoreSelection(): Promise<void> {
    await guard(async () => {
      for (const entry of selectedEntries) {
        await api.bagTrashRestore(entry.id, entry.crc);
      }
      selected = new Set();
      await refresh();
    });
  }

  async function purgeTrash(): Promise<void> {
    if (!confirmPurge) {
      confirmPurge = true;
      return;
    }
    await guard(async () => {
      const result = await api.bagTrashPurge();
      confirmPurge = false;
      await refresh();
      notice = MESSAGES.purgedTrash(result.purged);
    });
  }

  async function saveOutfit(name: string): Promise<void> {
    await guard(async () => {
      await store.saveOutfit(name);
      notice = MESSAGES.outfitSaved(name);
    });
  }

  async function applyOutfit(name: string): Promise<void> {
    await guard(async () => {
      const result = await store.applyOutfit(name);
      if (!result.ok) {
        throw new Error(result.error ?? MESSAGES.outfitFailed);
      }
      notice = MESSAGES.outfitApplied(name);
    });
  }

  async function renameOutfit(from: string, to: string): Promise<void> {
    await guard(() => store.renameOutfit(from, to));
  }

  async function deleteOutfit(name: string): Promise<void> {
    await guard(() => store.deleteOutfit(name));
  }

  function onGridScroll(event: Event): void {
    const grid = event.currentTarget as HTMLElement;
    if (grid.scrollTop + grid.clientHeight >= grid.scrollHeight - 240) {
      limit = nextLimit(limit, visible.length);
    }
  }

  function onKeydown(event: KeyboardEvent): void {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      onclose();
      return;
    }
    if (trapTabKey(event, dialog)) {
      event.stopPropagation();
    }
  }

  onMount(() => {
    void (async () => {
      await load();
      await tick();
      const target =
        dialog?.querySelector<HTMLButtonElement>(".bag-tile-wear") ??
        dialog?.querySelector<HTMLButtonElement>(".dialog-x");
      if (target) {
        target.focus();
      } else {
        focusFirst(dialog);
      }
    })();
  });
</script>

<div class="dialog-backdrop">
  <div
    class="dialog prop-dialog bag-dialog"
    role="dialog"
    aria-modal="true"
    aria-label={BAG.dialogLabel}
    tabindex="-1"
    bind:this={dialog}
    onkeydown={onKeydown}
    oncontextmenu={(event) => event.preventDefault()}
  >
    <div class="dialog-head">
      <span>{BAG.title}</span>
      <button class="dialog-x" type="button" aria-label={BAG.closeLabel} onclick={onclose}>×</button>
    </div>

    <div class="dialog-body bag-body">
      {#if loading}
        <p class="dialog-hint ui-loading" role="status" aria-live="polite">{BAG.loading}</p>
      {:else if error && catalog.length === 0}
        <div class="ui-alert error">
          <span class="ui-alert-text">{BAG.loadFailed}</span>
          <button class="btn" type="button" onclick={() => void load()}>{BAG.retry}</button>
        </div>
        {#if error}<p class="dialog-error">{error}</p>{/if}
      {:else}
        <div class="bag-layout">
          <BagSidebar
            collections={collectionsView}
            activeCollection={filters.collection}
            {totalCount}
            tab={sidebarTab}
            outfits={store.outfits}
            wornCount={wornIds.length}
            {busy}
            onSelectCollection={(name) => patchFilters({ collection: name })}
            onTab={(next) => (sidebarTab = next)}
            onSaveOutfit={(name) => void saveOutfit(name)}
            onApplyOutfit={(name) => void applyOutfit(name)}
            onRenameOutfit={(from, to) => void renameOutfit(from, to)}
            onDeleteOutfit={(name) => void deleteOutfit(name)}
          />

          <section class="bag-main" aria-label={BAG.browserLabel}>
            <div class="bag-toolbar">
              <div class="seg bag-filters" role="group" aria-label={BAG.filters.group}>
                <button
                  class="btn"
                  class:on={filters.filter === "all"}
                  type="button"
                  aria-pressed={filters.filter === "all"}
                  onclick={() => patchFilters({ filter: "all" })}
                >
                  {BAG.filters.all}
                </button>
                <button
                  class="btn"
                  class:on={filters.filter === "fav"}
                  type="button"
                  aria-pressed={filters.filter === "fav"}
                  onclick={() => patchFilters({ filter: "fav" })}
                >
                  {BAG.filters.favourites}
                </button>
                <button
                  class="btn"
                  class:on={filters.filter === "trash"}
                  type="button"
                  aria-pressed={filters.filter === "trash"}
                  onclick={() => patchFilters({ filter: "trash" })}
                >
                  {BAG.filters.trash} {trashCount > 0 ? `(${trashCount})` : ""}
                </button>
              </div>

              <input
                class="bag-search"
                type="search"
                placeholder={BAG.filters.searchPlaceholder}
                aria-label={BAG.filters.searchLabel}
                value={filters.query}
                oninput={(event) =>
                  patchFilters({ query: (event.currentTarget as HTMLInputElement).value })}
              />

              <label class="bag-control">
                <span>{BAG.filters.sort}</span>
                <select
                  aria-label={BAG.filters.sortLabel}
                  value={filters.sort}
                  onchange={(event) =>
                    patchFilters({ sort: (event.currentTarget as HTMLSelectElement).value as BagSort })}
                >
                  <option value="name">{BAG.filters.sortName}</option>
                  <option value="id">{BAG.filters.sortId}</option>
                  <option value="collection">{BAG.filters.sortCollection}</option>
                </select>
              </label>

              <label class="bag-control bag-check">
                <input
                  type="checkbox"
                  checked={showIndicators}
                  onchange={(event) =>
                    (showIndicators = (event.currentTarget as HTMLInputElement).checked)}
                />
                <span>{BAG.filters.indicators}</span>
              </label>

              <button
                class="btn"
                type="button"
                disabled={busy}
                title={BAG.actions.rebuildThumbnailsHint}
                onclick={() => void rebuildAllThumbnails()}
              >
                {BAG.actions.rebuildThumbnails}
              </button>

              <span class="bag-result-count" role="status" aria-live="polite" aria-label={BAG.filters.resultCount}>{visible.length}</span>
            </div>

            {#if filters.filter === "trash" && trashCount > 0}
              <div class="bag-trash-bar">
                <span>{MESSAGES.trashHolds(trashCount)}</span>
                <button class="btn danger" type="button" disabled={busy} onclick={() => void purgeTrash()}>
                  {confirmPurge ? MESSAGES.confirmPurge(trashCount) : BAG.trash.empty}
                </button>
                {#if confirmPurge}
                  <button class="btn ghost" type="button" onclick={() => (confirmPurge = false)}>
                    {BAG.trash.cancel}
                  </button>
                {/if}
              </div>
            {/if}

            {#if selectedEntries.length > 0}
              <PropActionBar
                entries={selectedEntries}
                {allWornNow}
                {single}
                {canRename}
                {canTrash}
                {canMove}
                {moveTargets}
                trashView={filters.filter === "trash"}
                {busy}
                onWear={() => void wearSelection()}
                onTakeOff={() => void takeOffSelection()}
                onFavourite={(favourite) => void favouriteSelection(favourite)}
                onRename={(name) => void renameSelection(name)}
                onDuplicate={() => void duplicateSelection()}
                onRebuildThumbnail={() => void rebuildSelectedThumbnails()}
                onMove={(target) => void moveSelection(target)}
                onCopy={() => void copySelectionId()}
                onTrash={() => void trashSelection()}
                onRestore={() => void restoreSelection()}
                onClear={() => (selected = new Set())}
              />
            {/if}

            {#if largeCollection}
              <p class="bag-page-note" role="status" aria-live="polite">
                {MESSAGES.largeCollection(visible.length)}
              </p>
            {/if}

            {#if visible.length === 0}
              <p class="dialog-hint">
                {totalCount === 0 && trashCount === 0 ? BAG.empty.none : BAG.empty.noMatch}
              </p>
            {:else}
              <div class="bag-grid" role="group" aria-label={BAG.filters.gridLabel} onscroll={onGridScroll}>
                {#each windowed as entry (bagEntryKey(entry))}
                  <BagTile
                    {entry}
                    {thumbVersion}
                    worn={wornIds.includes(entry.id)}
                    selected={selected.has(bagEntryKey(entry))}
                    {showIndicators}
                    onwear={onWear}
                    onselect={toggleSelected}
                  />
                {/each}
              </div>
              {#if windowed.length < visible.length}
                <button
                  class="btn bag-more"
                  type="button"
                  onclick={() => (limit = nextLimit(limit, visible.length))}
                >
                  {MESSAGES.showMore(windowed.length, visible.length)}
                </button>
              {/if}
            {/if}
          </section>
        </div>
      {/if}

      {#if notice}<p class="dialog-hint bag-notice" role="status">{notice}</p>{/if}
      {#if error && catalog.length > 0}<p class="dialog-error" role="alert">{error}</p>{/if}

      <div class="dialog-actions">
        <span class="prop-count">{MESSAGES.wornCount(wornIds.length, MAX_PROPS)}</span>
        <button
          class="btn"
          type="button"
          onclick={() => store.takeOffAvatar()}
          disabled={busy || wornIds.length === 0}
        >
          {BAG.actions.takeAllOff}
        </button>
        <button class="btn primary" type="button" onclick={onclose} disabled={busy}>{BAG.actions.close}</button>
      </div>
    </div>
  </div>
</div>

<style>
  .bag-dialog {
    width: min(940px, calc(100vw - 32px));
  }

  .bag-body {
    display: flex;
    flex-direction: column;
    min-height: 0;
  }

  .bag-layout {
    display: grid;
    grid-template-columns: minmax(190px, 230px) minmax(0, 1fr);
    gap: var(--sp-3);
    height: min(60vh, 520px);
    min-height: 0;
  }

  .bag-main {
    display: flex;
    flex-direction: column;
    gap: var(--sp-2);
    min-width: 0;
    min-height: 0;
  }

  .bag-toolbar {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: var(--sp-2);
    flex: none;
  }

  .bag-search {
    flex: 1;
    min-width: 140px;
    background: var(--bg-void);
    border: 1px solid var(--line-strong);
    border-radius: var(--r-sm);
    padding: var(--sp-1) var(--sp-2);
    color: var(--text-0);
    font-size: var(--fs-sm);
  }

  .bag-search:focus {
    border-color: var(--amber-dim);
    box-shadow: 0 0 0 2px var(--amber-soft);
  }

  .bag-control {
    display: inline-flex;
    align-items: center;
    gap: var(--sp-1);
    font-size: var(--fs-xs);
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--text-2);
  }

  .bag-control select {
    background: var(--bg-void);
    border: 1px solid var(--line-strong);
    border-radius: var(--r-sm);
    padding: var(--sp-1);
    color: var(--text-0);
    font-size: var(--fs-sm);
  }

  .bag-check {
    cursor: pointer;
  }

  .bag-check input {
    accent-color: var(--amber);
  }

  .bag-result-count {
    margin-left: auto;
    font-size: var(--fs-xs);
    color: var(--text-3);
    font-variant-numeric: tabular-nums;
  }

  .bag-trash-bar {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    flex: none;
    padding: var(--sp-2);
    background: var(--surface-1);
    border: 1px solid var(--line);
    border-radius: var(--r-sm);
    font-size: var(--fs-xs);
    color: var(--text-1);
  }

  .bag-page-note {
    margin: 0;
    flex: none;
    font-size: var(--fs-xs);
    color: var(--text-3);
  }

  .bag-grid {
    flex: 1;
    display: grid;
    grid-template-columns: repeat(auto-fill, 56px);
    grid-auto-rows: 56px;
    gap: var(--sp-1);
    align-content: start;
    justify-content: start;
    padding: var(--sp-2);
    overflow: auto;
    background: var(--bg-void);
    border: 1px solid var(--line);
    border-radius: var(--r-sm);
  }

  .bag-more {
    flex: none;
    align-self: flex-start;
  }

  .bag-notice {
    margin: var(--sp-2) 0 0;
    color: var(--cyan);
  }

  .bag-dialog .dialog-error {
    max-width: none;
  }

  .btn.danger:hover {
    color: var(--red);
    border-color: var(--red);
  }
</style>
