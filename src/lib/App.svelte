<script lang="ts">
  import { onMount } from "svelte";

  import PanelHost from "./PanelHost.svelte";
  import PanelWindow from "./PanelWindow.svelte";
  import StatusBar from "./components/StatusBar.svelte";
  import TopBar from "./components/TopBar.svelte";
  import { detachedAttr, trackStyle } from "./layout";
  import { viewForHash } from "./panels";
  import { panelLayout } from "./panelLayout.svelte";
  import { startSession } from "./session";
  import { store } from "./store.svelte";
  import "./styles/app.css";

  const SCALE_STEP = 0.05;

  let { hash = typeof window === "undefined" ? "" : window.location.hash }: { hash?: string } =
    $props();
  const view = $derived(viewForHash(hash));

  // Which panels are in their own OS windows. The grid track sizes and the
  // `data-detached` hooks are derived from this one list, so the shell has a
  // single source of truth for the reflow.
  const detachedIds = $derived(panelLayout.detachedIds());
  const detachedList = $derived(detachedAttr(detachedIds));
  const workspaceStyle = $derived(trackStyle(detachedIds));

  function onWheel(event: WheelEvent) {
    if (!event.ctrlKey) {
      return;
    }
    event.preventDefault();
    store.setScale(store.scale + (event.deltaY < 0 ? SCALE_STEP : -SCALE_STEP));
  }

  function onKeydown(event: KeyboardEvent) {
    const target = event.target as HTMLElement | null;
    if (event.key === "Escape") {
      target?.blur();
      return;
    }
    if (event.ctrlKey || event.metaKey || event.altKey) {
      return;
    }
    const tag = target?.tagName;
    if (tag === "INPUT" || tag === "TEXTAREA" || target?.isContentEditable) {
      return;
    }
    switch (event.key) {
      case "+":
      case "=":
        store.setScale(store.scale + SCALE_STEP);
        break;
      case "-":
      case "_":
        store.setScale(store.scale - SCALE_STEP);
        break;
      case "0":
        store.setScale(1);
        break;
      case "1":
        store.native = !store.native;
        break;
      case "f":
        store.native = false;
        break;
      default:
        return;
    }
    event.preventDefault();
  }

  onMount(() => {
    // The main shell seeds itself; a detached panel is seeded by PanelWindow.
    // Both call the same startSession helper — only `main` also owns the
    // global zoom keys, which resize the main window through `set_ui_scale`.
    const isMain = view.kind === "main";
    const session = isMain ? startSession() : undefined;
    // Rust tells `main` when a panel window closes; docking it again keeps the
    // re-attach path (button or titlebar) from ever losing the panel.
    const stopPanelClose = isMain ? panelLayout.startCloseListener() : undefined;
    if (isMain) {
      window.addEventListener("wheel", onWheel, { passive: false });
    }
    return () => {
      session?.stop();
      stopPanelClose?.();
      window.removeEventListener("wheel", onWheel);
    };
  });
</script>

<svelte:window onkeydown={view.kind === "main" ? onKeydown : undefined} />

{#if view.kind === "panel"}
  <PanelWindow panel={view.panel} />
{:else}
  <div class="app">
    <TopBar />
    <div class="workspace" data-detached={detachedList} style={workspaceStyle}>
      <PanelHost panel="rooms" detached={detachedIds.includes("rooms")} />
      <div class="center">
        <PanelHost panel="room" detached={detachedIds.includes("room")} />
        <PanelHost panel="chat" detached={detachedIds.includes("chat")} />
      </div>
      <PanelHost panel="users" detached={detachedIds.includes("users")} />
    </div>
    <StatusBar />
  </div>
{/if}
