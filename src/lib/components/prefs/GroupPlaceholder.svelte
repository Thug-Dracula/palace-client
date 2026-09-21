<script lang="ts">
  import type { PrefGroup } from "../../prefs";

  let { group }: { group: PrefGroup } = $props();
</script>

<div class="prefs-group prefs-placeholder">
  <p class="prefs-note">
    The <b>{group.label}</b> options are not wired up in this build yet. The window, navigation and
    apply semantics around them are in place.
  </p>
</div>
