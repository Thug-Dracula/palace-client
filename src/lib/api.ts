import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import { internalBaseUrl } from "./internalUrl";

export const EVENT_NAME = "palace://event";

export type ConnectionStatus = "disconnected" | "connecting" | "connected" | "error";

export interface Settings {
  host: string;
  port: number;
  username: string;
  soundfont: string | null;
}

export interface AudioState {
  enabled: boolean;
  volume: number;
  soundfont: string | null;
  soundfont_exists: boolean;
}

export interface ServerBanner {
  host: string;
  port: number;
  byte_order: string;
  user_id: number;
  version: string | null;
  name: string | null;
  media_base: string | null;
  total_users: number | null;
}

export interface RoomInfo {
  id: number;
  name: string;
  users: number;
  flags: number;
}

export interface UserInfo {
  id: number;
  name: string;
  face: number;
  color: number;
  room_id: number;
  x: number;
  y: number;
  props: number[];
  away: boolean;
  is_self: boolean;
  /** 0 = classic prop avatar, 1 = Type 1 single-image avatar. */
  avatar_type: number;
  /** Type 1 `AF_*` flags. */
  avatar_flags: number;
  /** The Type 1 avatar's 20-byte content hash as lowercase hex, when it has one. */
  avatar_hash: string | null;
}

export type ChatKind = "talk" | "whisper" | "system" | "error";

export interface ChatLine {
  seq: number;
  user_id: number;
  name: string;
  text: string;
  kind: ChatKind;
}

export interface ViewGeometry {
  room_w: number;
  room_h: number;
  viewport_w: number;
  viewport_h: number;
  dpr: number;
  zoom: number;
  native: boolean;
  scale: number;
  content_x: number;
  content_y: number;
  content_w: number;
  content_h: number;
  bitmap_w: number;
  bitmap_h: number;
}

export interface ScreenState {
  version: number;
  room_id: number;
  room_name: string;
  avatars: number;
  loose_props: number;
  props_pending: number;
  notes: string[];
  geometry: ViewGeometry;
  /** Board middle layer version; null/absent when that layer is empty. */
  mid_version?: number | null;
  /** Board top layer version; null/absent when that layer is empty. */
  top_version?: number | null;
}

/** A tagged reference to one piece of avatar artwork: a face, a prop, or a Type 1 image. */
export type AvatarArt =
  | { kind: "face"; face: number; color: number }
  | { kind: "prop"; id: number }
  | { kind: "type1"; hash: string };

/** One artwork layer of an avatar, positioned relative to its anchor. */
export interface AvatarPartState {
  art: AvatarArt;
  dx: number;
  dy: number;
  alpha: number;
  w: number;
  h: number;
}

/** One avatar's identity, position and sprite layers. */
export interface AvatarState {
  id: number;
  name: string;
  x: number;
  y: number;
  face: number;
  color: number;
  is_self: boolean;
  away: boolean;
  /** 0 = classic prop avatar, 1 = Type 1 single-image avatar. */
  avatar_type: number;
  parts: AvatarPartState[];
}

/** The avatar roster payload the webview sprite layer draws. */
export interface AvatarRoster {
  version: number;
  room_id: number;
  geometry: ViewGeometry;
  name_tags_visible: boolean;
  avatars: AvatarState[];
}

export interface ScriptReport {
  event: string;
  fired: number;
  effects: string[];
  problems: string[];
}

export type ClientEvent =
  | { type: "status"; status: ConnectionStatus; message: string | null }
  | { type: "banner"; banner: ServerBanner }
  | { type: "rooms"; rooms: RoomInfo[] }
  | { type: "users"; users: UserInfo[] }
  | { type: "room_entered"; room: RoomInfo }
  | { type: "chat"; line: ChatLine }
  | { type: "screen"; screen: ScreenState }
  | { type: "avatars"; roster: AvatarRoster }
  | { type: "tooltip"; text: string | null }
  | {
      type: "script";
      event: string;
      fired: number;
      effects: string[];
      problems: string[];
    }
  | { type: "note"; text: string }
  | { type: "sound"; name: string }
  | { type: "midi_play"; name: string }
  | { type: "midi_loop"; name: string; loops: number }
  | { type: "midi_stop" }
  | { type: "beep" };

