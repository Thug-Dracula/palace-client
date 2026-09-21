/**
 * The Sound preference group's rules, independent of the window.
 *
 * `PREFERENCES.md` (Task 5) is the authority for this group's keys and their
 * defaults. The group has two halves:
 *
 * * **Output and SoundFont** — `soundfont`, `audio_enabled` and `audio_volume`
 *   already exist as top level settings keys and are owned by the audio engine.
 *   This module never restates them as `prefs` keys; the group drives them
 *   through the engine's own commands, so there is exactly one source of truth.
 * * **`prefs.sound.*`** — the classic split of effects against music, and the
 *   speech voice. None of these can be backed by this build:
 *
 * | Key                      | Backing behaviour                                      |
 * |--------------------------|--------------------------------------------------------|
 * | `sound.sfx_volume`       | no — the engine mixes every sound into one master level |
 * | `sound.music_volume`     | no — same single output path                            |
 * | `sound.speech_voice`     | no — there is no speech / TTS integration in this build |
 *
 * The three unbacked options are still defined (so the stored defaults match
 * the specification and a future task can take them over), but the UI must
 * render them disabled and clearly marked, never as controls that appear to
 * work. {@link SOUND_OPTIONS} is the one table that says which is which, so the
 * component cannot drift from this module.
 */
import type { AudioState } from "./api";
import { readPref, type PrefsDocument } from "./prefs";

/** `sound.sfx_volume` default, matching `PREFERENCES.md`. */
export const DEFAULT_SFX_VOLUME = 1;

/** `sound.music_volume` default, matching `PREFERENCES.md`. */
export const DEFAULT_MUSIC_VOLUME = 1;

/** `sound.speech_voice` default: no voice chosen. */
export const DEFAULT_SPEECH_VOICE: string | null = null;

/** The `prefs.sound` values, resolved against the defaults. */
export interface SoundPrefs {
  sfxVolume: number;
  musicVolume: number;
  speechVoice: string | null;
}

/** A volume from a stored value; a missing, non-finite or out-of-range value takes `fallback`. */
function volumeOr(value: unknown, fallback: number): number {
  return typeof value === "number" && Number.isFinite(value) && value >= 0 && value <= 1
    ? value
    : fallback;
}

/** Resolve the stored sound block, falling back to the shipped defaults. */
export function soundPrefsFrom(values: PrefsDocument): SoundPrefs {
  const rawVoice = readPref<unknown>(values, ["sound", "speech_voice"], DEFAULT_SPEECH_VOICE);
  return {
    sfxVolume: volumeOr(
      readPref<unknown>(values, ["sound", "sfx_volume"], DEFAULT_SFX_VOLUME),
      DEFAULT_SFX_VOLUME,
    ),
    musicVolume: volumeOr(
      readPref<unknown>(values, ["sound", "music_volume"], DEFAULT_MUSIC_VOLUME),
      DEFAULT_MUSIC_VOLUME,
    ),
    speechVoice: typeof rawVoice === "string" ? rawVoice : DEFAULT_SPEECH_VOICE,
  };
}

/** Whether an option's effect is actually implemented by this build. */
export type OptionSupport = "supported" | "unsupported";

/** One option in the Sound group, and whether this build acts on it. */
export interface SoundOption {
  /** The stable option name, matching the key after `prefs.sound.`. */
  key: "sfx_volume" | "music_volume" | "speech_voice";
  /** The human label shown next to the control. */
  label: string;
  /** One line explaining what the option would do. */
  summary: string;
  support: OptionSupport;
  /** Why the option is not wired up. `null` when it is supported. */
  reason: string | null;
}

/**
 * Why the effects/music split cannot be offered: the engine's output path.
 *
 * Stated once here so both sliders explain the same limitation.
 */
export const SPLIT_UNSUPPORTED_REASON =
  "The audio engine mixes sound effects and MIDI music into one master output, so it cannot adjust them separately.";

/** Why no speech voice can be chosen. */
export const SPEECH_UNSUPPORTED_REASON =
  "This build has no speech or text-to-speech integration, so a voice cannot be selected or used.";

/**
 * The group's options in display order.
 *
 * Anything with `support: "unsupported"` must be rendered disabled and carry a
 * visible "Not supported" marker with its `reason`; it must not be written.
 */
export const SOUND_OPTIONS: readonly SoundOption[] = [
  {
    key: "sfx_volume",
    label: "Effects volume",
    summary: "Set the level of room sound effects independently of music.",
    support: "unsupported",
    reason: SPLIT_UNSUPPORTED_REASON,
  },
  {
    key: "music_volume",
    label: "Music volume",
    summary: "Set the level of MIDI music independently of sound effects.",
    support: "unsupported",
    reason: SPLIT_UNSUPPORTED_REASON,
  },
  {
    key: "speech_voice",
    label: "Speech voice",
    summary: "Read incoming chat aloud with a chosen voice.",
    support: "unsupported",
    reason: SPEECH_UNSUPPORTED_REASON,
  },
];

/** The keys of this group's options that have no backing behaviour. */
export const UNSUPPORTED_SOUND_KEYS: readonly string[] = SOUND_OPTIONS.filter(
  (option) => option.support === "unsupported",
).map((option) => option.key);

/** The table entry for one option key, or `null` when the key is unknown. */
export function soundOption(key: string): SoundOption | null {
  return SOUND_OPTIONS.find((option) => option.key === key) ?? null;
}

/** Whether this build acts on the option named `key` (unknown keys are not supported). */
export function isSoundOptionSupported(key: string): boolean {
  return soundOption(key)?.support === "supported";
}

/** The part of the engine's audio state the SoundFont row reads. */
export type SoundfontState = Pick<AudioState, "soundfont" | "soundfont_exists"> &
  Partial<Pick<AudioState, "soundfont_fallback" | "soundfont_bundled">>;

/** How the SoundFont row should read, and in what tone. */
export interface SoundfontSummary {
  /** `ok` for a usable font, `warn` for a missing one, `none` for no font at all. */
  tone: "ok" | "warn" | "none";
  /** The sentence shown to the user, naming the file in effect. */
  text: string;
}

/**
 * Describe what MIDI will actually be synthesized with.
 *
 * The engine resolves the font itself: a chosen file while it is on disk,
 * otherwise the bundled bank, otherwise its fallback tone. This function only
 * reports that resolution, so the row and the engine cannot disagree — in
 * particular `soundfont_fallback` means the chosen file is gone and the
 * *bundled* bank took over, while `soundfont_exists` without either flag means
 * the user's own file is loaded.
 */
export function soundfontSummary(state: SoundfontState): SoundfontSummary {
  if (state.soundfont_fallback) {
    const where = state.soundfont
      ? `the bundled bank at ${state.soundfont}`
      : "the fallback tone (no bundled bank is installed)";
    return {
      tone: "warn",
      text: `Your chosen SoundFont is missing from disk. MIDI uses ${where} instead.`,
    };
  }
  if (!state.soundfont) {
    return {
      tone: "none",
      text: "No SoundFont is available — MIDI plays the fallback tone.",
    };
  }
  if (state.soundfont_bundled) {
    return { tone: "ok", text: `The bundled bank is in use (${state.soundfont}).` };
  }
  if (!state.soundfont_exists) {
    return {
      tone: "warn",
      text: `This file is missing on disk — MIDI plays the fallback tone (${state.soundfont}).`,
    };
  }
  return { tone: "ok", text: `Selected: ${state.soundfont}` };
}
