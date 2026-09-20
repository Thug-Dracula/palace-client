/**
 * Avatar sprite layout — pure CSS-pixel math. No DOM, no Svelte.
 *
 * Room coordinates map to CSS pixels exactly like the rest of the client:
 *   css_x = content_x + room_x * scale
 *   css_y = content_y + room_y * scale
 * Values are returned unrounded; the display layer decides pixel snapping.
 */

import {
  faceCellUrl,
  propImageUrl,
  type1AvatarUrl,
  type AvatarArt,
  type AvatarRoster,
} from "./api";

/** One avatar artwork layer positioned in CSS pixels. */
export interface PositionedSprite {
  key: string;
  url: string;
  left: number;
  top: number;
  width: number;
  height: number;
  alpha: number;
}

/**
 * One avatar name tag.
 *
 * `left` is the avatar's horizontal anchor, which is the tag's centre — the
 * display is expected to translate the tag by -50% on X. `top` sits just above
 * the avatar's topmost artwork layer, so the tag never covers the sprite.
 */
export interface PositionedName {
  key: string;
  name: string;
  left: number;
  top: number;
}

/** Vertical gap between a name tag and the topmost sprite, in CSS pixels. */
export const NAME_TAG_GAP = 4;

export interface AvatarLayout {
  sprites: PositionedSprite[];
  names: PositionedName[];
}

const artUrl = (art: AvatarArt): string => {
  switch (art.kind) {
    case "face":
      return faceCellUrl(art.face, art.color);
    case "prop":
      return propImageUrl(art.id);
    case "type1":
      return type1AvatarUrl(art.hash);
  }
};

export const layoutAvatars = (roster: AvatarRoster): AvatarLayout => {
  const g = roster.geometry;
  const sprites: PositionedSprite[] = [];
  const names: PositionedName[] = [];

  for (const avatar of roster.avatars) {
    avatar.parts.forEach((part, index) => {
      sprites.push({
        key: `${avatar.id}:part:${index}`,
        url: artUrl(part.art),
        left: g.content_x + (avatar.x + part.dx) * g.scale,
        top: g.content_y + (avatar.y + part.dy) * g.scale,
        width: part.w * g.scale,
        height: part.h * g.scale,
        alpha: part.alpha,
      });
    });

    if (roster.name_tags_visible && avatar.name.trim().length > 0) {
      const topmostDy = avatar.parts.length
        ? Math.min(...avatar.parts.map((part) => part.dy))
        : 0;
      names.push({
        key: `${avatar.id}:name`,
        name: avatar.name,
        left: g.content_x + avatar.x * g.scale,
        top: g.content_y + (avatar.y + topmostDy) * g.scale - NAME_TAG_GAP,
      });
    }
  }

  return { sprites, names };
};
