<script lang="ts">
  import { onMount } from "svelte";
  import * as api from "./api";
  import { store } from "./store.svelte";
  import TopBar from "./components/TopBar.svelte";
  import RoomList from "./components/RoomList.svelte";
  import Viewport from "./components/Viewport.svelte";
  import UserList from "./components/UserList.svelte";
  import ChatPanel from "./components/ChatPanel.svelte";
  import StatusBar from "./components/StatusBar.svelte";
  import "./styles/app.css";

  const SCALE_STEP = 0.05;

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
    let unlisten: (() => void) | undefined;
    let disposed = false;
    void (async () => {
      const stop = await api.onEvent((event) => store.apply(event));
      if (disposed) {
        stop();
        return;
      }
      unlisten = stop;
      // The backend auto-connects before this listener exists; ask it to replay
      // the current status, banner, lists and frame so the UI starts in sync.
      await api.refresh().catch(() => {});
    })();
    window.addEventListener("wheel", onWheel, { passive: false });
    return () => {
      disposed = true;
      unlisten?.();
      window.removeEventListener("wheel", onWheel);
    };
  });
</script>

<svelte:window onkeydown={onKeydown} />

<div class="app">
  <TopBar />
  <div class="workspace">
    <RoomList />
    <div class="center">
      <Viewport />
      <ChatPanel />
    </div>
    <UserList />
  </div>
  <StatusBar />
</div>
