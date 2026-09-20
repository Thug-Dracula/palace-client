# Palace Client preferences: keyspace and apply semantics

Task 5 of the modularity plan. This is a specification, not code. It defines the
new preference keys, where each one is stored, whether it takes effect while the
client is running or needs a reconnect, and whether it is safe to put in the
settings file that is shared with the original PalaceChat client.

The shared file is `~/.config/org.palace.client/settings.json`. It is written by
both this client and the original PalaceChat client, so every rule below treats a
write as user-visible risk. The current writer and its hazards are described in
"What the writer does today and why it is a risk", and the contract that Task 8
must implement is in "Write contract".

## How to read this document

Each option row has these columns:

- **Group**: one of the ten option groups selected for the Preferences window.
- **Key**: the name as it appears in `settings.json`. A key starting with
  `prefs.` is nested inside one top level `prefs` object. Keys without that
  prefix are existing top level fields that this client already uses.
- **Type**: the JSON type stored.
- **Default**: the value used when the key is absent.
- **Applies**:
  - `live`: the option takes effect in the running session, with no reconnect.
  - `reconnect`: the option changes what a future connection uses. The UI must
    show a "reconnect required" affordance and must not silently restart the
    connection.
- **Share-safe**: whether the key may live in the shared `settings.json`:
  - `yes`: stored under `prefs`, additive only. PalaceChat's own keys are never
    touched, and a client that does not read `prefs` ignores it.
  - `separate`: stored in a different file, never in `settings.json`.
- **Status**: `EXISTING` (already in `src-tauri/src/settings.rs` today) or `NEW`
  (introduced by this work).
- **Notes**: behaviour the row depends on, or a reason it is unsafe.

The password is deliberately absent from every table. It is not a preference
key: `settings.rs:146` marks the field `#[serde(skip)]`, and
`settings.rs:489-506` pins that no password value or name reaches disk. There is
no key to add and none may be added.

## The ten option groups

1. Connection & identity
2. Appearance
3. Room & graphics behaviour
4. Sound
5. Chat logging
6. Avatar & prop behaviour
7. Mute/ignore
8. Notifications
9. Layout memory
10. Preferences shell

## Master key table

