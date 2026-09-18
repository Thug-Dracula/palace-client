<script lang="ts">
  import { onMount } from "svelte";
  import * as api from "../api";
  import { store } from "../store.svelte";
  import RoomMenu from "./RoomMenu.svelte";
  import AvatarDialog from "./AvatarDialog.svelte";

  let element: HTMLDivElement | undefined = $state();
  const last = { width: 0, height: 0, dpr: 0, zoom: -1, native: false };

  let menu = $state<{ x: number; y: number } | null>(null);
  let avatarOpen = $state(false);
  let pointer = $state<{ x: number; y: number } | null>(null);

  // The webview fires mousemove far faster than the runtime needs it. Coalesce
  // to one report per animation frame and drop repeats of the same CSS pixel,
  // so the runtime is not flooded with sub-pixel noise.
  let moveFrame = 0;
  let pendingMove: { x: number; y: number } | null = null;
  const lastSent = { x: -1, y: -1 };

  function onContextMenu(event: MouseEvent) {
    event.preventDefault();
    menu = { x: event.clientX, y: event.clientY };
  }

  function push() {
    if (!element) {
      return;
    }
    const rect = element.getBoundingClientRect();
    const width = Math.max(1, Math.round(rect.width));
    const height = Math.max(1, Math.round(rect.height));
    const dpr = window.devicePixelRatio || 1;
    if (
      last.width === width &&
      last.height === height &&
      last.dpr === dpr &&
      last.zoom === store.zoom &&
      last.native === store.native
    ) {
      return;
    }
    Object.assign(last, { width, height, dpr, zoom: store.zoom, native: store.native });
    store.viewport = { width, height, dpr };
    void api.setViewport(width, height, dpr, store.zoom, store.native).catch(() => {});
  }

  onMount(() => {
    const observer = new ResizeObserver(() => push());
    if (element) {
      observer.observe(element);
    }
    push();
    return () => {
      observer.disconnect();
      if (moveFrame) {
        cancelAnimationFrame(moveFrame);
        moveFrame = 0;
      }
    };
  });

  $effect(() => {
    void store.zoom;
    void store.native;
    push();
  });

  // If the runtime's geometry was computed for a different viewport than the
  // one on screen (a resize that landed before layout settled), re-report it.
  $effect(() => {
    const geometry = store.screen?.geometry;
    if (!geometry || !element) {
      return;
    }
    const rect = element.getBoundingClientRect();
    const width = Math.max(1, Math.round(rect.width));
    const height = Math.max(1, Math.round(rect.height));
    if (Math.abs(width - geometry.viewport_w) > 1 || Math.abs(height - geometry.viewport_h) > 1) {
      last.width = 0;
      last.height = 0;
      push();
    }
  });

  const geometry = $derived(store.screen?.geometry ?? null);

  const tooltipAt = $derived.by(() => {
    if (pointer) {
      return { x: pointer.x + 14, y: pointer.y + 16 };
    }
    if (geometry) {
      return { x: geometry.content_x + 12, y: geometry.content_y + 12 };
    }
    return { x: 12, y: 12 };
  });

  // Hotspot clicks go to the runtime in viewport pixels; it maps them through
  // the same transform that placed the frame, then hit-tests the room.
  function onClick(event: MouseEvent) {
    if (!element) {
      return;
    }
    const rect = element.getBoundingClientRect();
    const x = event.clientX - rect.left;
    const y = event.clientY - rect.top;
    void api.click(x, y).catch(() => {});
  }

  function onMouseMove(event: MouseEvent) {
    if (!element) {
      return;
    }
    const rect = element.getBoundingClientRect();
    pointer = { x: event.clientX - rect.left, y: event.clientY - rect.top };
    pendingMove = pointer;
    if (moveFrame) {
      return;
    }
    moveFrame = requestAnimationFrame(() => {
      moveFrame = 0;
      const next = pendingMove;
      pendingMove = null;
      if (!next) {
        return;
      }
      const rx = Math.round(next.x);
      const ry = Math.round(next.y);
      if (rx === lastSent.x && ry === lastSent.y) {
        return;
      }
      lastSent.x = rx;
      lastSent.y = ry;
      void api.mousemove(next.x, next.y).catch(() => {});
    });
  }

  function onMouseLeave() {
    pointer = null;
    if (moveFrame) {
      cancelAnimationFrame(moveFrame);
      moveFrame = 0;
    }
    pendingMove = null;
    lastSent.x = -1;
    lastSent.y = -1;
    void api.mouseLeave().catch(() => {});
  }