export const frameUrl = (version: number): string =>
  `${internalBaseUrl()}/frame?v=${version}`;

export const midUrl = (version: number): string =>
  `${internalBaseUrl()}/mid?v=${version}`;

export const topUrl = (version: number): string =>
  `${internalBaseUrl()}/top?v=${version}`;

/** A single worn prop's image. */
export const propImageUrl = (id: number): string =>
  `${internalBaseUrl()}/avatar-prop/${id}`;

/** One face cell (a specific face/color combination) out of the shared sheet. */
export const faceCellUrl = (face: number, color: number): string =>
  `${internalBaseUrl()}/face-cell/${face}/${color}`;

/**
 * One Type 1 avatar: a single server-hosted image, keyed by its content hash.
 * Distinct from `propImageUrl`, which serves a 44x44 prop tile.
 */
export const type1AvatarUrl = (hash: string): string =>
  `${internalBaseUrl()}/type1-avatar/${hash}`;

export const facesUrl = (): string => `${internalBaseUrl()}/faces`;

export interface FaceGrid {
  cell: number;
  faces: number;
  colors: number;
  rows: number[][];
}

export const facesGrid = async (): Promise<FaceGrid> => {
  const response = await fetch(`${internalBaseUrl()}/faces.json`);
  if (!response.ok) {
    throw new Error(`face grid: HTTP ${response.status}`);
  }
  return (await response.json()) as FaceGrid;
};

export interface PropEntry {
  id: number;
  crc: number;
  name?: string | null;
  w: number;
  h: number;
  flags: number;
  fav: boolean;
  trash: boolean;
}

export interface PropsCatalog {
  props: PropEntry[];
}

export const propsCatalog = async (): Promise<PropEntry[]> => {
  const response = await fetch(`${internalBaseUrl()}/props.json`);
  if (!response.ok) {
    throw new Error(`prop catalog: HTTP ${response.status}`);
  }
  const catalog = (await response.json()) as PropsCatalog;
  return Array.isArray(catalog.props) ? catalog.props : [];
};

export const propThumbUrl = (id: number): string => `${internalBaseUrl()}/prop/${id}`;

/** One collection the bag folder exposes. */
export interface BagCollectionInfo {
  path: string;
  name: string;
  writable: boolean;
  exists: boolean;
}

/** One shelf's health and location. */
export interface BagShelfInfo {
  name: string;
  path: string;
  status: string;
  records: number;
  named: number;
  undecodable: number;
  dropped: number;
  error: string | null;
}

/** A bag identity pair: an id alone is not unique, the crc completes it. */
export interface BagKey {
  id: number;
  crc: number;
}

/** One browsable bag prop; always bag-sourced, never the PalaceChat cache. */
export interface BagPropEntry {
  id: number;
  crc: number;
  name?: string | null;
  w: number;
  h: number;
  flags: number;
  fav: boolean;
  trash: boolean;
  collection: string;
  source: "bag";
}

/** One trashed prop. */
export interface BagTrashEntry {
  id: number;
  crc: number;
  name: string | null;
  bytes: number;
}

/** One saved outfit. */
export interface BagOutfit {
  name: string;
  props: BagKey[];
}

/** An outfit split into what the bag can wear and what it is missing. */
export interface BagOutfitApplication {
  worn: BagKey[];
  missing: BagKey[];
}

/** Whether a favourite toggle changed the stored list. */
export interface FavouriteResult {
  favourite: boolean;
  changed: boolean;
}

/** How many trashed props a purge discarded. */
export interface PurgeResult {
  purged: number;
}

/** What a bag mutation did. */
export type BagOutcome =
  | "added"
  | "already_present"
  | "removed"
  | "renamed"
  | "duplicated"
  | "moved"
  | "created"
  | "deleted"
  | "unchanged"
  | "not_found"
  | "already_exists";

/** What a gather attempt did. */
export type GatherResult =
  | { outcome: "added" }
  | { outcome: "already_present" }
  | { outcome: "rejected"; reason: string };

/** The bag catalog JSON route (bag-sourced props only). */
export const bagCatalogUrl = (): string => `${internalBaseUrl()}/bag-catalog.json`;

