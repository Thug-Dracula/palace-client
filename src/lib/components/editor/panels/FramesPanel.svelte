<script lang="ts">
  /**
   * Frames panel: the frame strip plus document-level save and export.
   *
   * Frame operations (add/duplicate/delete/reorder/copy/paste) go straight to
   * the Rust document. Save writes the current single-frame prop into My Bag via
   * the crate's writer, with the reference's placement options; a multi-frame
   * document is refused because animated `.prp` encoding is gated. Export writes
   * a PNG of the current frame or an animated WebP of the whole document.
   *
   * The save form itself lives in the editor shell so Ctrl+S can save from any
   * panel; this panel renders it and asks the shell to perform the save.
   */
  import * as api from "../../../api";
  import type { PanelContract } from "./contract";
  import type { SaveForm } from "../save";
  import { causeMessage, downloadBytes, runOutcome } from "./actions";
  import { MESSAGES, PANELS, REFERENCE_TOOLTIPS, TOOLTIPS } from "../strings";
  import "./panel.css";

  let {
    state: doc,
    busy,
    active,
    notify,
    refresh,
    form,
    onsave,
  }: PanelContract & { form: SaveForm; onsave: () => void } = $props();

  let working = $state(false);

  const disabled = $derived(busy || working || !active || !doc);
  const animatedDocument = $derived((doc?.frame_count ?? 1) > 1);

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

  const select = (index: number): Promise<void> =>
    run(() => api.editorSelectFrame(index), MESSAGES.selectedFrame(index + 1));
  const add = (): Promise<void> => run(() => api.editorAddFrame(), MESSAGES.addedFrame);
  const duplicate = (): Promise<void> =>
    run(() => api.editorDuplicateFrame(doc?.frame_index ?? 0), MESSAGES.duplicatedFrame);
  const remove = (): Promise<void> =>
    run(() => api.editorDeleteFrame(doc?.frame_index ?? 0), MESSAGES.deletedFrame);
  const moveUp = (index: number): Promise<void> =>
    run(() => api.editorMoveFrame(index, index - 1), MESSAGES.movedFrameUp);
  const moveDown = (index: number): Promise<void> =>
    run(() => api.editorMoveFrame(index, index + 1), MESSAGES.movedFrameDown);
  const copy = (): Promise<void> =>
    run(() => api.editorCopyFrame(doc?.frame_index ?? 0), MESSAGES.copiedFrame);
  const paste = (): Promise<void> => run(() => api.editorPasteFrame(), MESSAGES.pastedFrame);

  function save(): void {
    if (disabled || animatedDocument) {
      return;
    }
    onsave();
  }

  function exportName(extension: string): string {
    const origin = doc?.origin;
    const base =
      form.name.trim() ||
      (origin?.kind === "bag_prop" && origin.name ? origin.name : "") ||
      "prop";
    return `${base.replace(/[^\w.-]+/g, "_")}.${extension}`;
  }

  async function exportPng(): Promise<void> {
    if (disabled) {
      return;
    }
    working = true;
    try {
      const bytes = await api.editorExportPng();
      downloadBytes(bytes, exportName("png"), "image/png");
      notify(MESSAGES.exportedPng);
    } catch (cause) {
      notify(causeMessage(cause), "error");
    } finally {
      working = false;
    }
  }

  async function exportWebp(): Promise<void> {
    if (disabled) {
      return;
    }
    working = true;
    try {
      const bytes = await api.editorExportWebp(100);
      downloadBytes(bytes, exportName("webp"), "image/webp");
      notify(
        animatedDocument ? MESSAGES.exportedWebpAnimated : MESSAGES.exportedWebpFrame,
      );
    } catch (cause) {
      notify(causeMessage(cause), "error");
    } finally {
      working = false;
    }
  }
</script>

