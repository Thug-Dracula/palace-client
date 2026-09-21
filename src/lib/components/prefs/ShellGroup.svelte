<script lang="ts">
  import { FIRST_GROUP_ID, PREF_GROUPS, lastGroupFrom } from "../../prefs";
  import { prefs } from "../../prefsStore.svelte";

  const remembered = $derived(lastGroupFrom(prefs.values));
  const rememberedLabel = $derived(
    PREF_GROUPS.find((group) => group.id === remembered)?.label ?? "",
  );
</script>

<div class="prefs-group">
  <p class="prefs-note">
    Preferences reopens on the last group you were viewing; right now that is
    <b>{rememberedLabel}</b>. Choosing a group in the list saves it immediately.
  </p>
  <div class="prefs-actions">
    <button class="btn" type="button" onclick={() => prefs.selectGroup(FIRST_GROUP_ID)}>
      Open the first group next time
    </button>
  </div>
</div>