/** Every shelf's health JSON. */
export const bagShelvesUrl = (): string => `${internalBaseUrl()}/bag-shelves.json`;

/** One shelf's health JSON, by index into the shelf list. */
export const bagShelfUrl = (index: number): string => `${internalBaseUrl()}/bag-shelf/${index}`;

/** What rebuilding one prop's cached thumbnail did. */
export type ThumbnailRebuild = {
  id: number;
  crc: number;
  rebuilt: boolean;
  bytes: number;
};

/** How many cached thumbnails a "rebuild all" dropped. */
export type ThumbnailRebuildAll = { removed: number };

/**
 * One bag prop's thumbnail, keyed by its `(collection, id, crc)` identity.
 *
 * `version` is a cache-busting token: the protocol path is unchanged, but a new
 * value forces the webview to re-request the image after a rebuild.
 */
export const bagThumbUrl = (
  collection: string,
  id: number,
  crc: number,
  version?: number,
): string => {
  const base = `${internalBaseUrl()}/bag-prop/${encodeURIComponent(collection)}/${id}/${crc}`;
  return version === undefined ? base : `${base}?v=${version}`;
};

/** Forget and regenerate one bag prop's cached thumbnail. */
export const bagRebuildThumbnail = (
  id: number,
  crc: number,
): Promise<ThumbnailRebuild> => invoke("bag_rebuild_thumbnail", { id, crc });

/** Drop every cached bag thumbnail; they regenerate lazily as you browse. */
export const bagRebuildThumbnails = (): Promise<ThumbnailRebuildAll> =>
  invoke("bag_rebuild_thumbnails");

export const bagCollections = (): Promise<BagCollectionInfo[]> => invoke("bag_collections");

export const bagShelves = (): Promise<BagShelfInfo[]> => invoke("bag_shelves");

export const bagCatalog = (): Promise<BagPropEntry[]> => invoke("bag_catalog");

export const bagAdd = (
  id: number,
  crc: number,
  blob: number[],
  name: string | null = null,
): Promise<BagOutcome> => invoke("bag_add", { id, crc, blob, name });

export const bagRemove = (id: number, crc: number): Promise<BagOutcome> =>
  invoke("bag_remove", { id, crc });

export const bagMove = (from: string, to: string, id: number, crc: number): Promise<BagOutcome> =>
  invoke("bag_move", { from, to, id, crc });

export const bagDuplicate = (
  from: string,
  id: number,
  crc: number,
  name: string | null = null,
): Promise<BagOutcome> => invoke("bag_duplicate", { from, id, crc, name });

export const bagRename = (id: number, crc: number, name: string | null): Promise<BagOutcome> =>
  invoke("bag_rename", { id, crc, name });

export const bagCreateCollection = (): Promise<BagOutcome> => invoke("bag_create_collection");

export const bagDeleteCollection = (): Promise<BagOutcome> => invoke("bag_delete_collection");

export const bagFavourite = (
  id: number,
  crc: number,
  favourite: boolean,
): Promise<FavouriteResult> => invoke("bag_favourite", { id, crc, favourite });

export const bagTrash = (id: number, crc: number): Promise<BagOutcome> =>
  invoke("bag_trash", { id, crc });

export const bagTrashList = (): Promise<BagTrashEntry[]> => invoke("bag_trash_list");

export const bagTrashRestore = (id: number, crc: number): Promise<BagOutcome> =>
  invoke("bag_trash_restore", { id, crc });

export const bagTrashPurge = (): Promise<PurgeResult> => invoke("bag_trash_purge");

export const gatherProp = (
  id: number,
  crc: number | null = null,
  name: string | null = null,
): Promise<GatherResult> => invoke("gather_prop", { id, crc, name });

export const outfitsList = (): Promise<BagOutfit[]> => invoke("outfits_list");

export const outfitsSave = (name: string, props: BagKey[]): Promise<BagOutfit[]> =>
  invoke("outfits_save", { name, props });

export const outfitsApply = (name: string): Promise<BagOutfitApplication> =>
  invoke("outfits_apply", { name });

export const outfitsRename = (from: string, to: string): Promise<BagOutfit[]> =>
  invoke("outfits_rename", { from, to });

export const outfitsDelete = (name: string): Promise<BagOutfit[]> =>
  invoke("outfits_delete", { name });

