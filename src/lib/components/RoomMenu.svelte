<script lang="ts">
  import { onMount } from "svelte";
  import { store } from "../store.svelte";
  import { propLabel, type PickedProp } from "../propPick";

  let {
    x,
    y,
    prop = null,
    onclose,
    onchooseavatar,
    onprops,
    ongather = () => {},
  }: {
    x: number;
    y: number;
    /** The worn prop under the cursor, when the right-click landed on one. */
    prop?: PickedProp | null;
    onclose: () => void;
    onchooseavatar: () => void;
    onprops: () => void;
    /** Gather the picked prop, optionally wearing it straight after. */
    ongather?: (andWear: boolean) => void;
  } = $props();

  const MARGIN = 8;

  let menu: HTMLDivElement | undefined = $state();
  let placed = $state({ x: 0, y: 0 });
  let ready = $state(false);

  onMount(() => {
    if (menu) {
      const width = menu.offsetWidth;
      const height = menu.offsetHeight;
      placed = {
        x: Math.max(MARGIN, Math.min(x, window.innerWidth - width - MARGIN)),
        y: Math.max(MARGIN, Math.min(y, window.innerHeight - height - MARGIN)),
      };
      menu.querySelector<HTMLButtonElement>(".ctx-item")?.focus();
    }
    ready = true;
  });

  function onPointerDown(event: PointerEvent) {
    if (menu && event.target instanceof Node && menu.contains(event.target)) {
      return;
    }
    onclose();
  }

  function onKeydown(event: KeyboardEvent) {
    if (event.key !== "ArrowDown" && event.key !== "ArrowUp") {
      return;
    }
    const items = menu ? [...menu.querySelectorAll<HTMLButtonElement>(".ctx-item")] : [];
    if (!items.length) {
      return;
    }
    event.preventDefault();
    const index = items.indexOf(document.activeElement as HTMLButtonElement);
    const delta = event.key === "ArrowDown" ? 1 : -1;
    const next = index < 0 ? (delta === 1 ? 0 : items.length - 1) : (index + delta + items.length) % items.length;
    items[next]?.focus();
  }
</script>

<svelte:window
  onpointerdown={onPointerDown}
  onblur={onclose}
  onkeydown={(event) => {
    if (event.key === "Escape") {
      event.preventDefault();
      onclose();
    }
  }}
/>

<div
  class="ctx-menu"
  bind:this={menu}
  role="menu"
  aria-label="Room options"
  tabindex="-1"
  style="left:{placed.x}px; top:{placed.y}px; visibility:{ready ? "visible" : "hidden"};"
  oncontextmenu={(event) => event.preventDefault()}
  onkeydown={onKeydown}
>
  {#if prop}
    <div class="ctx-head">{propLabel(prop.id)}</div>

    <button
      class="ctx-item"
      type="button"
      role="menuitem"
      onclick={() => {
        onclose();
        ongather(false);
      }}
    >
      <span class="ctx-check" aria-hidden="true"></span>
      <span>Gather</span>
    </button>

    <button
      class="ctx-item"
      type="button"
      role="menuitem"
      onclick={() => {
        onclose();
        ongather(true);
      }}
    >
      <span class="ctx-check" aria-hidden="true"></span>
      <span>Gather &amp; Wear</span>
    </button>

    <div class="ctx-sep" role="separator"></div>
  {/if}

  <button
    class="ctx-item"
    type="button"
    role="menuitemcheckbox"
    aria-checked={store.showNames}
    onclick={() => {
      store.setVisibility(!store.showNames, store.showAvatars);
      onclose();
    }}
  >
    <span class="ctx-check" aria-hidden="true">{store.showNames ? "✓" : ""}</span>
    <span>Show names</span>
  </button>

  <button
    class="ctx-item"
    type="button"
    role="menuitemcheckbox"
    aria-checked={store.showAvatars}
    onclick={() => {
      store.setVisibility(store.showNames, !store.showAvatars);
      onclose();
    }}
  >
    <span class="ctx-check" aria-hidden="true">{store.showAvatars ? "✓" : ""}</span>
    <span>Show avatars</span>
  </button>

  <div class="ctx-sep" role="separator"></div>

  <button
    class="ctx-item"
    type="button"
    role="menuitem"
    onclick={() => {
      onclose();
      onchooseavatar();
    }}
  >
    <span class="ctx-check" aria-hidden="true"></span>
    <span>Choose an Avatar…</span>
  </button>

  <button
    class="ctx-item"
    type="button"
    role="menuitem"
    onclick={() => {
      onclose();
      onprops();
    }}
  >
    <span class="ctx-check" aria-hidden="true"></span>
    <span>Props…</span>
  </button>

  <button
    class="ctx-item"
    type="button"
    role="menuitem"
    disabled={!store.self?.props.length}
    onclick={() => {
      store.takeOffAvatar();
      onclose();
    }}
  >
    <span class="ctx-check" aria-hidden="true"></span>
    <span>Take off avatar</span>
  </button>
</div>

<style>
  /* A non-interactive caption, so the gather actions read as one prop's menu. */
  .ctx-head {
    padding: var(--sp-1) var(--sp-2);
    color: var(--text-3);
    font-size: var(--fs-xs);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
</style>
