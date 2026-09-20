/**
 * Picking the prop under a point, and phrasing what a gather did.
 *
 * Pure room-coordinate math: no DOM, no Svelte, no Tauri. The viewport owns the
 * screen point (already relative to its own box); this module maps that point
 * back onto the avatar sprite layers the roster already carries.
 *
 * The sprite geometry is deliberately identical to [`layoutAvatars`], which is
 * what the viewport drew:
 *
 *   css_x = content_x + (avatar.x + part.dx) * scale
 *   css_y = content_y + (avatar.y + part.dy) * scale
 *   w     = part.w * scale
 *   h     = part.h * scale
 *
 * A hit therefore lands on the same pixels the user can see.
 */

import type { AvatarRoster, GatherResult } from "./api";

/** One prop the roster can prove is under the cursor. */
export interface PickedProp {
  /** The avatar whose artwork carries the prop. */
  avatarId: number;
  /** That avatar's display name, for menus and messages. */
  avatarName: string;
  /** The id the protocol wears and gathers by. */
  id: number;
  /**
   * The `(id, crc)` pair's crc when the click-time data carries one, otherwise
   * null. Worn rosters key a prop by id alone, so this is null for every prop
   * the viewport can pick today; `gather_prop` then derives the crc from the
   * prop's bytes, which is the authoritative value.
   */
  crc: number | null;
}

/** A point in the viewport's own CSS-pixel space. */
export interface PickPoint {
  x: number;
  y: number;
}

/** The human label for a prop id; used by the menu and the feedback line. */
export const propLabel = (id: number): string => `Prop #${id}`;

/** How a requested wear of the gathered prop actually went. */
export type WearResult =
  | { state: "worn" }
  | { state: "already" }
  | { state: "refused"; error: string };

/** What a Gather & Wear request should do once the gather itself succeeded. */
export type WearPlan =
  | { action: "none" }
  | { action: "already" }
  | { action: "wear" };

/**
 * Decide the wear half of a Gather & Wear.
 *
 * The store's prop path toggles, so asking it to wear something already on the
 * avatar would take that prop off. A prop already worn is therefore reported as
 * `already` and left untouched; only a prop that is off is actually worn.
 */
export const planWear = (andWear: boolean, alreadyWorn: boolean): WearPlan => {
  if (!andWear) {
    return { action: "none" };
  }
  return alreadyWorn ? { action: "already" } : { action: "wear" };
};

const inside = (
  point: PickPoint,
  left: number,
  top: number,
  width: number,
  height: number,
): boolean =>
  point.x >= left && point.x < left + width && point.y >= top && point.y < top + height;

/**
 * The topmost prop sprite under `point`, or null when the point misses every
 * prop.
 *
 * Drawn order is the roster's order, so a later avatar covers an earlier one
 * and a later part covers an earlier part. Walking both lists backwards
 * therefore answers with what the user actually sees, and a face layer is
 * never mistaken for a prop.
 */
export const pickPropAt = (roster: AvatarRoster, point: PickPoint): PickedProp | null => {
  const g = roster.geometry;
  for (let a = roster.avatars.length - 1; a >= 0; a -= 1) {
    const avatar = roster.avatars[a];
    for (let p = avatar.parts.length - 1; p >= 0; p -= 1) {
      const part = avatar.parts[p];
      if (part.art.kind !== "prop" || part.alpha <= 0) {
        continue;
      }
      const left = g.content_x + (avatar.x + part.dx) * g.scale;
      const top = g.content_y + (avatar.y + part.dy) * g.scale;
      if (inside(point, left, top, part.w * g.scale, part.h * g.scale)) {
        return {
          avatarId: avatar.id,
          avatarName: avatar.name,
          id: part.art.id,
          crc: null,
        };
      }
    }
  }
  return null;
};

/**
 * One sentence describing a gather, and the optional wear that followed it.
 *
 * The three gather outcomes each get their own phrasing, and a wear is folded
 * in without ever claiming a wear that did not happen. No em dashes, matching
 * the house copy rule.
 */
export const formatGatherOutcome = (
  result: GatherResult,
  label: string,
  wear: WearResult | null = null,
): string => {
  if (result.outcome === "rejected") {
    return `Could not gather ${label}: ${result.reason}`;
  }
  const gathered =
    result.outcome === "added"
      ? `Gathered ${label} into My Bag`
      : `${label} was already in My Bag`;
  if (!wear) {
    return `${gathered}.`;
  }
  if (wear.state === "already") {
    return `${gathered}; it is already worn.`;
  }
  if (wear.state === "refused") {
    return `${gathered}, but it could not be worn: ${wear.error}`;
  }
  return `${gathered}, and wore it.`;
};
