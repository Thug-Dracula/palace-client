import type { EditorSaveOptions, EditorSavePlacement } from "../../api";

/**
 * The "Save to My Bag" form, lifted out of the Frames panel so the editor
 * shell can trigger a save from anywhere (Ctrl+S) and so the request-building
 * rule can be unit-tested without a DOM.
 */
export interface SaveForm {
  placement: EditorSavePlacement;
  name: string;
  propIdText: string;
  head: boolean;
  ghost: boolean;
  rare: boolean;
  bounce: boolean;
  animate: boolean;
  hOffset: number;
  vOffset: number;
}

/** A fresh save form: append to the bag, no explicit name or id. */
export function defaultSaveForm(): SaveForm {
  return {
    placement: "new_prop_at_end",
    name: "",
    propIdText: "",
    head: false,
    ghost: false,
    rare: false,
    bounce: false,
    animate: false,
    hOffset: 0,
    vOffset: 0,
  };
}

/**
 * Turn the form into an API request: blank name/id become `null` so Rust
 * allocates them, and an unparseable id is treated as blank rather than NaN.
 */
export function toSaveOptions(form: SaveForm): EditorSaveOptions {
  const name = form.name.trim();
  const propIdText = form.propIdText.trim();
  const propId = propIdText === "" ? null : Number(propIdText);
  return {
    placement: form.placement,
    name: name === "" ? null : name,
    propId: propId !== null && Number.isFinite(propId) ? propId : null,
    head: form.head,
    ghost: form.ghost,
    rare: form.rare,
    bounce: form.bounce,
    animate: form.animate,
    hOffset: form.hOffset,
    vOffset: form.vOffset,
  };
}

/** Animated save-back is gated until the animated-prop task lands. */
export function isAnimatedSaveBlocked(frameCount: number): boolean {
  return frameCount > 1;
}