export const outfitsDuplicate = (source: string, target: string): Promise<BagOutfit[]> =>
  invoke("outfits_duplicate", { source, target });

export const getSettings = (): Promise<Settings> => invoke("get_settings");

export const connect = (settings: Settings): Promise<void> =>
  invoke("connect", {
    host: settings.host,
    port: settings.port,
    username: settings.username,
  });

export const disconnect = (): Promise<void> => invoke("disconnect");

export const gotoRoom = (roomId: number): Promise<void> => invoke("goto_room", { roomId });

export const say = (text: string): Promise<void> => invoke("say", { text });

export const setViewport = (
  width: number,
  height: number,
  dpr: number,
  zoom: number,
  native: boolean,
): Promise<void> => invoke("set_viewport", { width, height, dpr, zoom, native });

export const setUiScale = (scale: number): Promise<number> => invoke("set_ui_scale", { scale });

export const refresh = (): Promise<void> => invoke("refresh");

export const click = (x: number, y: number): Promise<void> => invoke("click", { x, y });

export const mousemove = (x: number, y: number): Promise<void> => invoke("mousemove", { x, y });

export const mouseLeave = (): Promise<void> => invoke("mouse_leave");

export const setVisibility = (names: boolean, avatars: boolean): Promise<void> =>
  invoke("set_visibility", { names, avatars });

export const setAvatar = (face: number, color: number): Promise<void> =>
  invoke("set_avatar", { face, color });

export const setProps = (props: number[]): Promise<void> => invoke("set_props", { props });

/** The server's Type 1 avatar limits, from its `'AVAT'` reply. */
export interface Type1AvatarLimits {
  /** Permitted format bits: 1 GIF, 2 JPEG, 4 PNG. */
  formats: number;
  /** Maximum payload in kilobytes; 0 means unlimited. */
  max_payload_kb: number;
  /** Maximum height in pixels; 0 means unlimited. */
  max_height: number;
  /** Maximum width in pixels; 0 means unlimited. */
  max_width: number;
}

/** The server's Type 1 avatar limits, or null before its `'AVAT'` reply. */
export const type1AvatarLimits = (): Promise<Type1AvatarLimits | null> =>
  invoke("type1_avatar_limits");

/**
 * Set the signed-in user's Type 1 avatar from raw image bytes.
 *
 * Validated against the server's limits; the runtime reports a refusal as an
 * error chat line. The upload reaches the server only when uploads are enabled.
 */
export const setType1Avatar = (bytes: number[]): Promise<void> =>
  invoke("set_type1_avatar", { bytes });

/** Clear the signed-in user's Type 1 avatar, returning to a prop avatar. */
export const clearType1Avatar = (): Promise<void> => invoke("clear_type1_avatar");

export const getAudioState = (): Promise<AudioState> => invoke("get_audio_state");

export const setSoundfont = (path: string | null): Promise<void> =>
  invoke("set_soundfont", { path });

export const setAudioEnabled = (enabled: boolean): Promise<void> =>
  invoke("set_audio_enabled", { enabled });

export const setVolume = (volume: number): Promise<number> => invoke("set_volume", { volume });

export const onEvent = (handler: (event: ClientEvent) => void): Promise<UnlistenFn> =>
  listen<ClientEvent>(EVENT_NAME, (message) => handler(message.payload));

/** Where the open editor document came from: a blank canvas, a bag prop clone, or an imported image. */
export type EditorOrigin =
  | { kind: "blank" }
  | { kind: "bag_prop"; collection: string; id: number; crc: number; name: string | null }
  | { kind: "image" };

/** One layer of the selected frame, for the layer list. */
export interface EditorLayerSummary {
  id: number;
  name: string;
  width: number;
  height: number;
  opacity: number;
  visible: boolean;
  /** Horizontal placement of the layer's top-left, in frame pixels. */
  x: number;
  /** Vertical placement of the layer's top-left, in frame pixels. */
  y: number;
  /** Clockwise rotation in degrees about the layer centre. */
  rotation: number;
  /** Uniform scale; 1 is natural size. */
  scale: number;
}

