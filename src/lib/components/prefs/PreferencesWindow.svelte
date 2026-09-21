<script lang="ts">
  import { onMount } from "svelte";

  import * as api from "../../api";
  import { PREF_GROUPS } from "../../prefs";
  import { prefs } from "../../prefsStore.svelte";
  import AppearanceGroup from "./AppearanceGroup.svelte";
  import AvatarGroup from "./AvatarGroup.svelte";
  import ChatLogGroup from "./ChatLogGroup.svelte";
  import ConnectionGroup from "./ConnectionGroup.svelte";
  import GraphicsGroup from "./GraphicsGroup.svelte";
  import GroupPlaceholder from "./GroupPlaceholder.svelte";
  import LayoutGroup from "./LayoutGroup.svelte";
  import MuteGroup from "./MuteGroup.svelte";
  import NotificationsGroup from "./NotificationsGroup.svelte";
  import ShellGroup from "./ShellGroup.svelte";
  import SoundGroup from "./SoundGroup.svelte";

  onMount(() => {
    void prefs.load();
  });

  const active = $derived(prefs.activeGroup);
  const activeGroup = $derived(
    PREF_GROUPS.find((group) => group.id === active) ?? PREF_GROUPS[0],
  );

  async function done() {
    try {
      await api.closePreferences();
    } catch {
      // The window may already be on its way out; there is nothing to report.
    }
  }
</script>

<div class="prefs-window">
  <header class="prefs-head">
    <span class="prefs-title">Preferences</span>
    <span class="prefs-sub">Changes apply immediately, unless the group says otherwise.</span>
  </header>

  <div class="prefs-body">
    <nav class="prefs-nav" aria-label="Preference groups">
      {#each PREF_GROUPS as group (group.id)}
        <button
          class="prefs-nav-item"
          class:active={group.id === active}
          type="button"
          aria-current={group.id === active ? "page" : undefined}
          title={group.summary}
          onclick={() => prefs.selectGroup(group.id)}
        >
          <span class="prefs-nav-label">{group.label}</span>
        </button>
      {/each}
    </nav>

    <section class="prefs-content" aria-label={activeGroup.label}>
      <div class="prefs-content-head">
        <h1>{activeGroup.label}</h1>
        <p>{activeGroup.summary}</p>
      </div>

      {#if active === "connection"}
        <ConnectionGroup />
      {:else if active === "appearance"}
        <AppearanceGroup />
      {:else if active === "graphics"}
        <GraphicsGroup />
      {:else if active === "chat_log"}
        <ChatLogGroup />
      {:else if active === "avatar"}
        <AvatarGroup />
      {:else if active === "sound"}
        <SoundGroup />
      {:else if active === "mute"}
        <MuteGroup />
      {:else if active === "notifications"}
        <NotificationsGroup />
      {:else if active === "layout"}
        <LayoutGroup />
      {:else if active === "shell"}
        <ShellGroup />
      {:else}
        <GroupPlaceholder group={activeGroup} />
      {/if}
    </section>
  </div>

  <footer class="prefs-foot">
    {#if prefs.reconnectRequired}
      <span class="prefs-reconnect-chip" role="status">Reconnect required</span>
    {/if}
    <span class="prefs-status" aria-live="polite">
      {#if prefs.error}
        <span class="prefs-error">{prefs.error}</span>
      {:else if prefs.notice}
        <span class="prefs-notice">{prefs.notice}</span>
      {/if}
    </span>
    <button class="btn" type="button" onclick={() => prefs.restoreDefaults()} disabled={prefs.busy}>
      Restore defaults
    </button>
    <button class="btn primary" type="button" onclick={done}>Done</button>
  </footer>
</div>
