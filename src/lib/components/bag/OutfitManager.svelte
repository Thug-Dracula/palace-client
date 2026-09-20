<script lang="ts">
  import type { BagOutfit } from "../../api";
  import { BAG, MESSAGES } from "../editor/strings";

  let {
    outfits,
    wornCount,
    busy,
    onSave,
    onApply,
    onRename,
    onDelete,
  }: {
    outfits: BagOutfit[];
    wornCount: number;
    busy: boolean;
    onSave: (name: string) => void;
    onApply: (name: string) => void;
    onRename: (from: string, to: string) => void;
    onDelete: (name: string) => void;
  } = $props();

  let newName = $state("");
  let editing = $state<string | null>(null);
  let renameDraft = $state("");
  let confirming = $state<string | null>(null);

  function save() {
    const name = newName.trim();
    if (name === "") {
      return;
    }
    onSave(name);
    newName = "";
  }

  function startRename(outfit: BagOutfit) {
    editing = outfit.name;
    renameDraft = outfit.name;
    confirming = null;
  }

  function commitRename() {
    const next = renameDraft.trim();
    if (editing !== null && next !== "" && next !== editing) {
      onRename(editing, next);
    }
    editing = null;
  }

  function remove(name: string) {
    if (confirming === name) {
      onDelete(name);
      confirming = null;
      return;
    }
    confirming = name;
  }
</script>

<div class="outfit-manager">
  <form
    class="outfit-save"
    onsubmit={(event) => {
      event.preventDefault();
      save();
    }}
  >
    <label class="outfit-label" for="outfit-name">{BAG.outfits.saveCurrent}</label>
    <div class="outfit-save-row">
      <input
        id="outfit-name"
        type="text"
        placeholder={BAG.outfits.nameLabel}
        aria-label={BAG.outfits.nameAria}
        bind:value={newName}
      />
      <button class="btn" type="submit" disabled={busy || wornCount === 0 || newName.trim() === ""}>
        {BAG.outfits.save}
      </button>
    </div>
    {#if wornCount === 0}
      <p class="outfit-hint">{BAG.outfits.wearHint}</p>
    {/if}
  </form>

  {#if outfits.length === 0}
    <p class="outfit-hint">{BAG.outfits.none}</p>
  {:else}
    <ul class="outfit-list" aria-label={BAG.sidebar.outfits}>
      {#each outfits as outfit (outfit.name)}
        <li class="outfit-row">
          {#if editing === outfit.name}
            <div class="outfit-rename">
              <input
                type="text"
                aria-label={`Rename ${outfit.name}`}
                bind:value={renameDraft}
                onkeydown={(event) => {
                  if (event.key === "Enter") {
                    event.preventDefault();
                    commitRename();
                  } else if (event.key === "Escape") {
                    event.preventDefault();
                    event.stopPropagation();
                    editing = null;
                  }
                }}
              />
              <button class="btn" type="button" onclick={commitRename} aria-label="Confirm rename">
                {BAG.outfits.ok}
              </button>
              <button
                class="btn ghost"
                type="button"
                onclick={() => (editing = null)}
                aria-label={BAG.outfits.cancel}
              >
                ✕
              </button>
            </div>
          {:else}
            <div class="outfit-meta">
              <span class="outfit-name">{outfit.name}</span>
              <span class="outfit-count">{MESSAGES.propCount(outfit.props.length)}</span>
            </div>
            <div class="outfit-actions">
              <button
                class="btn"
                type="button"
                disabled={busy || wornCount === 0}
                onclick={() => onApply(outfit.name)}
                aria-label={`Apply outfit ${outfit.name}`}
              >
                {BAG.outfits.apply}
              </button>
              <button
                class="btn ghost"
                type="button"
                disabled={busy}
                onclick={() => startRename(outfit)}
                aria-label={`Rename outfit ${outfit.name}`}
              >
                {BAG.outfits.rename}
              </button>
              <button
                class="btn ghost danger"
                class:confirming={confirming === outfit.name}
                type="button"
                disabled={busy}
                onclick={() => remove(outfit.name)}
                aria-label={confirming === outfit.name ? `Confirm delete ${outfit.name}` : `Delete outfit ${outfit.name}`}
              >
                {confirming === outfit.name ? BAG.outfits.confirm : BAG.outfits.delete}
              </button>
            </div>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .outfit-manager {
    display: flex;
    flex-direction: column;
    gap: var(--sp-3);
  }

  .outfit-save {
    display: flex;
    flex-direction: column;
    gap: var(--sp-1);
  }

  .outfit-label {
    font-size: var(--fs-xs);
    letter-spacing: 0.12em;
    text-transform: uppercase;
    color: var(--text-2);
  }

  .outfit-save-row {
    display: flex;
    gap: var(--sp-1);
  }

  .outfit-save-row input,
  .outfit-rename input {
    min-width: 0;
    flex: 1;
    background: var(--bg-void);
    border: 1px solid var(--line-strong);
    border-radius: var(--r-sm);
    padding: var(--sp-1) var(--sp-2);
    color: var(--text-0);
    font-size: var(--fs-sm);
  }

  .outfit-save-row input:focus,
  .outfit-rename input:focus {
    border-color: var(--amber-dim);
    box-shadow: 0 0 0 2px var(--amber-soft);
  }

  .outfit-hint {
    margin: 0;
    font-size: var(--fs-xs);
    color: var(--text-3);
  }

  .outfit-list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: var(--sp-2);
  }

  .outfit-row {
    display: flex;
    flex-direction: column;
    gap: var(--sp-1);
    padding: var(--sp-2);
    background: var(--bg-void);
    border: 1px solid var(--line);
    border-radius: var(--r-sm);
  }

  .outfit-meta {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: var(--sp-2);
  }

  .outfit-name {
    font-size: var(--fs-sm);
    color: var(--text-0);
    overflow-wrap: anywhere;
  }

  .outfit-count {
    flex: none;
    font-size: var(--fs-xs);
    color: var(--text-3);
    font-variant-numeric: tabular-nums;
  }

  .outfit-actions,
  .outfit-rename {
    display: flex;
    gap: var(--sp-1);
  }

  .outfit-actions .btn {
    padding: 3px var(--sp-2);
    font-size: var(--fs-xs);
  }

  .btn.ghost.danger:hover {
    color: var(--red);
  }

  .btn.ghost.confirming {
    color: var(--red);
    border-color: var(--red);
  }
</style>
