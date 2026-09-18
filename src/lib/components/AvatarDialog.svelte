<script lang="ts">
  import { onMount } from "svelte";
  import * as api from "../api";
  import { store } from "../store.svelte";

  let { onclose }: { onclose: () => void } = $props();

  // Geometry comes from the renderer over palace://faces.json, so the picker
  // cannot disagree with the sheet it is cropping.
  let faces = $state(13);
  let colors = $state(16);
  let cell = $state(44);
  let faceRows = $state<number[][]>([[0, 1, 2], [3, 4, 5, 6, 7], [8, 9, 10, 11, 12]]);

  const self = store.self;
  const sheet = api.facesUrl();

  let face = $state(self?.face ?? 0);
  let color = $state(self?.color ?? 0);
  let busy = $state(false);
  let error = $state<string | null>(null);
  let dialog: HTMLDivElement | undefined = $state();

  function sprite(f: number, c: number, size = cell): string {
    const scale = size / cell;
    return `background-image:url('${sheet}'); background-size:${(faces * cell) * scale}px ${(colors * cell) * scale}px; background-position:-${f * size}px -${c * size}px;`;
  }

  async function accept() {
    if (busy) {
      return;
    }
    busy = true;
    error = null;
    try {
      await api.setAvatar(face, color);
      onclose();
    } catch (cause) {
      error = String(cause);
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

  onMount(async () => {
    try {
      const grid = await api.facesGrid();
      if (grid.faces > 0 && grid.colors > 0 && grid.cell > 0) {
        faces = grid.faces;
        colors = grid.colors;
        cell = grid.cell;
        faceRows = grid.rows;
        face = Math.min(face, faces - 1);
        color = Math.min(color, colors - 1);
      }
    } catch {
      // Keep the sequential fallback: a picker that draws the wrong grid is
      // worse than one that draws a plain one.
    }
    dialog?.querySelector<HTMLButtonElement>(".face-cell.selected")?.focus();
  });
</script>

<div class="dialog-backdrop">
  <div
    class="dialog"
    role="dialog"
    aria-modal="true"
    aria-label="Choose an Avatar"
    tabindex="-1"
    bind:this={dialog}
    onkeydown={onKeydown}
    oncontextmenu={(event) => event.preventDefault()}
  >
    <div class="dialog-head">
      <span>Choose an Avatar</span>
      <button class="dialog-x" type="button" aria-label="Close without applying" onclick={onclose}>×</button>
    </div>

    <div class="dialog-body">
      <p class="dialog-hint">Click an avatar to represent you:</p>

      <div class="face-grid">
        {#each faceRows as row}
          <div class="face-row">
            {#each row as index (index)}
              <button
                class="face-cell"
                class:selected={face === index}
                type="button"
                aria-label={`Face ${index + 1}`}
                aria-pressed={face === index}
                style={sprite(index, color)}
                onclick={() => (face = index)}
              ></button>
            {/each}
          </div>
        {/each}
      </div>

      <div class="colour-block">
        <span class="block-label" id="colour-label">Colour</span>
        <div class="colour-row" role="group" aria-labelledby="colour-label">
          {#each Array.from({ length: colors }, (_, index) => index) as index (index)}
            <button
              class="colour-cell"
              class:selected={color === index}
              type="button"
              aria-label={`Colour ${index + 1}`}
              aria-pressed={color === index}
              style={sprite(face, index, 22)}
              onclick={() => (color = index)}
            ></button>
          {/each}
        </div>
      </div>

      {#if error}<p class="dialog-error">{error}</p>{/if}

      <div class="dialog-actions">
        <span class="preview" style={sprite(face, color)} aria-hidden="true"></span>
        <button class="btn" type="button" onclick={onclose} disabled={busy}>Cancel</button>
        <button class="btn primary" type="button" onclick={accept} disabled={busy}>OK</button>
      </div>
    </div>
  </div>
</div>
