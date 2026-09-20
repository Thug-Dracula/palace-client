<script lang="ts">
  /**
   * Layer list: base + overlays + text layers for the selected frame.
   *
   * This is the R3 addition beyond PalaceChat: every layer shows a visibility
   * toggle, selection, reorder and delete, and the selected layer exposes its
   * position, rotation, scale and opacity. The list maps rows top-first while
   * the Rust model stores base-first, so "up" moves a layer toward the top.
   * Delete and reorder refuse the base layer (index 0), matching the model.
   */
  import * as api from "../../api";
  import type { PanelContract } from "./panels/contract";
  import { causeMessage, runOutcome } from "./panels/actions";
  import { MESSAGES, PANELS } from "./strings";
  import "./panels/panel.css";

  let { state: doc, busy, active, notify, refresh }: PanelContract = $props();

  let selected = $state(-1);
  let working = $state(false);

  const disabled = $derived(busy || working || !active || !doc);
  const layers = $derived(doc?.layers ?? []);
  const last = $derived(layers.length - 1);
  const current = $derived(selected >= 0 && selected < layers.length ? layers[selected] : null);

  $effect(() => {
    if (layers.length === 0) {
      selected = -1;
      return;
    }
    if (selected < 0 || selected >= layers.length) {
      selected = layers.length - 1;
    }
  });

  async function run(action: () => Promise<api.EditorOutcome>, success: string): Promise<void> {
    if (disabled) {
      return;
    }
    working = true;
    try {
      await runOutcome(action, notify, refresh, success);
    } finally {
      working = false;
    }
  }

  function update(patch: Partial<api.EditorLayerUpdate>, success: string): Promise<void> {
    return run(() => api.editorUpdateLayer({ layer: selected, ...patch }), success);
  }

  function toggle(index: number, visible: boolean): Promise<void> {
    return run(
      () => api.editorUpdateLayer({ layer: index, visible }),
      visible ? MESSAGES.layerShown : MESSAGES.layerHidden,
    );
  }

  function move(index: number, delta: number): Promise<void> {
    return run(
      () => api.editorMoveLayer(index, index + delta),
      delta > 0 ? MESSAGES.layerMovedUp : MESSAGES.layerMovedDown,
    );
  }

  function remove(index: number): Promise<void> {
    if (index < 1) {
      notify(MESSAGES.baseLayerProtected, "warn");
      return Promise.resolve();
    }
    return run(() => api.editorDeleteLayer(index), MESSAGES.layerDeleted);
  }

  function selectLayer(index: number): void {
    selected = index;
  }

  function onNumber(event: Event, field: "x" | "y" | "rotation" | "scale"): void {
    const value = Number((event.currentTarget as HTMLInputElement).value);
    if (Number.isFinite(value)) {
      void update({ [field]: value }, MESSAGES.layerUpdated);
    }
  }

  function onOpacity(event: Event): void {
    const value = Number((event.currentTarget as HTMLInputElement).value);
    if (Number.isFinite(value)) {
      void update({ opacity: value }, MESSAGES.layerOpacity);
    }
  }
</script>

<div class="panel">
  <h3>{PANELS.header.layers}</h3>

  {#if layers.length === 0}
    <p class="panel-note">{PANELS.common.noDocument}</p>
  {:else}
    <ul class="layer-list" aria-label={PANELS.header.layers}>
      {#each layers as layer, index (layer.id)}
        <li class="layer-row" class:on={index === selected}>
          <input
            class="layer-eye"
            type="checkbox"
            checked={layer.visible}
            disabled={disabled}
            aria-label={`${PANELS.layers.visibility} ${layer.name}`}
            onchange={(event) => void toggle(index, (event.currentTarget as HTMLInputElement).checked)}
          />
          <button
            class="layer-name"
            type="button"
            disabled={disabled}
            aria-pressed={index === selected}
            onclick={() => selectLayer(index)}
          >
            <span class="layer-title">{layer.name}</span>
            <span class="panel-tag">{layer.width}×{layer.height}</span>
          </button>
          <div class="layer-buttons">
            <button
              class="btn tiny"
              type="button"
              title={PANELS.layers.moveUp}
              aria-label={`Move ${layer.name} up`}
              disabled={disabled || index >= last}
              onclick={() => void move(index, 1)}
            >
              ▲
            </button>
            <button
              class="btn tiny"
              type="button"
              title={PANELS.layers.moveDown}
              aria-label={`Move ${layer.name} down`}
              disabled={disabled || index <= 1}
              onclick={() => void move(index, -1)}
            >
              ▼
            </button>
            <button
              class="btn tiny"
              type="button"
              title={PANELS.layers.delete}
              aria-label={`Delete ${layer.name}`}
              disabled={disabled || index === 0}
              onclick={() => void remove(index)}
            >
              ×
            </button>
          </div>
        </li>
      {/each}
    </ul>

    {#if current}
      <div class="panel-section">
        <h4>{current.name}</h4>
        <div class="panel-field">
          {PANELS.layers.opacity}
          <input
            type="range"
            min="0"
            max="1"
            step="0.05"
            value={current.opacity}
            disabled={disabled}
            aria-label={`${PANELS.layers.opacity}: ${current.name}`}
            onchange={onOpacity}
          />
        </div>
        <div class="panel-grid">
          <label class="panel-field">
            {PANELS.layers.x}
            <input
              type="number"
              step="1"
              value={current.x}
              disabled={disabled}
              onchange={(event) => onNumber(event, "x")}
            />
          </label>
          <label class="panel-field">
            {PANELS.layers.y}
            <input
              type="number"
              step="1"
              value={current.y}
              disabled={disabled}
              onchange={(event) => onNumber(event, "y")}
            />
          </label>
          <label class="panel-field">
            {PANELS.layers.rotation}
            <input
              type="number"
              step="1"
              value={current.rotation}
              disabled={disabled}
              onchange={(event) => onNumber(event, "rotation")}
            />
          </label>
          <label class="panel-field">
            {PANELS.layers.scale}
            <input
              type="number"
              step="0.1"
              min="0"
              value={current.scale}
              disabled={disabled}
              onchange={(event) => onNumber(event, "scale")}
            />
          </label>
        </div>
        <p class="panel-note">{PANELS.layers.positionNote}</p>
      </div>
    {/if}
  {/if}
</div>

<style>
  .layer-list {
    display: flex;
    flex-direction: column;
    gap: 2px;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .layer-row {
    display: flex;
    align-items: center;
    gap: var(--sp-1);
    padding: 2px var(--sp-1);
    border: 1px solid transparent;
    border-radius: var(--r-sm);
  }

  .layer-row.on {
    background: var(--amber-soft);
    border-color: var(--amber-dim);
  }

  .layer-eye {
    flex: none;
    accent-color: var(--amber);
  }

  .layer-name {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--sp-2);
    min-width: 0;
    padding: var(--sp-1);
    background: none;
    border: 0;
    border-radius: var(--r-sm);
    color: var(--text-1);
    font-size: var(--fs-xs);
    text-align: left;
  }

  .layer-title {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .layer-buttons {
    display: flex;
    flex: none;
    gap: 1px;
  }

  .btn.tiny {
    min-width: 22px;
    padding: 2px var(--sp-1);
    font-size: var(--fs-xs);
    line-height: 1;
  }
</style>
