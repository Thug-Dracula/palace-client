import { describe, expect, it } from "vitest";

import {
  faceCellUrl,
  propImageUrl,
  type1AvatarUrl,
  type AvatarRoster,
  type AvatarState,
  type ViewGeometry,
} from "./api";
import { layoutAvatars, NAME_TAG_GAP } from "./avatarLayout";

const geometry = (over: Partial<ViewGeometry> = {}): ViewGeometry => ({
  room_w: 100,
  room_h: 80,
  viewport_w: 400,
  viewport_h: 320,
  dpr: 1,
  zoom: 1,
  native: true,
  scale: 2,
  content_x: 10,
  content_y: 20,
  content_w: 200,
  content_h: 160,
  bitmap_w: 100,
  bitmap_h: 80,
  ...over,
});

const avatar = (over: Partial<AvatarState> = {}): AvatarState => ({
  id: 1,
  name: "",
  x: 0,
  y: 0,
  face: 0,
  color: 0,
  is_self: false,
  away: false,
  avatar_type: 0,
  parts: [],
  ...over,
});

const roster = (over: Partial<AvatarRoster> = {}): AvatarRoster => ({
  version: 1,
  room_id: 1,
  geometry: geometry(),
  name_tags_visible: false,
  avatars: [],
  ...over,
});

describe("layoutAvatars", () => {
  it("positions a face-only avatar at the expected left/top/size", () => {
    const result = layoutAvatars(
      roster({
        avatars: [
          avatar({
            id: 7,
            name: "Ann",
            x: 5,
            y: 6,
            parts: [
              { art: { kind: "face", face: 3, color: 4 }, dx: 1, dy: 2, alpha: 0.5, w: 3, h: 4 },
            ],
          }),
        ],
      }),
    );

    expect(result.sprites).toHaveLength(1);
    expect(result.sprites[0]).toEqual({
      key: "7:part:0",
      url: faceCellUrl(3, 4),
      left: 22,
      top: 36,
      width: 6,
      height: 8,
      alpha: 0.5,
    });
  });

  it("offsets a prop part by its dx/dy", () => {
    const result = layoutAvatars(
      roster({
        avatars: [
          avatar({
            id: 9,
            x: 0,
            y: 0,
            parts: [
              { art: { kind: "prop", id: 42 }, dx: 2, dy: -1, alpha: 1, w: 5, h: 6 },
            ],
          }),
        ],
      }),
    );

    expect(result.sprites).toHaveLength(1);
    expect(result.sprites[0]).toEqual({
      key: "9:part:0",
      url: propImageUrl(42),
      left: 14,
      top: 18,
      width: 10,
      height: 12,
      alpha: 1,
    });
  });

  it("serves a Type 1 avatar from its hash, not a prop tile", () => {
    const hash = "a9993e364706816aba3e25717850c26c9cd0d89d";
    const result = layoutAvatars(
      roster({
        avatars: [
          avatar({
            id: 12,
            avatar_type: 1,
            parts: [
              { art: { kind: "type1", hash }, dx: -66, dy: -66, alpha: 1, w: 132, h: 132 },
            ],
          }),
        ],
      }),
    );

    expect(result.sprites).toHaveLength(1);
    expect(result.sprites[0].url).toBe(type1AvatarUrl(hash));
  });

  it("emits only the prop sprite for a head-prop avatar with no face part", () => {
    const result = layoutAvatars(
      roster({
        avatars: [
          avatar({
            id: 11,
            parts: [{ art: { kind: "prop", id: 8 }, dx: 0, dy: 0, alpha: 1, w: 2, h: 2 }],
          }),
        ],
      }),
    );

    expect(result.sprites).toHaveLength(1);
    expect(result.sprites[0].url).toBe(propImageUrl(8));
    expect(result.sprites.some((sprite) => sprite.url.includes("face-cell"))).toBe(false);
  });

  it("emits name tags only when visible and the name is non-empty", () => {
    const parts = [
      { art: { kind: "face" as const, face: 1, color: 2 }, dx: 0, dy: 2, alpha: 1, w: 3, h: 4 },
    ];

    const hidden = layoutAvatars(
      roster({
        name_tags_visible: false,
        avatars: [avatar({ id: 1, name: "Ann", x: 5, y: 6, parts })],
      }),
    );
    expect(hidden.names).toHaveLength(0);

    const empty = layoutAvatars(
      roster({
        name_tags_visible: true,
        avatars: [avatar({ id: 1, name: "", x: 5, y: 6, parts })],
      }),
    );
    expect(empty.names).toHaveLength(0);

    const shown = layoutAvatars(
      roster({
        name_tags_visible: true,
        avatars: [avatar({ id: 1, name: "Ann", x: 5, y: 6, parts })],
      }),
    );
    expect(shown.names).toEqual([
      {
        key: "1:name",
        name: "Ann",
        left: 20,
        top: 36 - NAME_TAG_GAP,
      },
    ]);
  });

  it("preserves roster order for sprites and names", () => {
    const makeAvatar = (id: number, name: string, face: number): AvatarState =>
      avatar({
        id,
        name,
        parts: [
          { art: { kind: "face", face, color: 0 }, dx: 0, dy: 0, alpha: 1, w: 1, h: 1 },
        ],
      });

    const result = layoutAvatars(
      roster({
        name_tags_visible: true,
        avatars: [makeAvatar(2, "Second", 5), makeAvatar(3, "Third", 6)],
      }),
    );

    expect(result.sprites.map((sprite) => sprite.key)).toEqual(["2:part:0", "3:part:0"]);
    expect(result.names.map((name) => name.key)).toEqual(["2:name", "3:name"]);
  });
});
