<script lang="ts">
  import * as api from "../api";
  import { store } from "../store.svelte";

  let draft = $state("");
  let log: HTMLDivElement | undefined = $state();

  $effect(() => {
    void store.chat.length;
    if (log) {
      log.scrollTop = log.scrollHeight;
    }
  });

  function send() {
    const text = draft.trim();
    if (!text || !store.connected) {
      return;
    }
    draft = "";
    store.echoLocal(text);
    void api.say(text).catch(() => {});
  }
</script>

<section class="chat">
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
      bind:value={draft}
      placeholder={store.connected ? "Type a message…" : "Not connected"}
      disabled={!store.connected}
      maxlength="254"
      spellcheck="false"
      aria-label="Chat message"
    />
    <button class="btn primary" type="submit" disabled={!store.connected || !draft.trim()}>Send</button>
  </form>
</section>