/** One frame in the frame strip. */
export interface EditorFrameSummary {
  index: number;
  width: number;
  height: number;
  layer_count: number;
  selected: boolean;
}

/** The Rust-side editor session: dimensions, frames, layers and history. */
export interface EditorState {
  origin: EditorOrigin;
  width: number;
  height: number;
  frame_count: number;
  frame_index: number;
  frames: EditorFrameSummary[];
  layers: EditorLayerSummary[];
  can_undo: boolean;
  can_redo: boolean;
  /** A frame has been copied and can be pasted. */
  can_paste: boolean;
  undo_depth: number;
  redo_depth: number;
  undo_bytes: number;
  /** Unsaved edits: true once an undoable change exists. */
  dirty: boolean;
}

/** An editor operation's result: whether it changed anything, plus the new state. */
export interface EditorOutcome {
  changed: boolean;
  state: EditorState;
}

/** Open a blank transparent canvas. */
export const editorOpenBlank = (width: number, height: number): Promise<EditorState> =>
  invoke("editor_open_blank", { width, height });

/** Open a clone of an existing bag prop's pixels; the bag file itself is only read. */
export const editorOpenBagProp = (
  collection: string,
  id: number,
  crc: number,
): Promise<EditorState> => invoke("editor_open_bag_prop", { collection, id, crc });

/** Import an image file's bytes (PNG/JPG/BMP/GIF/WebP); Rust decodes them. */
export const editorOpenImage = (bytes: number[]): Promise<EditorState> =>
  invoke("editor_open_image", { bytes });

/** The open document's state, or null when the editor has nothing open. */
export const editorState = (): Promise<EditorState | null> => invoke("editor_state");

/** One frame's composited pixels as PNG bytes, for the canvas. */
export const editorFramePng = (frameIndex: number): Promise<number[]> =>
  invoke("editor_frame_png", { frameIndex });

export const editorUndo = (): Promise<EditorOutcome> => invoke("editor_undo");

export const editorRedo = (): Promise<EditorOutcome> => invoke("editor_redo");

export const editorAddFrame = (): Promise<EditorOutcome> => invoke("editor_add_frame");

export const editorDuplicateFrame = (index: number): Promise<EditorOutcome> =>
  invoke("editor_duplicate_frame", { index });

export const editorDeleteFrame = (index: number): Promise<EditorOutcome> =>
  invoke("editor_delete_frame", { index });

export const editorMoveFrame = (from: number, to: number): Promise<EditorOutcome> =>
  invoke("editor_move_frame", { from, to });

export const editorSelectFrame = (index: number): Promise<EditorOutcome> =>
  invoke("editor_select_frame", { index });

/* ------------------------------------------------------------------ */
/* Editor tools                                                        */
/* ------------------------------------------------------------------ */

/** How a saved prop is placed in My Bag. */
export type EditorSavePlacement =
  | "overwrite_in_place"
  | "new_prop_at_start"
  | "new_prop_at_end";

/** The resampling filter a resize or shape-crop uses. */
export type EditorResample = "nearest" | "bilinear";

/** Which axis a flip mirrors. */
export type EditorFlipAxis = "horizontal" | "vertical";

/** Which quarter turn a rotate performs. */
export type EditorQuarterTurn = "clockwise" | "counter_clockwise" | "half";

/** The mask a shape crop keeps. */
export type EditorShapeCropChoice =
  | { kind: "square" }
  | { kind: "circle" }
  | { kind: "rounded"; radius: number };

/** A brush or eraser stroke: endpoints, colour, stamp size and scope. */
export interface EditorStrokeOptions {
  fromX: number;
  fromY: number;
  toX: number;
  toY: number;
  /** RGBA, each channel 0-255. The brush forces alpha opaque. */
  color: number[];
  /** Stamp side in pixels, 1-48. */
  size: number;
  /** Layer index to paint on, 0 is the base. */
  layer: number;
  allFrames: boolean;
}

/** A flood-fill seed, colour and tolerance. */
export interface EditorFloodFillOptions {
  x: number;
  y: number;
  color: number[];
  /** 0.0-1.0. */
  tolerance: number;
  layer: number;
  allFrames: boolean;
}

/** Whether a frame-level operation touches the selected frame or all of them. */
export interface EditorFrameScope {
  allFrames: boolean;
}

