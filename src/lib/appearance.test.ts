/**
 * The Appearance group's rules, independent of the window.
 *
 * Two things are proven here that a weaker test would not:
 *
 * 1. The shipped defaults are read straight out of `styles/tokens.css`, so the
 *    "default theme equals the pre-change hardcoded values" claim is checked
 *    against the real stylesheet rather than a second copy of the numbers.
 * 2. {@link applyAppearance} writes a changed colour straight onto the style
 *    target (a change is live), while a value left at its default is removed so
 *    the stylesheet still owns it (an unchanged install cannot drift).
 */
import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

import {
  CRT_DARK_TOKENS,
  DEFAULT_FONT_FAMILY,
  DEFAULT_FONT_SIZE_PX,
  DEFAULT_THEME_ID,
  DEFAULT_UI_SCALE,
  EDITABLE_COLOR_TOKENS,
  MAX_FONT_SIZE_PX,
  MAX_UI_SCALE,
  MIN_FONT_SIZE_PX,
  MIN_UI_SCALE,
  applyAppearance,
  appearanceFrom,
  clearAppearance,
  effectiveOverrides,
  isThemeId,
  themeTokens,
  tokenOverridesFrom,
} from "./appearance";

/** Parse every `--name: value;` declaration out of a stylesheet. */
function declarations(css: string): Record<string, string> {
  const found: Record<string, string> = {};
  const pattern = /--([a-z0-9-]+)\s*:\s*([^;]+);/gi;
  for (const match of css.matchAll(pattern)) {
    found[match[1]] = match[2].trim();
  }
  return found;
}

const tokensCss = declarations(
  readFileSync(new URL("./styles/tokens.css", import.meta.url), "utf8"),
);

/** A throwaway style target that records what was set and removed. */
function recorder() {
  const properties = new Map<string, string>();
  return {
    properties,
    target: {
      style: {
        setProperty(name: string, value: string) {
          properties.set(name, value);
        },
        removeProperty(name: string) {
          properties.delete(name);
        },
      },
    },
  };
}