| Group | Key | Type | Default | Applies | Share-safe | Status | Notes |
|---|---|---|---|---|---|---|---|
| Connection & identity | host | string | `localhost` | reconnect | yes | EXISTING | `settings.rs:127`, env default `settings.rs:163`. Changing it must never auto-reconnect. |
| Connection & identity | port | number (u16) | `9998` | reconnect | yes | EXISTING | `settings.rs:128`, env default `settings.rs:164-167`. |
| Connection & identity | username | string | `Guest` | reconnect | yes | EXISTING | `settings.rs:129`, env default `settings.rs:168`. |
| Connection & identity | identity | object | none | reconnect | yes | EXISTING | `settings.rs:135-138`. Read-only indicator in the UI; never edited. Minted once by `ensure_identity` (`settings.rs:188-197`). |
| Connection & identity | prefs.connection.last_servers | array of objects | `[]` | reconnect | yes | NEW | Most recent first, capped at 8, each `{host, port, username}`. |
| Connection & identity | prefs.connection.home_palace | object or null | `null` | reconnect | yes | NEW | The server auto-connect offers on launch. |
| Connection & identity | prefs.connection.auto_connect | boolean | `false` | reconnect | yes | NEW | Only meaningful with a home palace set. |
| Appearance | prefs.appearance.theme | string | `crt-dark` | live | yes | NEW | The only built-in value is the current dark CRT palette (`tokens.css:4-64`). Default must reproduce today's look exactly. |
| Appearance | prefs.appearance.font_family | string | current mono stack | live | yes | NEW | Default equals `--font-mono` at `tokens.css:50`. |
| Appearance | prefs.appearance.font_size_px | number | `13` | live | yes | NEW | Default equals `--fs-md` at `tokens.css:53`. |
| Appearance | prefs.appearance.ui_scale | number | `1.0` | live | yes | NEW | Range 0.5 to 3.0, matching `MIN_UI_SCALE`/`MAX_UI_SCALE` at `commands.rs:31-32`. |
| Appearance | prefs.appearance.fullscreen | boolean | `false` | live | yes | NEW | Not persisted today; `store.svelte.ts:83` holds scale only in memory. |
| Room & graphics behaviour | prefs.graphics.show_names | boolean | `true` | live | yes | NEW | Persists the in-memory `showNames` at `store.svelte.ts:85`, toggled by `RoomMenu.svelte:125`. |
| Room & graphics behaviour | prefs.graphics.show_avatars | boolean | `true` | live | yes | NEW | Persists `showAvatars` at `store.svelte.ts:86`, toggled by `RoomMenu.svelte:139`. |
| Room & graphics behaviour | prefs.graphics.show_guests | boolean | `true` | live | yes | NEW | Classic PalaceChat option; no in-memory flag exists yet, so the filter is new work. |
| Room & graphics behaviour | prefs.graphics.animation | boolean | `true` | live | yes | NEW | Classic "Allow Animation". Needs an animated-frame gate in the render path. |
| Room & graphics behaviour | prefs.graphics.tinted_balloons | boolean | `false` | live | yes | NEW | Classic "Tinted Balloons". New render option. |
| Room & graphics behaviour | prefs.graphics.balloon_delay | string | `medium` | live | yes | NEW | One of `slow`, `medium`, `fast`. Classic "Balloon Delay". |
| Sound | soundfont | string or null | null (bundled bank) | live | yes | EXISTING | `settings.rs:130`. Validated as an existing `.sf2` at `settings.rs:339-358`. |
| Sound | audio_enabled | boolean | `true` | live | yes | EXISTING | `settings.rs:131-132`; default `settings.rs:150-152`. |
| Sound | audio_volume | number | `1.0` | live | yes | EXISTING | `settings.rs:133-134`; default `settings.rs:154-156`; clamped 0.0 to 1.0 at `commands.rs:658-664`. |
| Sound | prefs.sound.sfx_volume | number | `1.0` | live | yes | NEW | Only if the engine can split effects from music. Otherwise mark unsupported, do not fake it. |
| Sound | prefs.sound.music_volume | number | `1.0` | live | yes | NEW | Same condition as `sfx_volume`. |
| Sound | prefs.sound.speech_voice | string or null | `null` | live | yes | NEW | Mark unsupported unless a speech platform integration exists. |
| Chat logging | prefs.chat_log.to_file | boolean | `false` | live | yes | NEW | Backing subsystem built by Task 24. The in-memory log needs no key: it is backend-owned. |
| Chat logging | prefs.chat_log.path | string or null | `null` | live | yes | NEW | Chosen transcript destination. |
| Chat logging | prefs.chat_log.max_bytes | number | `8388608` (8 MiB) | live | yes | NEW | Rotation cap, so a transcript can never grow without bound. |
| Chat logging | prefs.chat_log.rotate_files | number | `3` | live | yes | NEW | Number of kept rotations. |
| Avatar & prop behaviour | prefs.avatar.prop_animation | boolean | `true` | live | yes | NEW | Only if prop animation is backed. Otherwise mark unsupported. |
| Avatar & prop behaviour | prefs.avatar.saved_avatar_slots | array | `[]` | live | yes | NEW | Saved-avatar shortcut slots. Mark not-yet-supported unless the backend supports them. |
| Mute/ignore | prefs.mute.ignore_all | boolean | `false` | live | yes | NEW | Enhancement: the classic client had no ignore list. |
| Mute/ignore | prefs.mute.identities | array of objects | `[]` | live | yes | NEW | Each `{name, identity}`. Suppression is client-side; any server mute command stays a separate, clearly indicated action. |
| Notifications | prefs.notifications.enabled | boolean | `false` | live | yes | NEW | New subsystem built by Task 26. No notification code exists today. |
| Notifications | prefs.notifications.on_mention | boolean | `true` | live | yes | NEW | Needs the Task 26 subsystem before it can do anything. |
| Notifications | prefs.notifications.private_message | boolean | `true` | live | yes | NEW | Same dependency. |
| Notifications | prefs.notifications.sound | boolean | `false` | live | yes | NEW | Same dependency. |
| Layout memory | layout.windows.<label>.x | number | none | live | separate | NEW | Per-window geometry, keyed by window label. Stored in `layout.json`, never `settings.json`. |
| Layout memory | layout.windows.<label>.y | number | none | live | separate | NEW | As above. |
| Layout memory | layout.windows.<label>.w | number | none | live | separate | NEW | As above. |
| Layout memory | layout.windows.<label>.h | number | none | live | separate | NEW | As above. |
| Layout memory | layout.windows.<label>.monitor | string or null | null | live | separate | NEW | Missing monitor is clamped to the primary monitor on restore. |
| Layout memory | layout.windows.<label>.detached | boolean | `false` | live | separate | NEW | Which panels are detached, restored at startup. |
| Layout memory | layout.panel_order | array of strings | `[]` | live | separate | NEW | Order of detached panels. |
| Preferences shell | prefs.shell.last_group | string | `first group` | live | yes | NEW | Remembers the last viewed group. UI-only state. |
| Preferences shell | layout.windows.prefs.x/y/w/h | number | none | live | separate | NEW | The Preferences window's own geometry uses the Layout memory file, same as every other window. |

The ten names above are the group names. Every row carries a valid apply value.
Every row in the Connection & identity group is connection-scoped and therefore
carries `reconnect`.

## Layout memory lives in its own file

The plan requires panel positions and sizes to persist, but also forbids writing
geometry into the shared `settings.json` (Task 7). The layout record therefore
goes to a sibling file in the same config directory:

- Path: `~/.config/org.palace.client/layout.json`.
- Shape: `{ "windows": { "<label>": { "x", "y", "w", "h", "monitor", "detached" } }, "panel_order": [ ... ] }`.
- Write timing: debounced on move and resize, and once on window close.
- Restore timing: at startup, clamped onto an available monitor.
- Share-safe: `separate`. PalaceChat never reads this file, and this client never
  merges it into `settings.json`.

