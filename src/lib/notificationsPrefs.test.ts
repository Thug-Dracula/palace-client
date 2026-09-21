/**
 * The Notifications group's rules: opt-in default, scope mapping and the
 * honest table for the option this build cannot back.
 *
 * These are the pure rules the window and the Rust side both rely on, so they
 * are tested without a browser or a bridge.
 */
import { describe, expect, it } from "vitest";

import {
  DEFAULT_NOTIFICATION_SOUND,
  DEFAULT_NOTIFICATIONS_ENABLED,
  DEFAULT_ON_MENTION,
  DEFAULT_PRIVATE_MESSAGE,
  NOTIFICATION_SCOPE_OPTIONS,
  notificationOption,
  notificationScopeFrom,
  notificationsPrefsFrom,
  scopePatch,
  isNotificationScope,
  isNotificationOptionSupported,
} from "./notificationsPrefs";

describe("resolving the stored block", () => {
  it("is off by default and ready for both scopes when switched on", () => {
    const prefs = notificationsPrefsFrom({});
    expect(prefs.enabled).toBe(DEFAULT_NOTIFICATIONS_ENABLED);
    expect(prefs.enabled).toBe(false);
    expect(prefs.onMention).toBe(DEFAULT_ON_MENTION);
    expect(prefs.privateMessage).toBe(DEFAULT_PRIVATE_MESSAGE);
    expect(prefs.sound).toBe(DEFAULT_NOTIFICATION_SOUND);
  });

  it("reads stored values and ignores mistyped ones", () => {
    const prefs = notificationsPrefsFrom({
      notifications: {
        enabled: true,
        on_mention: false,
        private_message: true,
        sound: "yes",
      },
    });
    expect(prefs.enabled).toBe(true);
    expect(prefs.onMention).toBe(false);
    expect(prefs.privateMessage).toBe(true);
    expect(prefs.sound).toBe(DEFAULT_NOTIFICATION_SOUND);
  });
});

describe("the scope control", () => {
  it("collapses the two flags into one scope and back", () => {
    expect(
      notificationScopeFrom({ enabled: true, onMention: true, privateMessage: true, sound: false }),
    ).toBe("all");
    expect(
      notificationScopeFrom({ enabled: true, onMention: true, privateMessage: false, sound: false }),
    ).toBe("mentions");
    expect(
      notificationScopeFrom({ enabled: true, onMention: false, privateMessage: true, sound: false }),
    ).toBe("private");
    expect(
      notificationScopeFrom({ enabled: true, onMention: false, privateMessage: false, sound: false }),
    ).toBe("none");
  });

  it("writes the flag pair a chosen scope means", () => {
    expect(scopePatch("all")).toEqual({ on_mention: true, private_message: true });
    expect(scopePatch("mentions")).toEqual({ on_mention: true, private_message: false });
    expect(scopePatch("private")).toEqual({ on_mention: false, private_message: true });
    expect(scopePatch("none")).toEqual({ on_mention: false, private_message: false });
  });

  it("offers only the scopes that make sense with the master toggle", () => {
    expect(NOTIFICATION_SCOPE_OPTIONS.map((option) => option.id)).toEqual([
      "private",
      "mentions",
      "all",
    ]);
    expect(NOTIFICATION_SCOPE_OPTIONS.some((option) => option.id === "none")).toBe(false);
    expect(isNotificationScope("all")).toBe(true);
    expect(isNotificationScope("nonsense")).toBe(false);
  });
});

describe("the unsupported option", () => {
  it("marks the sound option as unbacked and explains why", () => {
    expect(isNotificationOptionSupported("sound")).toBe(false);
    const option = notificationOption("sound");
    expect(option?.support).toBe("unsupported");
    expect(option?.reason).toContain("plays its own sound");
  });
});