/** A flip request. */
export interface EditorFlipOptions {
  axis: EditorFlipAxis;
  allFrames: boolean;
}

/** A rotate request. */
export interface EditorRotateOptions {
  turn: EditorQuarterTurn;
  allFrames: boolean;
}

/** A free crop rectangle. */
export interface EditorCropOptions {
  x: number;
  y: number;
  width: number;
  height: number;
  allFrames: boolean;
}

/** A shape crop: mask, zoom and rotation. */
export interface EditorShapeCropOptions {
  shape: EditorShapeCropChoice;
  zoom: number;
  rotation: number;
  resample: EditorResample;
  allFrames: boolean;
}

/** An image-resize request. */
export interface EditorResizeOptions {
  width: number;
  height: number;
  constrainAspect: boolean;
  resample: EditorResample;
  allFrames: boolean;
}

/** A brightness/contrast request; both are -1.0 to 1.0 with 0 the identity. */
export interface EditorBrightnessContrastOptions {
  brightness: number;
  contrast: number;
  allFrames: boolean;
}

/** A brightness/contrast/saturation request. */
export interface EditorColourAdjustOptions {
  brightness: number;
  contrast: number;
  saturation: number;
  allFrames: boolean;
}

/** A chroma-key background-removal request. */
export interface EditorRemoveBackgroundOptions {
  /** The background colour to cut, as [r, g, b]. */
  key: number[];
  /** Per-channel envelope, 0.0-1.0. */
  tolerance: number;
  /** Soft alpha ramp width, 0.0-1.0. */
  feather: number;
  allFrames: boolean;
}

/** Everything the save dialog sends to My Bag. */
export interface EditorSaveOptions {
  placement: EditorSavePlacement;
  name?: string | null;
  /** An explicit prop id for a new prop; omit to allocate by placement. */
  propId?: number | null;
  head: boolean;
  ghost: boolean;
  rare: boolean;
  bounce: boolean;
  animate: boolean;
  hOffset: number;
  vOffset: number;
}

/** The result of a save: where it landed and the new session state. */
export interface EditorSaveResult {
  outcome: BagOutcome;
  id: number;
  crc: number;
  name: string | null;
  message: string;
  state: EditorState;
}

/** Paint an opaque brush stroke. */
export const editorPaintStroke = (request: EditorStrokeOptions): Promise<EditorOutcome> =>
  invoke("editor_paint_stroke", { request });

/** Erase along a stroke (clear alpha). */
export const editorEraseStroke = (request: EditorStrokeOptions): Promise<EditorOutcome> =>
  invoke("editor_erase_stroke", { request });

/** Which tool a pointer drag stroke uses. */
export type EditorStrokeMode = "brush" | "erase";

/** One segment of a pointer drag stroke, as the begin/extend commands take it. */
export interface EditorStrokeSessionOptions extends EditorStrokeOptions {
  mode: EditorStrokeMode;
}

/** Begin a pointer drag stroke: its first segment and the drag's one undo step. */
export const editorBeginStroke = (
  request: EditorStrokeSessionOptions,
): Promise<EditorOutcome> => invoke("editor_begin_stroke", { request });

/** Extend an in-progress pointer drag stroke; shares the begin's undo step. */
export const editorExtendStroke = (
  request: EditorStrokeSessionOptions,
): Promise<EditorOutcome> => invoke("editor_extend_stroke", { request });

/** Flood-fill a region. */
export const editorFloodFill = (request: EditorFloodFillOptions): Promise<EditorOutcome> =>
  invoke("editor_flood_fill", { request });

/** Sample a layer pixel's colour; null when out of range. */
export const editorPickColor = (
  layer: number,
  x: number,
  y: number,
): Promise<number[] | null> => invoke("editor_pick_color", { layer, x, y });

/** Clear a frame's pixels to transparent. */
export const editorClearFrame = (request: EditorFrameScope): Promise<EditorOutcome> =>
  invoke("editor_clear_frame", { request });

/** Flip the selected frame, or every frame. */
export const editorFlip = (request: EditorFlipOptions): Promise<EditorOutcome> =>
  invoke("editor_flip", { request });

