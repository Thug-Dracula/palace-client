/**
 * The Appearance preference group's rules, independent of the window.
 *
 * `PREFERENCES.md` (Task 5) is the authority for this group's keys and their
 * defaults. Before this module every value below was hardcoded in
 * `styles/tokens.css`; the defaults here are the same values, so an install
 * that changes nothing renders exactly as it did before. The built-in theme is
 * the dark CRT palette and nothing else: this module exposes the existing
 * values, it does not grow a theming engine.
 *
 * # Keys (all under the additive `prefs` block)
 *
 * - `appearance.theme`         string,  `crt-dark` (the only built-in palette)
 * - `appearance.tokens`        object,  optional per-token colour overrides
 * - `appearance.font_family`   string,  the `--font-mono` stack
 * - `appearance.font_size_px`  number,  13 (the `--fs-md` value)
 * - `appearance.ui_scale`      number,  1.0, clamped to 0.5..3.0
 * - `appearance.fullscreen`    boolean, false
 *
 * # How "live" works
 *
 * {@link applyAppearance} writes only the values that differ from the default:
 * a value left at its default has its inline custom property removed, so the
 * stylesheet in `tokens.css` stays authoritative and an unchanged install is
 * byte-for-byte the old look. Changing a value sets just that one property on
 * the document root, so the next paint uses it with no reload.
 */
import { readPref, type PrefsDocument } from "./prefs";

/** The one built-in palette, the late-90s dark CRT look the client shipped with. */
export const DEFAULT_THEME_ID = "crt-dark";

/** Every palette this build ships. Deliberately one: no theming engine. */
export const THEME_IDS = [DEFAULT_THEME_ID] as const;

/** A theme id this build can draw. */
export type ThemeId = (typeof THEME_IDS)[number];

/**
 * The exact colour tokens from `styles/tokens.css`.
 *
 * `appearance.test.ts` reads `tokens.css` off disk and asserts every entry
 * matches, so a stylesheet edit that drifts from this map fails loudly instead
 * of silently changing the default look.
 */
export const CRT_DARK_TOKENS: Readonly<Record<string, string>> = {
  "bg-void": "#05070a",
  "bg-app": "#090d12",
  "surface-1": "#0e141b",
  "surface-2": "#141c25",
  "surface-3": "#1b2530",
  "surface-4": "#233040",
  line: "#22303d",
  "line-strong": "#33475c",
  "line-glow": "rgba(255, 180, 84, 0.35)",
  "text-0": "#e8eef4",
  "text-1": "#a5b4c3",
  "text-2": "#6d7f90",
  "text-3": "#4c5b6a",
  amber: "#ffb454",
  "amber-dim": "#a9762f",
  "amber-soft": "rgba(255, 180, 84, 0.12)",
  cyan: "#5ccfe6",
  "cyan-soft": "rgba(92, 207, 230, 0.12)",
  green: "#7fd962",
  red: "#ff6b6b",
  violet: "#c099ff",
};

/**
 * The colour tokens the group lets you edit.
 *
 * The full palette above defines the built-in theme; only these solid accents
 * are offered as overrides, because they are single opaque colours a colour
 * picker can represent. The soft/dim companions stay at the theme's values.
 */
export const EDITABLE_COLOR_TOKENS = ["amber", "cyan", "green", "red", "violet"] as const;

/** One editable accent token's name, without the leading `--`. */
export type EditableColorToken = (typeof EDITABLE_COLOR_TOKENS)[number];

/** The `--font-mono` stack, exactly as `tokens.css` declares it. */
export const DEFAULT_FONT_FAMILY =
  '"Hack", "JetBrainsMono NF", "DejaVu Sans Mono", "Liberation Mono", monospace';

/** The `--fs-md` body size in pixels, exactly as `tokens.css` declares it. */
export const DEFAULT_FONT_SIZE_PX = 13;

/** The size range the group accepts, so a stray value cannot blank the interface. */
export const MIN_FONT_SIZE_PX = 9;
export const MAX_FONT_SIZE_PX = 24;

/** The interface-scale range, matching `MIN_UI_SCALE`/`MAX_UI_SCALE` in `commands.rs`. */
export const MIN_UI_SCALE = 0.5;
export const MAX_UI_SCALE = 3;
export const DEFAULT_UI_SCALE = 1;

/** The CSS custom property a token name maps to. */
export function tokenProperty(token: string): string {
  return `--${token}`;
}

/** The CSS custom property for the interface font stack. */
export const FONT_FAMILY_PROPERTY = "--font-mono";

/** The CSS custom property for the base body size. */
export const FONT_SIZE_PROPERTY = "--fs-md";

/** The appearance settings, resolved against the defaults. */
export interface Appearance {
  theme: ThemeId;
  fontFamily: string;
  fontSizePx: number;
  uiScale: number;
  fullscreen: boolean;
  /** Only the overrides that differ from the built-in palette. */
  tokens: Partial<Record<EditableColorToken, string>>;
}

/** A 3, 4, 6 or 8 digit CSS hex colour. */
const HEX_COLOUR = /^#[0-9a-fA-F]{3,8}$/;