<div class="panel">
  <h3>{PANELS.header.frames}</h3>

  {#if doc && doc.frames.length > 0}
    <ul class="panel-list" aria-label={PANELS.header.frames}>
      {#each doc.frames as frame (frame.index)}
        <li>
          <button
            type="button"
            class:on={frame.selected}
            disabled={disabled}
            aria-pressed={frame.selected}
            title={`Frame ${frame.index + 1}`}
            onclick={() => void select(frame.index)}
          >
            <span>{frame.selected ? "▶" : " "} Frame {frame.index + 1}</span>
            <span class="panel-tag">{frame.width}×{frame.height} · {frame.layer_count}L</span>
          </button>
        </li>
      {/each}
    </ul>
  {:else}
    <p class="panel-note">{PANELS.common.noDocument}</p>
  {/if}

  <div class="panel-buttons">
    <button class="btn" type="button" title={TOOLTIPS.frames.add} disabled={disabled} onclick={() => void add()}>{PANELS.frames.add}</button>
    <button class="btn" type="button" title={TOOLTIPS.frames.duplicate} disabled={disabled} onclick={() => void duplicate()}>{PANELS.frames.duplicate}</button>
    <button class="btn" type="button" title={TOOLTIPS.frames.delete} disabled={disabled || (doc?.frame_count ?? 1) <= 1} onclick={() => void remove()}>
      {PANELS.frames.delete}
    </button>
  </div>
  <div class="panel-buttons">
    <button
      class="btn"
      type="button"
      title={TOOLTIPS.frames.moveUp}
      disabled={disabled || (doc?.frame_index ?? 0) <= 0}
      onclick={() => void moveUp(doc?.frame_index ?? 0)}
    >
      {PANELS.frames.moveUp}
    </button>
    <button
      class="btn"
      type="button"
      title={TOOLTIPS.frames.moveDown}
      disabled={disabled || (doc?.frame_index ?? 0) >= (doc?.frame_count ?? 1) - 1}
      onclick={() => void moveDown(doc?.frame_index ?? 0)}
    >
      {PANELS.frames.moveDown}
    </button>
    <button class="btn" type="button" title={TOOLTIPS.frames.copy} disabled={disabled} onclick={() => void copy()}>{PANELS.frames.copy}</button>
    <button class="btn" type="button" title={TOOLTIPS.frames.paste} disabled={disabled || !doc?.can_paste} onclick={() => void paste()}>
      {PANELS.frames.paste}
    </button>
  </div>

  <div class="panel-section">
    <h4>{PANELS.frames.saveToBag}</h4>
    <div class="panel-field">
      {PANELS.frames.placement}
      <select bind:value={form.placement} disabled={disabled || animatedDocument} aria-label={PANELS.frames.placement}>
        <option value="overwrite_in_place">{PANELS.frames.placements.overwrite}</option>
        <option value="new_prop_at_start">{PANELS.frames.placements.start}</option>
        <option value="new_prop_at_end">{PANELS.frames.placements.end}</option>
      </select>
    </div>
    <div class="panel-grid">
      <label class="panel-field">{PANELS.frames.name}<input type="text" bind:value={form.name} placeholder={PANELS.frames.namePlaceholder} disabled={disabled || animatedDocument} /></label>
      <label class="panel-field">{PANELS.frames.propId}<input type="number" bind:value={form.propIdText} placeholder={PANELS.frames.propIdPlaceholder} disabled={disabled || animatedDocument} /></label>
    </div>
    <div class="panel-row">
      <label class="panel-check"><input type="checkbox" bind:checked={form.head} disabled={disabled || animatedDocument} /> {PANELS.frames.head}</label>
      <label class="panel-check"><input type="checkbox" bind:checked={form.ghost} disabled={disabled || animatedDocument} /> {PANELS.frames.ghost}</label>
      <label class="panel-check"><input type="checkbox" bind:checked={form.rare} disabled={disabled || animatedDocument} /> {PANELS.frames.rare}</label>
      <label class="panel-check"><input type="checkbox" bind:checked={form.bounce} disabled={disabled || animatedDocument} /> {PANELS.frames.bounce}</label>
      <label class="panel-check"><input type="checkbox" bind:checked={form.animate} disabled={disabled || animatedDocument} /> {PANELS.frames.animate}</label>
    </div>
    <div class="panel-grid">
      <label class="panel-field">{PANELS.frames.hOffset}<input type="number" bind:value={form.hOffset} disabled={disabled || animatedDocument} /></label>
      <label class="panel-field">{PANELS.frames.vOffset}<input type="number" bind:value={form.vOffset} disabled={disabled || animatedDocument} /></label>
    </div>
    {#if form.placement === "overwrite_in_place" && doc?.origin.kind !== "bag_prop"}
      <p class="panel-note warn">{PANELS.frames.overwriteWarning}</p>
    {/if}
    {#if animatedDocument}
      <p class="panel-note warn">
        {MESSAGES.animatedSaveBlocked(doc?.frame_count ?? 0)}
      </p>
    {/if}
    <div class="panel-buttons">
      <button class="btn primary" type="button" disabled={disabled || animatedDocument} onclick={save}>
        {PANELS.frames.save}
      </button>
    </div>
  </div>

  <div class="panel-section">
    <h4>{PANELS.frames.export}</h4>
    <div class="panel-buttons">
      <button class="btn" type="button" title={TOOLTIPS.frames.exportPng} disabled={disabled} onclick={() => void exportPng()}>{PANELS.frames.exportPng}</button>
      <button
        class="btn"
        type="button"
        title={REFERENCE_TOOLTIPS.exportWebp}
        disabled={disabled}
        onclick={() => void exportWebp()}
      >
        {PANELS.frames.exportWebp}
      </button>
    </div>
  </div>
</div>