Because geometry is in a separate file, the shared settings file gains only the
`prefs` block and gains nothing about window position.

## Write contract

These rules are mandatory for Task 8 and any future writer of `settings.json`.

1. **Additive only.** Read the existing file as a generic JSON map, merge only
   our keys into it, and write the result back. Never rebuild the file from a
   typed struct and overwrite it. This is explicitly new work: the prop-bag
   session added no settings keys, and today's writer does not preserve unknown
   keys (see the risk section).
2. **Preserve unknown keys verbatim.** Any key this client does not understand
   must survive with the same name, value, and order. Plan task 8, line 747.
3. **Backup before the first modified write.** Take a one-time copy of the
   original file before the first write that changes anything, so a bad merge is
   recoverable. Plan task 8, line 748.
4. **Schema version marker.** `prefs.schema_version` is an integer, currently
   `1`, written inside the `prefs` block so future migrations can be detected
   without guessing.
5. **No reordering or reformatting of existing content.** Do not rename, sort, or
   re-pretty-print keys that were already there. Plan task 5, line 536.
6. **No password, ever.** It is `#[serde(skip)]` at `settings.rs:146` and pinned
   by the test at `settings.rs:489-506`.
7. **No resurrection of legacy keys.** The old top level `puid` is
   `#[serde(default, skip_serializing)]` at `settings.rs:141-142`; it is read once
   for migration and must never be written back.
8. **Keep the atomic write.** The current writer goes to a temp file and renames
   over the target (`settings.rs:311-331`). Any new merge layer must keep that
   property so a crash cannot leave a half-written config.
9. **Keep the precedence rule for existing keys.** Environment is weakest, then
   the file, then the command line (`settings.rs:246-264`). New `prefs` keys
   resolve from file and defaults only, with no environment or command line
   source, unless a later task deliberately adds one.
10. **Separate subsystems stay separate.** Chat transcripts (Task 24) and
    window geometry (Task 7) must not be folded into `settings.json` or into the
    diagnostic log.

## What the writer does today and why it is a risk

This is stated plainly because Task 8 exists to fix it, and because the shared
file makes it user-visible.

- **The current `save()` does not preserve unknown keys.** It serialises the
  whole typed `Settings` struct with `serde_json::to_string_pretty`
  (`settings.rs:323`) and writes that over the file. Any key that is not a field
  on the struct is silently dropped on the next write. So today, changing the
  volume, choosing a SoundFont, or connecting can delete a sibling-client key.
  This is not hypothetical: it is the behaviour of the code as it stands.
- **There is no backup today.** `save()` (`settings.rs:316-331`) writes straight
  over the target through a temp file; it never keeps a copy of what was there.
- **There is no schema version today.** Nothing in the file records which client
  wrote a given key, so a migration cannot tell new from old.
- **The prop-bag work added no settings keys.** The pre-start refresh note for
  this plan records that the prop bag lives in its own folder and `BagSlot`, not
  in `settings.json`. So the additive merge and the backup are genuinely new, not
  a switch to turn on.
- **Formatting is not byte-stable today.** Fields are written in struct order
  with two-space indentation. A file edited by another client will not survive a
  current write byte-for-byte. The definition of done for the plan requires the
  keys PalaceChat uses to be byte-identical after this work, which is exactly
  what the additive merge must deliver.

## Risks

1. **Silent deletion of sibling keys (live today).** Until Task 8 lands, a
   settings write can drop any key the struct does not declare, because
   `settings.rs:323` serialises the struct and overwrites. Owner: Task 8.
   Mitigation: additive map merge plus one-time backup.
2. **Unknown-key tolerance of the original PalaceChat client is unverified.**
   The additive design never modifies its keys, but the extra `prefs` block does
   sit in the same file. If PalaceChat rejects unknown keys, the fallback is to
   move our preferences to a separate file, mirroring the layout decision. This
   must be tested before shipping, not assumed.
3. **Byte-identity is not guaranteed today.** Reordering and reformatting happen
   on every current write (`settings.rs:323`). Task 8 must prove, by test, that
   foreign keys and their order survive unchanged.
4. **Connection options must never take effect without a reconnect.** `host`,
   `port`, `username`, and `identity` all mark `reconnect`. The QA check for this
   document enforces that no row in that group carries `live`. A UI that
   reconnects silently would violate the plan's guardrails.
5. **Legacy `puid` and the credential.** Both must stay out of new writes;
   `settings.rs:141-142` and `settings.rs:146` are the pins. A merge layer that
   round-trips the raw map must be careful not to reintroduce `puid`.
6. **Geometry must not leak into the shared file.** `layout.json` is separate by
   design (`share-safe: separate`). Writing panel rectangles into
   `settings.json` would expose this work to the sibling client and is forbidden
   by Task 7.

## Verification of this document

The two QA scenarios in Task 5 are recorded as:

- `.omo/evidence/task-5-prefs-spec.txt`: all ten group names appear, every table
  row has an apply value of `live` or `reconnect`, and the preserve-unknown-keys
  statement is present.
- `.omo/evidence/task-5-prefs-safety.txt`: no Connection & identity row is
  `live`, and every write is described as additive only.