/** Whether `value` names a palette this build ships. */
export function isThemeId(value: unknown): value is ThemeId {
  return typeof value === "string" && (THEME_IDS as readonly string[]).includes(value);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

/** A finite number from a stored value, or `null` when it is not one. */
function numberOrNull(value: unknown): number | null {
  if (typeof value === "number" && Number.isFinite(value)) {
    return value;
  }
  if (typeof value === "string" && value.trim() !== "") {
    const parsed = Number(value);
    return Number.isFinite(parsed) ? parsed : null;
  }
  return null;
}

/** The editable colour overrides stored under `appearance.tokens`, validated. */
export function tokenOverridesFrom(values: PrefsDocument): Record<string, string> {
  const raw = readPref<unknown>(values, ["appearance", "tokens"], null);
  if (!isRecord(raw)) {
    return {};
  }
  const tokens: Record<string, string> = {};
  for (const token of EDITABLE_COLOR_TOKENS) {
    const value = raw[token];
    if (typeof value === "string" && HEX_COLOUR.test(value)) {
      tokens[token] = value;
    }
  }
  return tokens;
}

/** Resolve the stored appearance block, falling back to the shipped defaults. */
export function appearanceFrom(values: PrefsDocument): Appearance {
  const rawTheme = readPref<unknown>(values, ["appearance", "theme"], DEFAULT_THEME_ID);
  const theme: ThemeId = isThemeId(rawTheme) ? rawTheme : DEFAULT_THEME_ID;

  const rawFamily = readPref<unknown>(values, ["appearance", "font_family"], DEFAULT_FONT_FAMILY);
  const fontFamily =
    typeof rawFamily === "string" && rawFamily.trim() !== ""
      ? rawFamily.trim()
      : DEFAULT_FONT_FAMILY;

  const rawSize = numberOrNull(readPref<unknown>(values, ["appearance", "font_size_px"], null));
  const fontSizePx =
    rawSize === null
      ? DEFAULT_FONT_SIZE_PX
      : Math.round(clamp(rawSize, MIN_FONT_SIZE_PX, MAX_FONT_SIZE_PX));

  const rawScale = numberOrNull(readPref<unknown>(values, ["appearance", "ui_scale"], null));
  const uiScale =
    rawScale === null
      ? DEFAULT_UI_SCALE
      : Math.round(clamp(rawScale, MIN_UI_SCALE, MAX_UI_SCALE) * 100) / 100;

  const fullscreen = readPref<unknown>(values, ["appearance", "fullscreen"], false) === true;

  return { theme, fontFamily, fontSizePx, uiScale, fullscreen, tokens: tokenOverridesFrom(values) };
}

/** The palette for a theme id. The only built-in is the dark CRT palette. */
export function themeTokens(theme: ThemeId): Readonly<Record<string, string>> {
  return theme === DEFAULT_THEME_ID ? CRT_DARK_TOKENS : {};
}

/** The overrides that actually differ from the built-in palette. */
export function effectiveOverrides(appearance: Appearance): Record<string, string> {
  const overrides: Record<string, string> = {};
  for (const token of EDITABLE_COLOR_TOKENS) {
    const value = appearance.tokens[token];
    if (value !== undefined && value.toLowerCase() !== CRT_DARK_TOKENS[token]?.toLowerCase()) {
      overrides[token] = value;
    }
  }
  return overrides;
}

/** A style surface `applyAppearance` can write to: the document root in the app. */
export interface StyleTarget {
  style: { setProperty(name: string, value: string): void; removeProperty(name: string): void };
}

/**
 * Apply an appearance to `target` (the document root by default).
 *
 * Every value at its default has its inline property removed, so the
 * stylesheet's own value wins and the default look is untouched. Only values
 * the user actually changed become inline custom properties, which is what
 * makes a change visible on the next paint without a reload.
 */
export function applyAppearance(
  appearance: Appearance,
  target: StyleTarget | null = typeof document === "undefined" ? null : document.documentElement,
): void {
  if (target === null) {
    return;
  }
  const overrides = effectiveOverrides(appearance);
  for (const token of EDITABLE_COLOR_TOKENS) {
    const property = tokenProperty(token);
    const value = overrides[token];
    if (value === undefined) {
      target.style.removeProperty(property);
    } else {
      target.style.setProperty(property, value);
    }
  }

  if (appearance.fontFamily === DEFAULT_FONT_FAMILY) {
    target.style.removeProperty(FONT_FAMILY_PROPERTY);
  } else {
    target.style.setProperty(FONT_FAMILY_PROPERTY, appearance.fontFamily);
  }

  if (appearance.fontSizePx === DEFAULT_FONT_SIZE_PX) {
    target.style.removeProperty(FONT_SIZE_PROPERTY);
  } else {
    target.style.setProperty(FONT_SIZE_PROPERTY, `${appearance.fontSizePx}px`);
  }
}

/** Remove every property {@link applyAppearance} owns, restoring `tokens.css`. */
export function clearAppearance(target: StyleTarget | null): void {
  if (target === null) {
    return;
  }
  for (const token of EDITABLE_COLOR_TOKENS) {
    target.style.removeProperty(tokenProperty(token));
  }
  target.style.removeProperty(FONT_FAMILY_PROPERTY);
  target.style.removeProperty(FONT_SIZE_PROPERTY);
}
