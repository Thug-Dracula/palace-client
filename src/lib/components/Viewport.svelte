<script lang="ts">
  import { onMount } from "svelte";
  import * as api from "../api";
  import { store } from "../store.svelte";
  import RoomMenu from "./RoomMenu.svelte";
  import AvatarDialog from "./AvatarDialog.svelte";
  import PropBagDialog from "./PropBagDialog.svelte";
  import DetachButton from "./DetachButton.svelte";
  import EditorDialog from "./editor/EditorDialog.svelte";
  import { layoutAvatars, type AvatarLayout } from "../avatarLayout";
  import {
    formatGatherOutcome,
    pickPropAt,
    planWear,
    propLabel,
    type PickedProp,
    type WearResult,
  } from "../propPick";

  let element: HTMLDivElement | undefined = $state();
  const last = { width: 0, height: 0, dpr: 0, native: false };

  let menu = $state<{ x: number; y: number; prop: PickedProp | null } | null>(null);
  let avatarOpen = $state(false);
  let propsOpen = $state(false);
  let editorOpen = $state(false);
  let pointer = $state<{ x: number; y: number } | null>(null);

  // Gather feedback lives next to the artwork, not in a blocking dialog: a
  // short status line that clears itself.
  let feedback = $state<string | null>(null);
  let feedbackTimer: ReturnType<typeof setTimeout> | null = null;
  const FEEDBACK_MS = 6000;

  function showFeedback(text: string) {
    feedback = text;
    if (feedbackTimer) {
      clearTimeout(feedbackTimer);
    }
    feedbackTimer = setTimeout(() => {
      feedback = null;
      feedbackTimer = null;
    }, FEEDBACK_MS);
  }

  // The webview fires mousemove far faster than the runtime needs it. Coalesce
  // to one report per animation frame and drop repeats of the same CSS pixel,
  // so the runtime is not flooded with sub-pixel noise.
  let moveFrame = 0;
  let pendingMove: { x: number; y: number } | null = null;
  const lastSent = { x: -1, y: -1 };

  function onContextMenu(event: MouseEvent) {
    event.preventDefault();
    // The roster already carries every worn prop's sprite box, so a right-click
    // can be answered from the data on screen. A miss leaves the menu without a
    // prop, and it shows the room options exactly as before.
    let prop: PickedProp | null = null;
    const roster = store.avatars;
    if (element && roster) {
      const rect = element.getBoundingClientRect();
      prop = pickPropAt(roster, {
        x: event.clientX - rect.left,
        y: event.clientY - rect.top,
      });
    }
    menu = { x: event.clientX, y: event.clientY, prop };
  }

  // Gather one prop into My Bag, optionally wearing it afterwards. The wear
  // reuses the store's existing prop path; a prop that is already worn is left
  // alone, because the path toggles and toggling it would take it off.
  async function gather(picked: PickedProp, andWear: boolean): Promise<void> {
    const label = propLabel(picked.id);
    try {
      const result = await store.gatherProp(picked.id, picked.crc);
      let wear: WearResult | null = null;
      if (result.outcome !== "rejected") {
        const plan = planWear(andWear, store.self?.props.includes(picked.id) ?? false);
        if (plan.action === "already") {
          wear = { state: "already" };
        } else if (plan.action === "wear") {
          const entry = store.bagCatalog.find((candidate) => candidate.id === picked.id);
          const worn = entry ? store.toggleBagProp(entry) : store.toggleProp(picked.id);
          wear = worn.ok
            ? { state: "worn" }
            : { state: "refused", error: worn.error ?? "the store refused it" };
        }
      }
      showFeedback(formatGatherOutcome(result, label, wear));
    } catch (error) {
      showFeedback(
        `Could not gather ${label}: ${error instanceof Error ? error.message : String(error)}`,
      );
    }
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
      last.native === store.native
    ) {
      return;
    }
    Object.assign(last, { width, height, dpr, native: store.native });
    store.viewport = { width, height, dpr };
    void api.setViewport(width, height, dpr, 1, store.native).catch(() => {});
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
      if (feedbackTimer) {
        clearTimeout(feedbackTimer);
        feedbackTimer = null;
      }
    };
  });

  $effect(() => {
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

  // Recomputes only when the roster reference changes, so screen/notes/tooltip
  // updates never re-run the avatar math or touch the board images.
  const avatarLayout = $derived<AvatarLayout>(
    store.avatars ? layoutAvatars(store.avatars) : { sprites: [], names: [] },
  );

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

<div class="viewport-wrap" data-panel="room">
  <div class="toolbar">
    <div class="seg">
      <button class="btn" class:on={!store.native} type="button" onclick={() => (store.native = false)}>Fit</button>
      <button class="btn" class:on={store.native} type="button" onclick={() => (store.native = true)} title="1 room pixel = 1 CSS pixel">
        1:1
      </button>
    </div>

    <button class="btn" type="button" onclick={() => (editorOpen = true)} title="Open the prop editor">
      Editor
    </button>

    <span class="spacer"></span>
    {#if store.room}<span class="readout">{store.room.name}</span>{/if}
    <DetachButton panel="room" />
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

      <div class="avatar-layer">
        {#each avatarLayout.sprites as sprite (sprite.key)}
          <img
            class="avatar-sprite"
            src={sprite.url}
            alt=""
            draggable="false"
            style="left:{sprite.left}px; top:{sprite.top}px; width:{sprite.width}px; height:{sprite.height}px; opacity:{sprite.alpha};"
          />
        {/each}
        {#each avatarLayout.names as tag (tag.key)}
          <div class="avatar-name" style="left:{tag.left}px; top:{tag.top}px;">{tag.name}</div>
        {/each}
      </div>

      {#if store.screen.mid_version != null}
        <img
          class="board-layer"
          src={api.midUrl(store.screen.mid_version)}
          alt=""
          draggable="false"
          style="left:{geometry.content_x}px; top:{geometry.content_y}px; width:{geometry.content_w}px; height:{geometry.content_h}px;"
        />
      {/if}

      {#if store.screen.top_version != null}
        <img
          class="board-layer"
          src={api.topUrl(store.screen.top_version)}
          alt=""
          draggable="false"
          style="left:{geometry.content_x}px; top:{geometry.content_y}px; width:{geometry.content_w}px; height:{geometry.content_h}px;"
        />
      {/if}

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

    {#if feedback}
      <div class="gather-toast" role="status" aria-live="polite">{feedback}</div>
    {/if}
  </div>

  {#if menu}
    {@const picked = menu.prop}
    <RoomMenu
      x={menu.x}
      y={menu.y}
      prop={picked}
      onclose={() => (menu = null)}
      onchooseavatar={() => (avatarOpen = true)}
      onprops={() => (propsOpen = true)}
      ongather={(andWear) => {
        if (picked) {
          void gather(picked, andWear);
        }
      }}
    />
  {/if}

  {#if avatarOpen}
    <AvatarDialog onclose={() => (avatarOpen = false)} />
  {/if}

  {#if propsOpen}
    <PropBagDialog onclose={() => (propsOpen = false)} />
  {/if}

  {#if editorOpen}
    <EditorDialog onclose={() => (editorOpen = false)} />
  {/if}
</div>

<style>
  /* Overlay geometry matches the base frame: same content box, same scale.
     Every layer is presentation-only, so pointer events fall through to the
     .viewport handlers that own click/hover/drag. */
  .avatar-layer {
    position: absolute;
    inset: 0;
    pointer-events: none;
  }

  .avatar-sprite {
    position: absolute;
    display: block;
    pointer-events: none;
    user-select: none;
    -webkit-user-select: none;
    -webkit-user-drag: none;
  }

  .avatar-name {
    position: absolute;
    transform: translateX(-50%);
    white-space: nowrap;
    line-height: 1;
    font-size: var(--fs-xs);
    color: var(--text-0);
    text-shadow: 0 1px 2px #000, 0 0 6px #000;
    pointer-events: none;
    user-select: none;
    -webkit-user-select: none;
  }

  .board-layer {
    position: absolute;
    background: transparent;
    pointer-events: none;
    user-select: none;
    -webkit-user-select: none;
    -webkit-user-drag: none;
  }

  /* Small pixel-art sprite layers are scaled up by the room scale, so nearest
     neighbour keeps their hard edges, matching the board's pixelated look. */
  .avatar-sprite,
  .board-layer {
    image-rendering: pixelated;
    image-rendering: crisp-edges;
  }

  /* A gather result, announced politely and never blocking the next click. */
  .gather-toast {
    position: absolute;
    left: 50%;
    bottom: var(--sp-3);
    z-index: 5;
    max-width: min(420px, calc(100% - var(--sp-5)));
    transform: translateX(-50%);
    padding: var(--sp-1) var(--sp-3);
    font-size: var(--fs-xs);
    line-height: 1.5;
    color: var(--text-0);
    text-align: center;
    background: rgba(5, 7, 10, 0.92);
    border: 1px solid var(--line-strong);
    border-radius: var(--r-sm);
    box-shadow: 0 6px 18px rgba(0, 0, 0, 0.5);
    pointer-events: none;
  }
</style>
