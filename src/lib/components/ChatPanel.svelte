<script lang="ts">
  import { onMount } from "svelte";
  import * as api from "../api";
  import { CHAT_INPUT_LIMIT, clampChatDraft } from "../chat";
  import { store } from "../store.svelte";
  import { isPanelWindow } from "../windowRole";
  import DetachButton from "./DetachButton.svelte";

  let draft = $state("");
  let log: HTMLDivElement | undefined = $state();
  let input: HTMLInputElement | undefined = $state();
  // The transcript writer's state, read once on mount: a visible sign that the
  // room is being written to disk, with the file named in the tooltip.
  let logStatus = $state<api.ChatLogStatus | null>(null);

  // A detached panel window has no TopBar, so nothing else fetches settings for
  // it. `echoLocal` names the sender from `store.settings`; without this seed a
  // line sent from the panel would echo as the placeholder username.
  const detached = isPanelWindow();

  $effect(() => {
    void store.chat.length;
    if (log) {
      log.scrollTop = log.scrollHeight;
    }
  });

  // Give a detached chat window the keyboard focus it exists for, once the
  // replayed status says the session is live (the input is disabled until then).
  let focused = false;
  $effect(() => {
    if (!detached || focused || !input || !store.connected) {
      return;
    }
    input.focus();
    focused = true;
  });

  onMount(() => {
    void api
      .chatLogStatus()
      .then((state) => {
        logStatus = state;
      })
      .catch(() => {});
    if (!detached) {
      return;
    }
    void api
      .getSettings()
      .then((settings) => {
        store.settings = settings;
      })
      .catch(() => {});
  });

  function send() {
    const text = clampChatDraft(draft).trim();
    if (!text || !store.connected) {
      return;
    }
    draft = "";
    if (detached) {
      input?.focus();
    }
    store.echoLocal(text);
    void api.say(text).catch(() => {});
  }
</script>

<section class="chat" data-panel="chat">
  <div class="panel-head">
    <span>Chat</span>
    <div class="panel-head-tools">
      {#if logStatus?.enabled}
        <span
          class="log-chip"
          data-testid="chat-log-chip"
          title="Chat transcript being written to {logStatus.path}"
        >
          log
        </span>
      {/if}
      <span class="count">{store.chat.length}</span>
      <DetachButton panel="chat" />
    </div>
  </div>
  <div class="chat-log" bind:this={log}>
    {#each store.chat as line (line.seq)}
      <div class="line {line.kind}" class:pending={line.pending}>
        <span class="who">{line.name}</span>
        <span class="text">{line.text}</span>
      </div>
    {/each}
    {#if !store.chat.length}
      <div class="empty-note">
        {store.connected ? "Say hello." : "Chat appears here once connected."}
      </div>
    {/if}
  </div>

  <form
    class="chat-input"
    onsubmit={(event) => {
      event.preventDefault();
      send();
    }}
  >
    <input
      bind:this={input}
      bind:value={draft}
      placeholder={store.connected ? "Type a message…" : "Not connected"}
      disabled={!store.connected}
      maxlength={CHAT_INPUT_LIMIT}
      spellcheck="false"
      aria-label="Chat message"
    />
    <button class="btn primary" type="submit" disabled={!store.connected || !draft.trim()}>Send</button>
  </form>
</section>

<style>
  .log-chip {
    padding: 0 var(--sp-1);
    border: 1px solid var(--amber-dim);
    border-radius: var(--r-xs);
    color: var(--amber-dim);
    letter-spacing: 0.08em;
  }
</style>
