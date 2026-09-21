/**
 * Task 23's rule-level tests: the Sound group's stored values and its support
 * table. The component-level behaviour lives in `soundGroup.dom.test.ts`.
 */
import { describe, expect, it } from "vitest";

import {
  DEFAULT_MUSIC_VOLUME,
  DEFAULT_SFX_VOLUME,
  DEFAULT_SPEECH_VOICE,
  SOUND_OPTIONS,
  SPEECH_UNSUPPORTED_REASON,
  SPLIT_UNSUPPORTED_REASON,
  UNSUPPORTED_SOUND_KEYS,
  isSoundOptionSupported,
  soundPrefsFrom,
  soundfontSummary,
  soundOption,
} from "./soundPrefs";

describe("resolving prefs.sound", () => {
  it("uses the specification's defaults when the block is absent", () => {
    expect(soundPrefsFrom({})).toEqual({
      sfxVolume: DEFAULT_SFX_VOLUME,
      musicVolume: DEFAULT_MUSIC_VOLUME,
      speechVoice: DEFAULT_SPEECH_VOICE,
    });
    expect(DEFAULT_SFX_VOLUME).toBe(1);
    expect(DEFAULT_MUSIC_VOLUME).toBe(1);
    expect(DEFAULT_SPEECH_VOICE).toBeNull();
  });

  it("reads stored values back", () => {
    expect(
      soundPrefsFrom({
        sound: { sfx_volume: 0.25, music_volume: 0.75, speech_voice: "Ada" },
      }),
    ).toEqual({ sfxVolume: 0.25, musicVolume: 0.75, speechVoice: "Ada" });
  });

  it("falls back to the defaults for values it cannot honour", () => {
    expect(
      soundPrefsFrom({
        sound: { sfx_volume: 2, music_volume: "loud", speech_voice: 7 },
      }),
    ).toEqual({
      sfxVolume: DEFAULT_SFX_VOLUME,
      musicVolume: DEFAULT_MUSIC_VOLUME,
      speechVoice: DEFAULT_SPEECH_VOICE,
    });
    expect(soundPrefsFrom({ sound: { sfx_volume: Number.NaN } }).sfxVolume).toBe(
      DEFAULT_SFX_VOLUME,
    );
    expect(soundPrefsFrom({ sound: null }).sfxVolume).toBe(DEFAULT_SFX_VOLUME);
  });
});

describe("the support table", () => {
  it("marks the effects/music split and the speech voice unsupported", () => {
    expect(SOUND_OPTIONS.map((option) => option.key)).toEqual([
      "sfx_volume",
      "music_volume",
      "speech_voice",
    ]);
    for (const option of SOUND_OPTIONS) {
      expect(option.support).toBe("unsupported");
      expect(option.reason).toBeTruthy();
      expect(isSoundOptionSupported(option.key)).toBe(false);
    }
    expect(UNSUPPORTED_SOUND_KEYS).toEqual(["sfx_volume", "music_volume", "speech_voice"]);
  });

  it("explains the engine's single output path and the missing speech platform", () => {
    expect(soundOption("sfx_volume")?.reason).toBe(SPLIT_UNSUPPORTED_REASON);
    expect(soundOption("music_volume")?.reason).toBe(SPLIT_UNSUPPORTED_REASON);
    expect(soundOption("speech_voice")?.reason).toBe(SPEECH_UNSUPPORTED_REASON);
    expect(SPLIT_UNSUPPORTED_REASON).toContain("one master output");
    expect(SPEECH_UNSUPPORTED_REASON).toContain("speech");
    expect(soundOption("nope")).toBeNull();
    expect(isSoundOptionSupported("nope")).toBe(false);
  });
});

describe("describing the SoundFont in effect", () => {
  it("reports a chosen file that exists as selected", () => {
    const summary = soundfontSummary({
      soundfont: "/fonts/mine.sf2",
      soundfont_exists: true,
      soundfont_fallback: false,
      soundfont_bundled: false,
    });
    expect(summary.tone).toBe("ok");
    expect(summary.text).toContain("/fonts/mine.sf2");
  });

  it("names the bundled bank when no file was chosen", () => {
    const summary = soundfontSummary({
      soundfont: "/bundle/GeneralUser-GS.sf2",
      soundfont_exists: true,
      soundfont_fallback: false,
      soundfont_bundled: true,
    });
    expect(summary.tone).toBe("ok");
    expect(summary.text).toContain("bundled bank");
    expect(summary.text).toContain("/bundle/GeneralUser-GS.sf2");
  });

  it("warns that a missing chosen file fell back to the bundled bank", () => {
    const summary = soundfontSummary({
      soundfont: "/bundle/GeneralUser-GS.sf2",
      soundfont_exists: true,
      soundfont_fallback: true,
      soundfont_bundled: false,
    });
    expect(summary.tone).toBe("warn");
    expect(summary.text).toContain("missing");
    expect(summary.text).toContain("/bundle/GeneralUser-GS.sf2");
  });

  it("warns about the fallback tone when no bank is installed either", () => {
    const summary = soundfontSummary({
      soundfont: null,
      soundfont_exists: false,
      soundfont_fallback: true,
    });
    expect(summary.tone).toBe("warn");
    expect(summary.text).toContain("fallback tone");
  });

  it("reports no font at all without inventing one", () => {
    const summary = soundfontSummary({ soundfont: null, soundfont_exists: false });
    expect(summary.tone).toBe("none");
    expect(summary.text).toContain("fallback tone");
  });
});