/** Rotate the selected frame, or every frame. */
export const editorRotate = (request: EditorRotateOptions): Promise<EditorOutcome> =>
  invoke("editor_rotate", { request });

/** Crop the selected frame, or every frame, to a rectangle. */
export const editorCrop = (request: EditorCropOptions): Promise<EditorOutcome> =>
  invoke("editor_crop", { request });

/** Crop the selected frame, or every frame, to its largest centred square. */
export const editorCropSquare = (request: EditorFrameScope): Promise<EditorOutcome> =>
  invoke("editor_crop_square", { request });

/** Shape-crop the selected frame, or every frame. */
export const editorShapeCrop = (request: EditorShapeCropOptions): Promise<EditorOutcome> =>
  invoke("editor_shape_crop", { request });

/** Resize the selected frame, or every frame. */
export const editorResize = (request: EditorResizeOptions): Promise<EditorOutcome> =>
  invoke("editor_resize", { request });

/** Apply brightness and contrast. */
export const editorAdjustBrightnessContrast = (
  request: EditorBrightnessContrastOptions,
): Promise<EditorOutcome> => invoke("editor_adjust_brightness_contrast", { request });

/** Apply brightness, contrast and saturation. */
export const editorAdjustColour = (
  request: EditorColourAdjustOptions,
): Promise<EditorOutcome> => invoke("editor_adjust_colour", { request });

/** Remove a chroma-key background. */
export const editorRemoveBackground = (
  request: EditorRemoveBackgroundOptions,
): Promise<EditorOutcome> => invoke("editor_remove_background", { request });

/** Sample a pixel's colour as a chroma key; null when it is transparent. */
export const editorSampleKeyColor = (
  layer: number,
  x: number,
  y: number,
): Promise<number[] | null> => invoke("editor_sample_key_color", { layer, x, y });

/** Copy a frame into the editor clipboard. */
export const editorCopyFrame = (index: number): Promise<EditorOutcome> =>
  invoke("editor_copy_frame", { index });

/** Paste the copied frame after the current frame. */
export const editorPasteFrame = (): Promise<EditorOutcome> => invoke("editor_paste_frame");

/** Save the current single-frame prop into My Bag. */
export const editorSaveProp = (request: EditorSaveOptions): Promise<EditorSaveResult> =>
  invoke("editor_save_prop", { request });

/** Export the current frame as PNG bytes. */
export const editorExportPng = (): Promise<number[]> => invoke("editor_export_png");

/** Export the document as WebP bytes (animated when multi-frame). */
export const editorExportWebp = (frameDurationMs = 100): Promise<number[]> =>
  invoke("editor_export_webp", { frameDurationMs });

/* ------------------------------------------------------------------ */
/* Editor text, overlays and layers                                    */
/* ------------------------------------------------------------------ */

/** The text tool's request: the string plus every draw parameter. */
export interface EditorTextOptions {
  text: string;
  /** Fill colour RGBA, each channel 0-255. */
  color: number[];
  /** Font size in pixels. */
  size: number;
  /** Layer top-left x, in frame pixels. */
  x: number;
  /** Layer top-left y, in frame pixels. */
  y: number;
  /** Clockwise rotation in degrees. */
  rotation: number;
  /** Draw opacity, 0-1. */
  opacity: number;
  /** Draw a dark drop shadow behind the glyphs. */
  shadow: boolean;
  /** Draw a contrasting outline around the glyphs. */
  outline: boolean;
  /** Wrap lines to the prop's width. */
  wrap: boolean;
}

/** A partial update to one layer, addressed by its index (base is 0). */
export interface EditorLayerUpdate {
  layer: number;
  name?: string;
  opacity?: number;
  visible?: boolean;
  x?: number;
  y?: number;
  rotation?: number;
  scale?: number;
}

/** Rasterize text and add it as a new layer. */
export const editorAddText = (request: EditorTextOptions): Promise<EditorOutcome> =>
  invoke("editor_add_text", { request });

/** Decode an image's bytes (PNG/JPG/BMP/GIF/WebP) and add it as an overlay layer. */
export const editorImportOverlay = (
  bytes: number[],
  name: string | null = null,
): Promise<EditorOutcome> => invoke("editor_import_overlay", { bytes, name });

