import { describe, expect, it } from "vitest";

import type {
  AvatarPartState,
  AvatarRoster,
  AvatarState,
  GatherResult,
  ViewGeometry,
} from "./api";
import { formatGatherOutcome, pickPropAt, planWear, propLabel } from "./propPick";

/**
 * The geometry used by the existing avatar tests: content at (10, 20), scale 2.
 * An avatar at (5, 6) with a part dx/dy of (1, 2) therefore starts at (22, 36)
 * and, with a 3x4 source part, covers 22..28 by 36..44.
 */
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

const propPart = (id: number, over: Partial<AvatarPartState> = {}): AvatarPartState => ({
  art: { kind: "prop", id },
  dx: 1,
  dy: 2,
  alpha: 1,
  w: 3,
  h: 4,
  ...over,
});

const facePart = (): AvatarPartState => ({
  art: { kind: "face", face: 3, color: 4 },
  dx: 1,
  dy: 2,
  alpha: 1,
  w: 3,
  h: 4,
});

const avatar = (over: Partial<AvatarState> = {}): AvatarState => ({
  id: 1,
  name: "Ann",
  x: 5,
  y: 6,
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

describe("pickPropAt", () => {
  it("picks a worn prop under the point and reports its id with no crc", () => {
    const picked = pickPropAt(
      roster({ avatars: [avatar({ id: 9, name: "Bea", parts: [propPart(42)] })] }),
      { x: 25, y: 40 },
    );

    expect(picked).toEqual({ avatarId: 9, avatarName: "Bea", id: 42, crc: null });
  });

  it("misses when the point is outside the sprite", () => {
    const room = roster({ avatars: [avatar({ parts: [propPart(42)] })] });
    expect(pickPropAt(room, { x: 21, y: 40 })).toBeNull();
    expect(pickPropAt(room, { x: 28, y: 40 })).toBeNull();
    expect(pickPropAt(room, { x: 25, y: 35 })).toBeNull();
    expect(pickPropAt(room, { x: 25, y: 44 })).toBeNull();
  });

  it("treats the sprite's top-left corner as inside and the far edge as outside", () => {
    const room = roster({ avatars: [avatar({ parts: [propPart(42)] })] });
    expect(pickPropAt(room, { x: 22, y: 36 })?.id).toBe(42);
    expect(pickPropAt(room, { x: 27.999, y: 43.999 })?.id).toBe(42);
    expect(pickPropAt(room, { x: 28, y: 44 })).toBeNull();
  });

  it("never mistakes a face layer for a prop", () => {
    const room = roster({ avatars: [avatar({ parts: [facePart()] })] });
    expect(pickPropAt(room, { x: 25, y: 40 })).toBeNull();
  });

  it("skips a fully transparent prop but keeps a partly transparent one", () => {
    const invisible = roster({ avatars: [avatar({ parts: [propPart(1, { alpha: 0 })] })] });
    expect(pickPropAt(invisible, { x: 25, y: 40 })).toBeNull();

    const faded = roster({ avatars: [avatar({ parts: [propPart(1, { alpha: 0.4 })] })] });
    expect(pickPropAt(faded, { x: 25, y: 40 })?.id).toBe(1);
  });

  it("answers with the later part when two props overlap", () => {
    const room = roster({
      avatars: [avatar({ parts: [propPart(1), propPart(2, { dx: 0, dy: 0 })] })],
    });
    expect(pickPropAt(room, { x: 24, y: 38 })?.id).toBe(2);
  });

  it("answers with the later avatar when two avatars overlap", () => {
    const room = roster({
      avatars: [
        avatar({ id: 1, name: "Under", x: 0, y: 0, parts: [propPart(10, { dx: 0, dy: 0 })] }),
        avatar({ id: 2, name: "Over", x: 0, y: 0, parts: [propPart(20, { dx: 0, dy: 0 })] }),
      ],
    });
    const picked = pickPropAt(room, { x: 12, y: 24 });
    expect(picked?.avatarId).toBe(2);
    expect(picked?.avatarName).toBe("Over");
    expect(picked?.id).toBe(20);
  });

  it("returns null when the roster has no props at all", () => {
    expect(pickPropAt(roster(), { x: 25, y: 40 })).toBeNull();
    expect(pickPropAt(roster({ avatars: [avatar({ parts: [] })] }), { x: 25, y: 40 })).toBeNull();
  });
});

describe("propLabel", () => {
  it("names a prop by its id", () => {
    expect(propLabel(42)).toBe("Prop #42");
  });
});

describe("planWear", () => {
  it("does nothing for a plain Gather", () => {
    expect(planWear(false, false)).toEqual({ action: "none" });
    expect(planWear(false, true)).toEqual({ action: "none" });
  });

  it("wears a prop that is off", () => {
    expect(planWear(true, false)).toEqual({ action: "wear" });
  });

  it("leaves a prop that is already worn alone instead of toggling it off", () => {
    expect(planWear(true, true)).toEqual({ action: "already" });
  });
});

describe("formatGatherOutcome", () => {
  it("phrases a plain add", () => {
    expect(formatGatherOutcome({ outcome: "added" }, "Prop #7")).toBe(
      "Gathered Prop #7 into My Bag.",
    );
  });

  it("phrases an already-present gather", () => {
    expect(formatGatherOutcome({ outcome: "already_present" }, "Prop #7")).toBe(
      "Prop #7 was already in My Bag.",
    );
  });

  it("carries the rejection reason through", () => {
    const result: GatherResult = {
      outcome: "rejected",
      reason: "prop 7 is not in the live asset intake",
    };
    expect(formatGatherOutcome(result, "Prop #7")).toBe(
      "Could not gather Prop #7: prop 7 is not in the live asset intake",
    );
  });

  it("says when the gathered prop was then worn", () => {
    expect(formatGatherOutcome({ outcome: "added" }, "Prop #7", { state: "worn" })).toBe(
      "Gathered Prop #7 into My Bag, and wore it.",
    );
  });

  it("says when the prop was already worn", () => {
    expect(formatGatherOutcome({ outcome: "added" }, "Prop #7", { state: "already" })).toBe(
      "Gathered Prop #7 into My Bag; it is already worn.",
    );
  });

  it("reports a wear refusal without losing the gather result", () => {
    expect(
      formatGatherOutcome({ outcome: "already_present" }, "Prop #7", {
        state: "refused",
        error: "You can wear at most 9 props at once",
      }),
    ).toBe(
      "Prop #7 was already in My Bag, but it could not be worn: You can wear at most 9 props at once",
    );
  });

  it("never mentions a wear on a rejected gather", () => {
    const message = formatGatherOutcome(
      { outcome: "rejected", reason: "too short" },
      "Prop #7",
      { state: "worn" },
    );
    expect(message).toBe("Could not gather Prop #7: too short");
  });
});