describe("the default palette", () => {
  it("is exactly the colour tokens declared in tokens.css", () => {
    for (const [token, value] of Object.entries(CRT_DARK_TOKENS)) {
      expect(tokensCss[token], `--${token} in tokens.css`).toBe(value);
    }
  });

  it("covers every colour token the stylesheet declares", () => {
    // A colour token is one whose value starts with a colour literal; spacing,
    // radii, motion and elevation are deliberate non-goals for this group.
    const colourValue = /^(#|rgba?\(|hsla?\()/i;
    const cssColourTokens = Object.entries(tokensCss)
      .filter(([, value]) => colourValue.test(value))
      .map(([token]) => token)
      .sort();
    expect(Object.keys(CRT_DARK_TOKENS).sort()).toEqual(cssColourTokens);
  });

  it("takes the font defaults from the stylesheet too", () => {
    expect(tokensCss["font-mono"]).toBe(DEFAULT_FONT_FAMILY);
    expect(Number.parseInt(tokensCss["fs-md"], 10)).toBe(DEFAULT_FONT_SIZE_PX);
  });
});

describe("appearanceFrom", () => {
  it("returns the shipped defaults for an empty document", () => {
    const appearance = appearanceFrom({});
    expect(appearance.theme).toBe(DEFAULT_THEME_ID);
    expect(appearance.fontFamily).toBe(DEFAULT_FONT_FAMILY);
    expect(appearance.fontSizePx).toBe(DEFAULT_FONT_SIZE_PX);
    expect(appearance.uiScale).toBe(DEFAULT_UI_SCALE);
    expect(appearance.fullscreen).toBe(false);
    expect(appearance.tokens).toEqual({});
  });

  it("falls back to the default theme when the stored id is not built in", () => {
    expect(isThemeId("crt-dark")).toBe(true);
    expect(isThemeId("neon")).toBe(false);
    expect(appearanceFrom({ appearance: { theme: "neon" } }).theme).toBe(DEFAULT_THEME_ID);
    expect(appearanceFrom({ appearance: { theme: 7 } }).theme).toBe(DEFAULT_THEME_ID);
  });

  it("clamps a size and a scale out of range instead of trusting them", () => {
    const huge = appearanceFrom({ appearance: { font_size_px: 400, ui_scale: 99 } });
    expect(huge.fontSizePx).toBe(MAX_FONT_SIZE_PX);
    expect(huge.uiScale).toBe(MAX_UI_SCALE);

    const tiny = appearanceFrom({ appearance: { font_size_px: 1, ui_scale: 0.01 } });
    expect(tiny.fontSizePx).toBe(MIN_FONT_SIZE_PX);
    expect(tiny.uiScale).toBe(MIN_UI_SCALE);

    const junk = appearanceFrom({ appearance: { font_size_px: "wide", ui_scale: {} } });
    expect(junk.fontSizePx).toBe(DEFAULT_FONT_SIZE_PX);
    expect(junk.uiScale).toBe(DEFAULT_UI_SCALE);
  });

  it("reads a blank font family as the default", () => {
    expect(appearanceFrom({ appearance: { font_family: "   " } }).fontFamily).toBe(
      DEFAULT_FONT_FAMILY,
    );
    expect(appearanceFrom({ appearance: { font_family: "Comic Sans" } }).fontFamily).toBe(
      "Comic Sans",
    );
  });

  it("only accepts hex colours for the editable tokens", () => {
    const tokens = tokenOverridesFrom({
      appearance: {
        tokens: { amber: "#00ff00", cyan: "red", green: "#abc", blue: "#0000ff" },
      },
    });
    expect(tokens).toEqual({ amber: "#00ff00", green: "#abc" });
  });
});

describe("applyAppearance", () => {
  it("sets a changed colour token and removes a token left at its default", () => {
    const { properties, target } = recorder();

    const changed = appearanceFrom({
      appearance: { tokens: { amber: "#00ff00", cyan: "#5ccfe6" } },
    });
    applyAppearance(changed, target);

    expect(properties.get("--amber")).toBe("#00ff00");
    expect(
      properties.has("--cyan"),
      "a token stored as its default value must not become an inline override",
    ).toBe(false);
  });

  it("removes an override when the value returns to the default", () => {
    const { properties, target } = recorder();
    applyAppearance(appearanceFrom({ appearance: { tokens: { red: "#123456" } } }), target);
    expect(properties.get("--red")).toBe("#123456");

    applyAppearance(appearanceFrom({}), target);
    expect(properties.has("--red")).toBe(false);
    expect(properties.size).toBe(0);
  });

  it("sets the font family and size, and clears them at their defaults", () => {
    const { properties, target } = recorder();
    applyAppearance(
      appearanceFrom({ appearance: { font_family: "IBM Plex Mono", font_size_px: 16 } }),
      target,
    );
    expect(properties.get("--font-mono")).toBe("IBM Plex Mono");
    expect(properties.get("--fs-md")).toBe("16px");

    applyAppearance(appearanceFrom({}), target);
    expect(properties.has("--font-mono")).toBe(false);
    expect(properties.has("--fs-md")).toBe(false);
  });

  it("leaves nothing inline for a default appearance", () => {
    const { properties, target } = recorder();
    applyAppearance(appearanceFrom({}), target);
    expect(properties.size).toBe(0);
  });

  it("clearAppearance removes every property the applier owns", () => {
    const { properties, target } = recorder();
    applyAppearance(
      appearanceFrom({
        appearance: {
          tokens: { amber: "#111111", red: "#222222" },
          font_family: "Fira Code",
          font_size_px: 20,
        },
      }),
      target,
    );
    expect(properties.size).toBe(4);
    clearAppearance(target);
    expect(properties.size).toBe(0);
  });

  it("does nothing when there is no document to write to", () => {
    expect(() => applyAppearance(appearanceFrom({}), null)).not.toThrow();
    expect(() => clearAppearance(null)).not.toThrow();
  });

  it("is a no-op on the real default, so tokens.css stays authoritative", () => {
    for (const token of EDITABLE_COLOR_TOKENS) {
      expect(themeTokens(DEFAULT_THEME_ID)[token], `--${token}`).toBe(CRT_DARK_TOKENS[token]);
      expect(effectiveOverrides(appearanceFrom({})).hasOwnProperty(token)).toBe(false);
    }
  });
});