/** Apply a partial update (name, opacity, visibility, transform) to one layer. */
export const editorUpdateLayer = (request: EditorLayerUpdate): Promise<EditorOutcome> =>
  invoke("editor_update_layer", { request });

/** Move a layer within the selected frame; the base layer stays at index 0. */
export const editorMoveLayer = (from: number, to: number): Promise<EditorOutcome> =>
  invoke("editor_move_layer", { from, to });

/** Delete an overlay or text layer; the base layer cannot be removed. */
export const editorDeleteLayer = (index: number): Promise<EditorOutcome> =>
  invoke("editor_delete_layer", { index });

/* ------------------------------------------------------------------ */
/* Editor guides and snapping                                          */
/* ------------------------------------------------------------------ */

/** A point in canvas pixels. */
export interface EditorGuidePoint {
  x: number;
  y: number;
}

/** A straight guide line, as its two endpoints. */
export interface EditorGuideLine {
  from: EditorGuidePoint;
  to: EditorGuidePoint;
}

/** The grid lines for the open canvas. */
export interface EditorGuideGrid {
  vertical: EditorGuideLine[];
  horizontal: EditorGuideLine[];
}

/** The centre cross for the open canvas. */
export interface EditorGuideCentre {
  vertical: EditorGuideLine;
  horizontal: EditorGuideLine;
}

/** The rule-of-thirds lines for the open canvas. */
export interface EditorGuideThirds {
  vertical: EditorGuideLine[];
  horizontal: EditorGuideLine[];
}

/** The safe-area rectangle for the open canvas. */
export interface EditorGuideRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** Every guide line for the open canvas; pure geometry, never pixels. */
export interface EditorGuideGeometry {
  width: number;
  height: number;
  grid: EditorGuideGrid;
  centre: EditorGuideCentre;
  safe_area: EditorGuideRect;
  thirds: EditorGuideThirds;
}

/** The editor's six guide switches, mirroring the reference client's names. */
export interface EditorGuideToggles {
  /** Ghost neighbouring frames. */
  onion: boolean;
  /** Draw the pixel grid. */
  grid: boolean;
  /** Snap coordinates to grid lines. */
  snap_grid: boolean;
  /** Snap coordinates to the centre cross and draw it. */
  snap_centre: boolean;
  /** Draw the safe-area rectangle. */
  safe_area: boolean;
  /** Draw the rule-of-thirds lines. */
  thirds: boolean;
}

/** The guide view state: switches, pitches and canvas size. */
export interface EditorGuideState {
  toggles: EditorGuideToggles;
  grid_spacing: number;
  safe_area_inset: number;
  onion_radius: number;
  width: number;
  height: number;
  frame_index: number;
  frame_count: number;
}

/** One frame to ghost while onion skinning. */
export interface EditorOnionNeighbour {
  index: number;
  offset: number;
}

/** The guide switches and canvas size for the open document. */
export const editorGuideState = (): Promise<EditorGuideState> => invoke("editor_guide_state");

/** Replace the guide switches; view-only, so the document is untouched. */
export const editorSetGuideToggles = (
  toggles: EditorGuideToggles,
): Promise<EditorGuideState> => invoke("editor_set_guide_toggles", { toggles });

/** Set the onion-skin radius in frames each side; view-only. */
export const editorSetOnionRadius = (radius: number): Promise<EditorGuideState> =>
  invoke("editor_set_onion_radius", { radius });

/** Every guide line for the open canvas. */
export const editorGuideGeometry = (): Promise<EditorGuideGeometry> =>
  invoke("editor_guide_geometry");

/** The frames to ghost around the current one, nearest first. */
export const editorOnionNeighbours = (): Promise<EditorOnionNeighbour[]> =>
  invoke("editor_onion_neighbours");

/** Snap a point through the active guide switches for the open canvas. */
export const editorSnapPoint = (x: number, y: number): Promise<EditorGuidePoint> =>
  invoke("editor_snap_point", { x, y });

/** The image-resize presets the editor offers, in pixels. */
export const EDITOR_SIZE_PRESETS: ReadonlyArray<readonly [number, number]> = [
  [16, 16],
  [32, 32],
  [44, 44],
  [64, 64],
  [88, 88],
  [128, 128],
  [176, 176],
  [256, 256],
];