</script>

<div class="viewport-wrap">
  <div class="toolbar">
    <span>zoom</span>
    <div class="seg">
      <button class="btn" type="button" onclick={() => store.nudgeZoom(-0.25)} title="Zoom out">−</button>
      <button class="btn" type="button" onclick={() => store.setZoom(1)} title="Reset to 100%">1×</button>
      <button class="btn" type="button" onclick={() => store.nudgeZoom(0.25)} title="Zoom in">+</button>
    </div>
    <input
      type="range"
      min="0.5"
      max="3"
      step="0.05"
      value={store.zoom}
      oninput={(event) => store.setZoom(Number(event.currentTarget.value))}
      aria-label="Zoom"
    />
    <span class="readout">×{store.zoom.toFixed(2)}</span>

    <div class="seg">
      <button class="btn" class:on={!store.native} type="button" onclick={() => (store.native = false)}>Fit</button>
      <button class="btn" class:on={store.native} type="button" onclick={() => (store.native = true)} title="1 room pixel = 1 CSS pixel">
        1:1
      </button>
    </div>

    <span class="spacer"></span>
    {#if store.room}<span class="readout">{store.room.name}</span>{/if}
  </div>

  <div
    class="viewport"
    bind:this={element}
    role="button"
    tabindex="0"
    onclick={onClick}
    onkeydown={() => {}}
    oncontextmenu={onContextMenu}
    onmousemove={onMouseMove}
    onmouseleave={onMouseLeave}
  >
    {#if store.screen && geometry}
      <img
        class="frame"
        src={api.frameUrl(store.screen.version)}
        alt={`Room ${store.screen.room_name}`}
        draggable="false"
        style="left:{geometry.content_x}px; top:{geometry.content_y}px; width:{geometry.content_w}px; height:{geometry.content_h}px;"
      />
      <div class="badge">
        <span>{store.screen.room_id} · {geometry.room_w.toFixed(0)}×{geometry.room_h.toFixed(0)}</span>
        <span>buffer {geometry.bitmap_w}×{geometry.bitmap_h}</span>
        <span>scale ×{geometry.scale.toFixed(3)}</span>
        <span>dpr {geometry.dpr.toFixed(1)}</span>
      </div>
    {:else}
      <div class="empty">
        <div class="big">{store.connected ? "Waiting for a room" : "Offline"}</div>
        {#if store.connected}
          The server has not described a room yet. Pick one from the list on the left.
        {:else}
          Set a host and name above, then connect.
        {/if}
      </div>
    {/if}

    {#if store.notes.length || store.notices.length}
      <div class="notes">
        {#each [...store.notes, ...store.notices].slice(-4) as note}<div>{note}</div>{/each}
      </div>
    {/if}

    {#if store.tooltip}
      <div class="tooltip" style="left:{tooltipAt.x}px; top:{tooltipAt.y}px;">{store.tooltip}</div>
    {/if}
  </div>

  {#if menu}
    <RoomMenu
      x={menu.x}
      y={menu.y}
      onclose={() => (menu = null)}
      onchooseavatar={() => (avatarOpen = true)}
    />
  {/if}

  {#if avatarOpen}
    <AvatarDialog onclose={() => (avatarOpen = false)} />
  {/if}
</div>
