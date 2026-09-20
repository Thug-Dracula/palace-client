<script lang="ts">
  import type { BagPropEntry } from "../../api";
  import type { BagCollectionView } from "../../bagFilters";
  import { BAG, MESSAGES } from "../editor/strings";

  let {
    entries,
    allWornNow,
    single,
    canRename,
    canTrash,
    canMove,
    moveTargets,
    trashView,
    busy,
    onWear,
    onTakeOff,
    onFavourite,
    onRename,
    onDuplicate,
    onRebuildThumbnail,
    onMove,
    onCopy,
    onTrash,
    onRestore,
    onClear,
  }: {
    entries: BagPropEntry[];
    allWornNow: boolean;
    single: BagPropEntry | null;
    canRename: boolean;
    canTrash: boolean;
    canMove: boolean;
    moveTargets: BagCollectionView[];
    trashView: boolean;
    busy: boolean;
    onWear: () => void;
    onTakeOff: () => void;
    onFavourite: (favourite: boolean) => void;
    onRename: (name: string) => void;
    onDuplicate: () => void;
    onRebuildThumbnail: () => void;
    onMove: (target: string) => void;
    onCopy: () => void;
    onTrash: () => void;
    onRestore: () => void;
    onClear: () => void;
  } = $props();

  let renaming = $state(false);
  let renameDraft = $state("");
  let moving = $state(false);
  let moveTarget = $state("");
  let renameInput: HTMLInputElement | undefined = $state();

  const allFavourite = $derived(entries.length > 0 && entries.every((entry) => entry.fav));

  function startRename() {
    if (!single) {
      return;
    }
    renameDraft = single.name ?? "";
    renaming = true;
    moving = false;
  }

  function commitRename() {
    onRename(renameDraft.trim());
    renaming = false;
  }

  function startMove() {
    moving = true;
    renaming = false;
    moveTarget = moveTargets.find((target) => target.name !== single?.collection)?.name ?? "";
  }

  function commitMove() {
    if (moveTarget !== "") {
      onMove(moveTarget);
    }
    moving = false;
  }

  $effect(() => {
    if (renaming) {
      renameInput?.focus();
      renameInput?.select();
    }
  });
</script>

<div class="bag-action-bar" role="toolbar" aria-label={BAG.actions.toolbar} aria-disabled={busy}>
  <span class="bag-action-count">{MESSAGES.selectedCount(entries.length)}</span>

  {#if renaming}
    <div class="bag-inline">
      <label class="bag-inline-label" for="bag-rename">{BAG.actions.newName}</label>
      <input
        id="bag-rename"
        bind:this={renameInput}
        type="text"
        bind:value={renameDraft}
        onkeydown={(event) => {
          if (event.key === "Enter") {
            event.preventDefault();
            commitRename();
          } else if (event.key === "Escape") {
            event.preventDefault();
            event.stopPropagation();
            renaming = false;
          }
        }}
      />
      <button class="btn primary" type="button" onclick={commitRename}>{BAG.actions.save}</button>
      <button class="btn ghost" type="button" onclick={() => (renaming = false)}>{BAG.actions.cancel}</button>
    </div>
  {:else if moving}
    <div class="bag-inline">
      <label class="bag-inline-label" for="bag-move">{BAG.actions.moveTo}</label>
      <select id="bag-move" bind:value={moveTarget}>
        {#each moveTargets as target (target.name)}
          <option value={target.name}>{target.name}</option>
        {/each}
      </select>
      <button class="btn primary" type="button" disabled={moveTarget === ""} onclick={commitMove}>
        {BAG.actions.move}
      </button>
      <button class="btn ghost" type="button" onclick={() => (moving = false)}>{BAG.actions.cancel}</button>
    </div>
  {:else if trashView}
    <button class="btn" type="button" disabled={busy} onclick={onRestore}>{BAG.actions.restore}</button>
  {:else}
    {#if allWornNow}
      <button class="btn" type="button" disabled={busy} onclick={onTakeOff}>{BAG.actions.takeOff}</button>
    {:else}
      <button class="btn primary" type="button" disabled={busy} onclick={onWear}>{BAG.actions.wear}</button>
    {/if}
    <button
      class="btn"
      type="button"
      disabled={busy}
      aria-pressed={allFavourite}
      onclick={() => onFavourite(!allFavourite)}
    >
      {allFavourite ? BAG.actions.unfavourite : BAG.actions.favourite}
    </button>
    <button class="btn" type="button" disabled={busy || !canRename} onclick={startRename}>{BAG.actions.rename}</button>
    <button class="btn" type="button" disabled={busy} onclick={onDuplicate}>{BAG.actions.duplicate}</button>
    <button
      class="btn"
      type="button"
      disabled={busy}
      title={BAG.actions.rebuildThumbnailHint}
      onclick={onRebuildThumbnail}
    >
      {BAG.actions.rebuildThumbnail}
    </button>
    <button
      class="btn"
      type="button"
      disabled={busy || !canMove || moveTargets.length < 2}
      title={moveTargets.length < 2 ? BAG.actions.moveHintDisabled : BAG.actions.moveHint}
      onclick={startMove}
    >
      {BAG.actions.move}
    </button>
    <button
      class="btn danger"
      type="button"
      disabled={busy || !canTrash}
      title={canTrash ? BAG.actions.trashHint : BAG.actions.trashHintDisabled}
      onclick={onTrash}
    >
      {BAG.actions.delete}
    </button>
  {/if}

  <span class="bag-action-spacer"></span>
  <button class="btn ghost" type="button" disabled={busy} onclick={onCopy} aria-label={BAG.actions.copyId}>
    {BAG.actions.copyId}
  </button>
  <button class="btn ghost" type="button" onclick={onClear} aria-label={BAG.actions.clear}>{BAG.actions.clear}</button>
</div>

<style>
  .bag-action-bar {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: var(--sp-1);
    padding: var(--sp-2);
    background: var(--surface-3);
    border: 1px solid var(--line-strong);
    border-radius: var(--r-sm);
  }

  .bag-action-count {
    font-size: var(--fs-xs);
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--cyan);
    font-variant-numeric: tabular-nums;
  }

  .bag-action-spacer {
    flex: 1;
  }

  .bag-inline {
    display: flex;
    align-items: center;
    gap: var(--sp-1);
    flex-wrap: wrap;
  }

  .bag-inline-label {
    font-size: var(--fs-xs);
    color: var(--text-2);
    text-transform: uppercase;
    letter-spacing: 0.08em;
  }

  .bag-inline input,
  .bag-inline select {
    background: var(--bg-void);
    border: 1px solid var(--line-strong);
    border-radius: var(--r-sm);
    padding: 3px var(--sp-2);
    color: var(--text-0);
    font-size: var(--fs-sm);
  }

  .bag-inline input:focus,
  .bag-inline select:focus {
    border-color: var(--amber-dim);
    box-shadow: 0 0 0 2px var(--amber-soft);
  }

  .btn.danger:hover {
    color: var(--red);
    border-color: var(--red);
  }
</style>
