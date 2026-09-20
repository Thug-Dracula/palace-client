//! The prop editor's app-side session: one in-progress document the UI drives.
//!
//! The pixel model lives in `palace_prop::editor` — frames, layers, compositing
//! and the bounded undo/redo history. This module owns what the app layer adds
//! on top:
//!
//! * **One shared session.** [`EditorSlot`] mirrors the catalog slots: an
//!   `Arc<Mutex<Option<EditorSession>>>` that Tauri manages once, so every
//!   command addresses the same in-progress document and a test can drive it
//!   without a webview.
//! * **Opening.** A blank canvas, a clone of an existing bag prop's pixels, or
//!   an image decoded from imported bytes. Opening a bag prop only **reads** the
//!   collection: the clone is what the editor mutates, so the `.prp` on disk is
//!   byte-identical until an explicit save (a later task) writes it.
//! * **Format decoding.** `palace_prop::editor` is deliberately format-free, so
//!   the app layer decodes imported PNG/JPG/BMP/GIF/WebP here, through the
//!   `image` crate the app's `Cargo.toml` declares (and justifies). Decoding
//!   produces RGBA8: PNG/GIF/WebP/BMP keep their alpha, JPEG has none and
//!   arrives opaque. An animated GIF imports its first frame — multi-frame prop
//!   *encoding* stays gated.
//! * **Commands.** Open, inspect, undo/redo, frame operations and one PNG per
//!   frame. All pixel work happens in Rust; the webview only ever receives
//!   typed state or already-encoded PNG bytes.
//!
//! # Threading
//!
//! Every command that decodes, encodes, reads a bag file or copies a document
//! runs on Tauri's blocking pool ([`editor_task`]); the webview thread only
//! takes the slot's lock to read a state snapshot. The frame PNG is encoded
//! after the lock is released, so a slow encode never stalls another command.

use std::sync::{Arc, Mutex};

use palace_prop::editor::tools::transform::{self, CropShape, ShapeCropOptions};
use palace_prop::editor::tools::{
    adjust,
    guides::{
        self, CentreCross, GuideGeometry, GuideLine, GuideToggles, GridLines, Point, Rect,
        ThirdsLines, DEFAULT_GRID_SPACING, DEFAULT_ONION_RADIUS, DEFAULT_SAFE_AREA_INSET,
    },
    paint,
};
use palace_prop::editor::{EditorDocument, Frame, Transform};
use palace_prop::prp::{PropKey, Roster};
use palace_prop::{Prop, PropImage, MAX_DIMENSION, MAX_PIXELS};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::bag::{BagOutcome, BagService};
use crate::protocol::BagSlot;
use crate::AppState;

/// The shared "nothing is open" error every mutating command reports.
const NO_SESSION: &str = "no editor session is open";

/// The refusal every save attempt reports for an animated document.
///
/// Encoding a multi-frame prop is a separate, gated task: the `.prp` blob the
/// encoder emits carries one image, so a document with more than one frame has
/// no representation to write yet. The message names the limitation and the
/// escape hatch (export the animation, or save a single frame).
pub const ANIMATED_SAVE_REFUSED: &str =
    "this document has more than one frame; saving an animated prop is not supported yet \
     (animated .prp encoding is a separate, gated task). Use Export animation to get a WebP, \
     or save a single-frame document to a .prp.";

/// The collection save writes into. My Bag is the bag folder's one writer.
const MY_BAG: &str = "My Bag";

// -- Request shapes ---------------------------------------------------------

/// How a saved prop is placed in My Bag, mirroring PalaceChat's save dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SavePlacement {
    /// Replace the bag record the document was opened from, keeping its id.
    OverwriteInPlace,
    /// Give the prop a fresh id below every id already in My Bag.
    NewPropAtStart,
    /// Give the prop a fresh id above every id already in My Bag.
    NewPropAtEnd,
}

/// The resampling filter a resize/shape-crop request selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResampleChoice {
    /// Nearest neighbour: exact colours, hard edges (pixel art).
    Nearest,
    /// Bilinear: smooth, the default.
    Bilinear,
}

impl ResampleChoice {
    /// The crate's resample filter for this choice.
    fn to_resample(self) -> transform::Resample {
        match self {
            ResampleChoice::Nearest => transform::Resample::Nearest,
            ResampleChoice::Bilinear => transform::Resample::Bilinear,
        }
    }
}

/// The mask a shape-crop request keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ShapeCropChoice {
    /// The whole square canvas.
    Square,
    /// The inscribed circle.
    Circle,
    /// A square with rounded corners of `radius` pixels.
    Rounded {
        /// Corner radius in pixels.
        radius: u32,
    },
}

impl ShapeCropChoice {
    /// The crate's crop shape for this choice.
    fn to_shape(self) -> CropShape {
        match self {
            ShapeCropChoice::Square => CropShape::Square,
            ShapeCropChoice::Circle => CropShape::Circle,
            ShapeCropChoice::Rounded { radius } => CropShape::Rounded { radius },
        }
    }
}

/// Which axis a flip request mirrors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FlipAxis {
    /// Mirror left-to-right.
    Horizontal,
    /// Mirror top-to-bottom.
    Vertical,
}

/// Which exact quarter turn a rotate request performs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QuarterTurnChoice {
    /// 90 degrees clockwise.
    Clockwise,
    /// 90 degrees counter-clockwise.
    CounterClockwise,
    /// 180 degrees.
    Half,
}

impl QuarterTurnChoice {
    /// The crate's quarter turn for this choice.
    fn to_turn(self) -> transform::QuarterTurn {
        match self {
            QuarterTurnChoice::Clockwise => transform::QuarterTurn::Clockwise,
            QuarterTurnChoice::CounterClockwise => transform::QuarterTurn::CounterClockwise,
            QuarterTurnChoice::Half => transform::QuarterTurn::Half,
        }
    }
}

/// A brush/eraser stroke: two endpoints, a colour and a stamp size.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StrokeRequest {
    /// Start point x (layer pixels).
    pub from_x: i32,
    /// Start point y.
    pub from_y: i32,
    /// End point x; equal to the start for a single click.
    pub to_x: i32,
    /// End point y.
    pub to_y: i32,
    /// RGBA colour; the brush overrides alpha to opaque.
    pub color: [u8; 4],
    /// Stamp side length in pixels, clamped to 1..=48.
    pub size: u32,
    /// Layer index to paint on (0 is the base layer).
    pub layer: usize,
    /// Apply the same stroke to every frame.
    #[serde(default)]
    pub all_frames: bool,
}

/// Which tool a pointer drag stroke uses.
///
/// The one-shot `editor_paint_stroke`/`editor_erase_stroke` commands already
/// exist; this only names the tool so a drag can be expressed as one `begin`
/// plus a series of `extend` calls that share a single undo snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StrokeMode {
    /// Paint an opaque brush stroke.
    #[default]
    Brush,
    /// Erase along the stroke (clear alpha).
    Erase,
}

/// One segment of a pointer drag stroke: the tool plus a [`StrokeRequest`].
///
/// The stroke fields are flattened into the same object so the wire shape is
/// `{ mode, fromX, fromY, toX, toY, color, size, layer, allFrames }`.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StrokeSessionRequest {
    /// Which tool the drag uses; defaults to the brush.
    #[serde(default)]
    pub mode: StrokeMode,
    /// The segment's endpoints, colour, size, layer and frame scope.
    #[serde(flatten)]
    pub stroke: StrokeRequest,
}

/// A flood-fill seed plus its colour and tolerance.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FloodFillRequest {
    /// Seed x.
    pub x: u32,
    /// Seed y.
    pub y: u32,
    /// Fill colour, alpha included.
    pub color: [u8; 4],
    /// Per-channel tolerance, `0.0..=1.0` of full scale.
    pub tolerance: f32,
    /// Layer index to fill (0 is the base layer).
    pub layer: usize,
    /// Apply to every frame.
    #[serde(default)]
    pub all_frames: bool,
}

/// Whether a frame-level operation applies to one frame or all of them.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameScopeRequest {
    /// Apply to every frame instead of just the selected one.
    #[serde(default)]
    pub all_frames: bool,
}

/// A flip request.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlipRequest {
    /// Which axis to mirror.
    pub axis: FlipAxis,
    /// Apply to every frame.
    #[serde(default)]
    pub all_frames: bool,
}

/// A rotate request.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RotateRequest {
    /// Which quarter turn to perform.
    pub turn: QuarterTurnChoice,
    /// Apply to every frame.
    #[serde(default)]
    pub all_frames: bool,
}

/// A free crop rectangle.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CropRequest {
    /// Left edge in pixels.
    pub x: u32,
    /// Top edge in pixels.
    pub y: u32,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Apply to every frame.
    #[serde(default)]
    pub all_frames: bool,
}

/// A shape-crop request: mask, zoom and rotation.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShapeCropRequest {
    /// Which mask to keep.
    pub shape: ShapeCropChoice,
    /// Magnification about the canvas centre.
    pub zoom: f32,
    /// Clockwise rotation in degrees.
    pub rotation: f32,
    /// Resampling filter.
    pub resample: ResampleChoice,
    /// Apply to every frame.
    #[serde(default)]
    pub all_frames: bool,
}

/// An image-resize request.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResizeRequest {
    /// Target width in pixels.
    pub width: u32,
    /// Target height in pixels.
    pub height: u32,
    /// Keep the source aspect ratio, fitting inside `width` x `height`.
    #[serde(default)]
    pub constrain_aspect: bool,
    /// Resampling filter.
    pub resample: ResampleChoice,
    /// Apply to every frame.
    #[serde(default)]
    pub all_frames: bool,
}

/// A brightness/contrast request.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrightnessContrastRequest {
    /// Additive brightness, `-1.0..=1.0`.
    pub brightness: f32,
    /// Contrast strength, `-1.0..=1.0`.
    pub contrast: f32,
    /// Apply to every frame.
    #[serde(default)]
    pub all_frames: bool,
}

/// A colour-adjust request.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ColourAdjustRequest {
    /// Additive brightness, `-1.0..=1.0`.
    pub brightness: f32,
    /// Contrast strength, `-1.0..=1.0`.
    pub contrast: f32,
    /// Saturation strength, `-1.0..=1.0`.
    pub saturation: f32,
    /// Apply to every frame.
    #[serde(default)]
    pub all_frames: bool,
}

/// A chroma-key background-removal request.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoveBackgroundRequest {
    /// The background colour to cut, as `[r, g, b]`.
    pub key: [u8; 3],
    /// Per-channel envelope, `0.0..=1.0`.
    pub tolerance: f32,
    /// Soft alpha ramp width, `0.0..=1.0`.
    pub feather: f32,
    /// Apply to every frame.
    #[serde(default)]
    pub all_frames: bool,
}

/// The text tool's request: the string plus every draw parameter.
///
/// `opacity` defaults to opaque and `size` is in pixels; `wrap` clamps the
/// text block to the frame's width when true.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextLayerRequest {
    /// The text to rasterize. Empty or whitespace-only is refused.
    pub text: String,
    /// Fill colour, RGBA (alpha included).
    pub color: [u8; 4],
    /// Font size in pixels, clamped to a sane range.
    pub size: f32,
    /// Layer top-left x, in frame pixels.
    #[serde(default)]
    pub x: i32,
    /// Layer top-left y, in frame pixels.
    #[serde(default)]
    pub y: i32,
    /// Clockwise rotation in degrees about the layer centre.
    #[serde(default)]
    pub rotation: f32,
    /// Draw opacity, `0.0..=1.0`; defaults to opaque.
    #[serde(default = "opaque")]
    pub opacity: f32,
    /// Draw a dark drop shadow behind the glyphs.
    #[serde(default)]
    pub shadow: bool,
    /// Draw a contrasting outline around the glyphs.
    #[serde(default)]
    pub outline: bool,
    /// Wrap lines to the frame's width instead of one long line.
    #[serde(default)]
    pub wrap: bool,
}

/// The default draw opacity for a new layer.
fn opaque() -> f32 {
    1.0
}

/// Clamp an opacity into `0.0..=1.0`, treating a non-finite value as opaque.
fn sane_opacity(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        1.0
    }
}

/// A partial update to one layer, addressed by its index (base is 0).
///
/// Every field is optional so the layer list can flip visibility without
/// resending the transform, and drag a position without resending the colour.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LayerUpdateRequest {
    /// Layer index in the selected frame, base first.
    pub layer: usize,
    /// New display name; blank is ignored.
    #[serde(default)]
    pub name: Option<String>,
    /// New draw opacity, `0.0..=1.0`.
    #[serde(default)]
    pub opacity: Option<f32>,
    /// Whether the layer is drawn.
    #[serde(default)]
    pub visible: Option<bool>,
    /// New horizontal placement in frame pixels.
    #[serde(default)]
    pub x: Option<i32>,
    /// New vertical placement in frame pixels.
    #[serde(default)]
    pub y: Option<i32>,
    /// New clockwise rotation in degrees.
    #[serde(default)]
    pub rotation: Option<f32>,
    /// New uniform scale.
    #[serde(default)]
    pub scale: Option<f32>,
}

/// Everything the save dialog sends: placement, identity and prop flags.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavePropRequest {
    /// How to place the prop in My Bag.
    pub placement: SavePlacement,
    /// The record's name; blank means unnamed.
    #[serde(default)]
    pub name: Option<String>,
    /// An explicit prop id for a new prop; `None` allocates one by placement.
    #[serde(default)]
    pub prop_id: Option<u32>,
    /// Set the `HEAD` flag.
    #[serde(default)]
    pub head: bool,
    /// Set the `GHOST` flag.
    #[serde(default)]
    pub ghost: bool,
    /// Set the `RARE` flag.
    #[serde(default)]
    pub rare: bool,
    /// Set the `BOUNCE` flag.
    #[serde(default)]
    pub bounce: bool,
    /// Set the `ANIMATE` flag (the flag, not multi-frame content).
    #[serde(default)]
    pub animate: bool,
    /// Horizontal anchor offset written into the prop header.
    #[serde(default)]
    pub h_offset: i16,
    /// Vertical anchor offset written into the prop header.
    #[serde(default)]
    pub v_offset: i16,
}

/// The result of a save: where it landed and the session's new state.
#[derive(Debug, Clone, Serialize)]
pub struct EditorSaveResult {
    /// What the bag writer did.
    pub outcome: crate::bag::BagOutcome,
    /// The prop id the record now has.
    pub id: u32,
    /// The payload CRC of the written blob.
    pub crc: u32,
    /// The name stored, when one was given.
    pub name: Option<String>,
    /// A one-line human summary for the status bar.
    pub message: String,
    /// The session state after the save (origin now points at the record).
    pub state: EditorState,
}

/// A shared handle to the app's one in-progress editor document.
///
/// Mirrors [`crate::protocol::CatalogSlot`]: the slot is managed once at
/// startup, so every command and every test addresses the same session. The
/// lock is only held around the document itself; file reads, image decoding and
/// PNG encoding happen outside it.
#[derive(Clone, Default)]
pub struct EditorSlot {
    inner: Arc<Mutex<Option<EditorSession>>>,
}

impl EditorSlot {
    /// Replace the session with a freshly opened one.
    pub fn set(&self, session: EditorSession) {
        match self.inner.lock() {
            Ok(mut guard) => *guard = Some(session),
            Err(poisoned) => *poisoned.into_inner() = Some(session),
        }
    }

    /// Run `read` on the open session; `None` when no session is open.
    pub fn with_session<T>(&self, read: impl FnOnce(&EditorSession) -> T) -> Option<T> {
        let guard = lock(&self.inner);
        guard.as_ref().map(read)
    }

    /// Run `mutate` on the open session, or fail when nothing is open.
    pub fn with_session_mut<T>(
        &self,
        mutate: impl FnOnce(&mut EditorSession) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut guard = lock(&self.inner);
        match guard.as_mut() {
            Some(session) => mutate(session),
            None => Err(NO_SESSION.to_string()),
        }
    }
}

/// Lock a slot mutex, recovering a poisoned one: a panicked command must not
/// take the editor down with it, exactly like the frame and catalog slots.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// The one in-progress document plus where it came from.
#[derive(Debug)]
pub struct EditorSession {
    document: EditorDocument,
    origin: EditorOrigin,
    saved_undo_depth: Option<usize>,
    /// The guide switches and pitches; presentation state, never snapshotted.
    guides: GuideSettings,
    /// Whether the in-progress pointer drag has already pushed its one snapshot.
    ///
    /// A drag is one `begin` followed by any number of `extend` calls; this flag
    /// is what keeps the whole drag to a single undo step. It is reset by the
    /// next `begin`, so no `end` command is needed.
    stroke_snapshot_taken: bool,
}

impl EditorSession {
    /// A blank, fully transparent canvas.
    pub fn blank(width: u32, height: u32) -> Result<Self, String> {
        let document = EditorDocument::blank(width, height)
            .map_err(|error| format!("could not open a {width}x{height} canvas: {error}"))?;
        Ok(Self {
            document,
            origin: EditorOrigin::Blank,
            saved_undo_depth: None,
            guides: GuideSettings::default(),
            stroke_snapshot_taken: false,
        })
    }

    /// A clone of an already-decoded bag prop's pixels.
    ///
    /// The document copies the prop's image, so editing the clone cannot touch
    /// the collection's bytes.
    #[must_use]
    pub fn from_bag_prop(
        prop: &Prop,
        collection: String,
        id: u32,
        crc: u32,
        name: Option<String>,
    ) -> Self {
        Self {
            document: EditorDocument::from_prop(prop),
            origin: EditorOrigin::BagProp {
                collection,
                id,
                crc,
                name,
            },
            saved_undo_depth: None,
            guides: GuideSettings::default(),
            stroke_snapshot_taken: false,
        }
    }

    /// An image decoded from imported bytes (PNG/JPG/BMP/GIF/WebP).
    pub fn from_image_bytes(bytes: &[u8]) -> Result<Self, String> {
        Ok(Self {
            document: decode_image(bytes)?,
            origin: EditorOrigin::Image,
            saved_undo_depth: None,
            guides: GuideSettings::default(),
            stroke_snapshot_taken: false,
        })
    }

    /// Everything `editor_state` reports about the open document.
    #[must_use]
    pub fn state(&self) -> EditorState {
        let current = self.document.current_frame();
        let frames = self
            .document
            .frames()
            .iter()
            .enumerate()
            .map(|(index, frame)| EditorFrameSummary {
                index,
                width: frame.width(),
                height: frame.height(),
                layer_count: frame.layer_count(),
                selected: index == self.document.current_index(),
            })
            .collect();
        let layers = current
            .map(|frame| {
                frame
                    .layers()
                    .iter()
                    .map(|layer| EditorLayerSummary {
                        id: layer.id.get(),
                        name: layer.name.clone(),
                        width: layer.image.width(),
                        height: layer.image.height(),
                        opacity: layer.opacity,
                        visible: layer.visible,
                        x: layer.transform.x,
                        y: layer.transform.y,
                        rotation: layer.transform.rotation,
                        scale: layer.transform.scale,
                    })
                    .collect()
            })
            .unwrap_or_default();
        let history = self.document.history();
        let dirty = match self.saved_undo_depth {
            Some(saved) => history.undo_len() != saved,
            None => self.document.can_undo(),
        };
        EditorState {
            origin: self.origin.clone(),
            width: current.map_or(0, |frame| frame.width()),
            height: current.map_or(0, |frame| frame.height()),
            frame_count: self.document.frame_count(),
            frame_index: self.document.current_index(),
            frames,
            layers,
            can_undo: self.document.can_undo(),
            can_redo: self.document.can_redo(),
            can_paste: self.document.has_clipboard(),
            undo_depth: history.undo_len(),
            redo_depth: history.redo_len(),
            undo_bytes: history.memory_bytes(),
            dirty,
        }
    }

    /// The composited pixels of `index` — what `editor_frame_png` encodes.
    ///
    /// A document always has at least one frame; an out-of-range index is the
    /// only failure, reported with the frame count so the UI can correct itself.
    pub fn frame_image(&self, index: usize) -> Result<PropImage, String> {
        let frame = self.document.frame(index).ok_or_else(|| {
            format!(
                "frame {index} is out of range (the document has {} frames)",
                self.document.frame_count()
            )
        })?;
        frame
            .composite()
            .map_err(|error| format!("frame {index} could not be composited: {error}"))
    }

    /// Undo one snapshot; `changed` is false when the history is empty.
    pub fn undo(&mut self) -> EditorOutcome {
        let changed = self.document.undo();
        EditorOutcome {
            changed,
            state: self.state(),
        }
    }

    /// Reapply the most recently undone snapshot; `changed` is false when the
    /// redo branch is empty.
    pub fn redo(&mut self) -> EditorOutcome {
        let changed = self.document.redo();
        EditorOutcome {
            changed,
            state: self.state(),
        }
    }

    /// Append a blank frame after the current one and select it.
    ///
    /// The snapshot is pushed before the mutation, so one undo removes the new
    /// frame exactly as it removes a paint stroke.
    pub fn add_frame(&mut self) -> EditorOutcome {
        self.document.push_snapshot();
        let changed = self.document.add_blank_frame().is_some();
        EditorOutcome {
            changed,
            state: self.state(),
        }
    }

    /// Duplicate the frame at `index` directly after itself, selecting the copy.
    pub fn duplicate_frame(&mut self, index: usize) -> EditorOutcome {
        if index >= self.document.frame_count() {
            return self.unchanged();
        }
        self.document.push_snapshot();
        let changed = self.document.duplicate_frame(index).is_some();
        EditorOutcome {
            changed,
            state: self.state(),
        }
    }

    /// Delete the frame at `index`; the document's last frame cannot be removed.
    pub fn delete_frame(&mut self, index: usize) -> EditorOutcome {
        if index >= self.document.frame_count() || self.document.frame_count() <= 1 {
            return self.unchanged();
        }
        self.document.push_snapshot();
        let changed = self.document.delete_frame(index).is_some();
        EditorOutcome {
            changed,
            state: self.state(),
        }
    }

    /// Move the frame at `from` so it sits at `to`.
    ///
    /// A move to the same index is a no-op and pushes no snapshot, so
    /// `changed` is false and the history is not polluted.
    pub fn move_frame(&mut self, from: usize, to: usize) -> EditorOutcome {
        let count = self.document.frame_count();
        if from >= count || to >= count || from == to {
            return self.unchanged();
        }
        self.document.push_snapshot();
        let changed = self.document.move_frame(from, to);
        EditorOutcome {
            changed,
            state: self.state(),
        }
    }

    /// Select the frame at `index` for painting.
    ///
    /// Selection is not pixel state, so no snapshot is pushed: undoing a later
    /// paint restores the selection the paint was made at.
    pub fn select_frame(&mut self, index: usize) -> EditorOutcome {
        let changed = self.document.set_current(index);
        EditorOutcome {
            changed,
            state: self.state(),
        }
    }

    /// The outcome of a refused or no-op operation: nothing changed, and the
    /// state is reported unchanged so the caller can render the refusal.
    fn unchanged(&self) -> EditorOutcome {
        EditorOutcome {
            changed: false,
            state: self.state(),
        }
    }

    // -- Guides -------------------------------------------------------------

    /// The guide switches, pitches and canvas size for the open document.
    #[must_use]
    pub fn guide_state(&self) -> GuideState {
        let current = self.document.current_frame();
        GuideState {
            toggles: self.guides.toggles.into(),
            grid_spacing: self.guides.grid_spacing,
            safe_area_inset: self.guides.safe_area_inset,
            onion_radius: self.guides.onion_radius,
            width: current.map_or(0, Frame::width),
            height: current.map_or(0, Frame::height),
            frame_index: self.document.current_index(),
            frame_count: self.document.frame_count(),
        }
    }

    /// Replace the guide switches; view-only, so no snapshot is pushed.
    pub fn set_guide_toggles(&mut self, toggles: GuideTogglesState) -> GuideState {
        self.guides.toggles = toggles.into();
        self.guide_state()
    }

    /// Set the onion-skin radius in frames each side; view-only.
    pub fn set_onion_radius(&mut self, radius: usize) -> GuideState {
        self.guides.onion_radius = radius;
        self.guide_state()
    }

    /// Every guide line for the open canvas, or `None` when nothing is open.
    #[must_use]
    pub fn guide_geometry(&self) -> Option<GuideGeometryState> {
        let frame = self.document.current_frame()?;
        let geometry = GuideGeometry::for_canvas(
            frame.width(),
            frame.height(),
            self.guides.grid_spacing,
            self.guides.safe_area_inset,
        );
        Some(guide_geometry_state(geometry))
    }

    /// The frames to ghost around the current one, nearest first.
    #[must_use]
    pub fn onion_neighbours(&self) -> Vec<OnionNeighbourState> {
        guides::onion_neighbours(
            self.document.frames(),
            self.document.current_index(),
            self.guides.onion_radius,
        )
        .into_iter()
        .map(|neighbour| OnionNeighbourState {
            index: neighbour.index,
            offset: neighbour.offset,
        })
        .collect()
    }

    /// Snap one point through the active switches for the current canvas.
    #[must_use]
    pub fn snap_point(&self, x: i32, y: i32) -> GuidePointState {
        guide_point(self.snapped(x, y))
    }

    /// The canvas size a snap should be measured against.
    fn canvas_size(&self) -> (u32, u32) {
        self.document
            .current_frame()
            .map_or((0, 0), |frame| (frame.width(), frame.height()))
    }

    /// Whether any coordinate snap is on.
    fn snaps_active(&self) -> bool {
        self.guides.toggles.snap_grid || self.guides.toggles.snap_centre
    }

    /// Snap a point through the active switches for the current canvas.
    fn snapped(&self, x: i32, y: i32) -> Point {
        let (width, height) = self.canvas_size();
        self.guides
            .toggles
            .snap(Point::new(x, y), width, height, self.guides.grid_spacing)
    }

    /// Snap both endpoints of a stroke when a snap switch is on.
    fn snap_stroke(&self, request: &StrokeRequest) -> StrokeRequest {
        if !self.snaps_active() {
            return *request;
        }
        let from = self.snapped(request.from_x, request.from_y);
        let to = self.snapped(request.to_x, request.to_y);
        StrokeRequest {
            from_x: from.x,
            from_y: from.y,
            to_x: to.x,
            to_y: to.y,
            ..*request
        }
    }

    // -- Tools: paint -------------------------------------------------------

    /// Paint an opaque brush stroke, on one frame or every frame.
    ///
    /// When snap-to-grid or snap-to-centre is on, both endpoints are moved onto
    /// their guide first: the snap is part of the paint input path, so the
    /// stroke lands where the guide shows.
    pub fn paint_stroke(&mut self, request: &StrokeRequest) -> Result<EditorOutcome, String> {
        let request = self.snap_stroke(request);
        let color = request.color;
        let changed = self.apply_frame_ops(request.all_frames, |frame| {
            paint::brush_stroke_frame(
                frame,
                request.layer,
                (request.from_x, request.from_y),
                (request.to_x, request.to_y),
                color,
                request.size,
            )
        })?;
        Ok(self.outcome(changed))
    }

    /// Erase along a stroke (clear alpha), on one frame or every frame.
    ///
    /// Snapping applies to the eraser exactly as it does to the brush, so both
    /// tools honour the same guide.
    pub fn erase_stroke(&mut self, request: &StrokeRequest) -> Result<EditorOutcome, String> {
        let request = self.snap_stroke(request);
        let changed = self.apply_frame_ops(request.all_frames, |frame| {
            paint::erase_stroke_frame(
                frame,
                request.layer,
                (request.from_x, request.from_y),
                (request.to_x, request.to_y),
                request.size,
            )
        })?;
        Ok(self.outcome(changed))
    }

    /// Begin a pointer drag stroke: paint the first segment, then take the
    /// drag's one undo snapshot.
    ///
    /// The snapshot is deferred until a segment actually changes a pixel, so a
    /// drag that paints nothing leaves the history untouched. [`Self::extend_stroke`]
    /// reuses the same snapshot, which is what makes a whole freehand drag a
    /// single undo step instead of one per pointer-move event.
    pub fn begin_stroke(
        &mut self,
        request: &StrokeSessionRequest,
    ) -> Result<EditorOutcome, String> {
        self.stroke_snapshot_taken = false;
        let changed = self.apply_stroke(request)?;
        Ok(self.outcome(changed))
    }

    /// Extend the in-progress pointer drag stroke; never pushes a second
    /// snapshot, so the entire drag undoes in one step.
    pub fn extend_stroke(
        &mut self,
        request: &StrokeSessionRequest,
    ) -> Result<EditorOutcome, String> {
        let changed = self.apply_stroke(request)?;
        Ok(self.outcome(changed))
    }

    /// Paint or erase one drag segment, snapshotting at most once per drag.
    fn apply_stroke(&mut self, request: &StrokeSessionRequest) -> Result<bool, String> {
        let stroke = self.snap_stroke(&request.stroke);
        let mode = request.mode;
        let planned = self.plan_frame_ops(stroke.all_frames, &mut |frame| match mode {
            StrokeMode::Brush => paint::brush_stroke_frame(
                frame,
                stroke.layer,
                (stroke.from_x, stroke.from_y),
                (stroke.to_x, stroke.to_y),
                stroke.color,
                stroke.size,
            ),
            StrokeMode::Erase => paint::erase_stroke_frame(
                frame,
                stroke.layer,
                (stroke.from_x, stroke.from_y),
                (stroke.to_x, stroke.to_y),
                stroke.size,
            ),
        })?;
        if planned.is_empty() {
            return Ok(false);
        }
        if !self.stroke_snapshot_taken {
            self.document.push_snapshot();
            self.stroke_snapshot_taken = true;
        }
        self.write_frames(planned);
        Ok(true)
    }

    /// Flood-fill a region, on one frame or every frame.
    pub fn flood_fill(&mut self, request: &FloodFillRequest) -> Result<EditorOutcome, String> {
        let color = request.color;
        let tolerance = request.tolerance;
        let changed = self.apply_frame_ops(request.all_frames, |frame| {
            paint::flood_fill_frame(frame, request.layer, request.x, request.y, color, tolerance)
        })?;
        Ok(self.outcome(changed))
    }

    /// Sample a layer pixel's colour without changing the document.
    #[must_use]
    pub fn pick_color(&self, layer: usize, x: u32, y: u32) -> Option<[u8; 4]> {
        self.document
            .current_frame()
            .and_then(|frame| paint::pick_color_frame(frame, layer, x, y))
    }

    /// Clear a frame's pixels to transparent (every layer), on one or all frames.
    pub fn clear_frame(&mut self, all_frames: bool) -> Result<EditorOutcome, String> {
        let changed =
            self.apply_frame_ops(all_frames, |frame| Ok(Some(paint::clear_frame(frame))))?;
        Ok(self.outcome(changed))
    }

    // -- Tools: transform ---------------------------------------------------

    /// Flip one frame or every frame on an axis.
    pub fn flip(&mut self, request: &FlipRequest) -> Result<EditorOutcome, String> {
        let axis = request.axis;
        let changed = self.apply_frame_ops(request.all_frames, |frame| {
            let mut next = frame.clone();
            transform::transform_frame(&mut next, |image| match axis {
                FlipAxis::Horizontal => transform::flip_horizontal(image),
                FlipAxis::Vertical => transform::flip_vertical(image),
            })?;
            Ok(Some(next))
        })?;
        Ok(self.outcome(changed))
    }

    /// Rotate one frame or every frame by an exact quarter turn.
    pub fn rotate(&mut self, request: &RotateRequest) -> Result<EditorOutcome, String> {
        let turn = request.turn.to_turn();
        let changed = self.apply_frame_ops(request.all_frames, |frame| {
            let mut next = frame.clone();
            transform::transform_frame(&mut next, |image| transform::rotate(image, turn))?;
            Ok(Some(next))
        })?;
        Ok(self.outcome(changed))
    }

    /// Crop one frame or every frame to a rectangle.
    pub fn crop(&mut self, request: &CropRequest) -> Result<EditorOutcome, String> {
        let rect = transform::CropRect::new(request.x, request.y, request.width, request.height);
        let changed = self.apply_frame_ops(request.all_frames, |frame| {
            let mut next = frame.clone();
            transform::transform_frame(&mut next, |image| transform::crop(image, rect))?;
            Ok(Some(next))
        })?;
        Ok(self.outcome(changed))
    }

    /// Crop one frame or every frame to its largest centred square.
    pub fn crop_square(&mut self, all_frames: bool) -> Result<EditorOutcome, String> {
        let changed = self.apply_frame_ops(all_frames, |frame| {
            let mut next = frame.clone();
            transform::transform_frame(&mut next, transform::crop_square)?;
            Ok(Some(next))
        })?;
        Ok(self.outcome(changed))
    }

    /// Shape-crop one frame or every frame (square/circle/rounded, zoom, rotation).
    pub fn shape_crop(&mut self, request: &ShapeCropRequest) -> Result<EditorOutcome, String> {
        let options = ShapeCropOptions {
            shape: request.shape.to_shape(),
            zoom: request.zoom,
            rotation: request.rotation,
            resample: request.resample.to_resample(),
        };
        let changed = self.apply_frame_ops(request.all_frames, |frame| {
            let mut next = frame.clone();
            transform::shape_crop_frame(&mut next, &options)?;
            Ok(Some(next))
        })?;
        Ok(self.outcome(changed))
    }

    /// Resize one frame or every frame, optionally keeping the aspect ratio.
    pub fn resize(&mut self, request: &ResizeRequest) -> Result<EditorOutcome, String> {
        let resample = request.resample.to_resample();
        let (width, height, constrain) = (request.width, request.height, request.constrain_aspect);
        let changed = self.apply_frame_ops(request.all_frames, |frame| {
            let mut next = frame.clone();
            transform::transform_frame(&mut next, |image| {
                if constrain {
                    transform::resize_to_fit(image, width, height, resample)
                } else {
                    transform::resize(image, width, height, resample)
                }
            })?;
            Ok(Some(next))
        })?;
        Ok(self.outcome(changed))
    }

    // -- Tools: adjust ------------------------------------------------------

    /// Apply brightness and contrast, to one frame or every frame.
    pub fn adjust_brightness_contrast(
        &mut self,
        request: &BrightnessContrastRequest,
    ) -> Result<EditorOutcome, String> {
        let setting = adjust::BrightnessContrast {
            brightness: request.brightness,
            contrast: request.contrast,
        };
        let changed = self.apply_adjust(request.all_frames, |frame| {
            adjust::apply_brightness_contrast_all_frames(std::slice::from_ref(frame), setting)
        })?;
        Ok(self.outcome(changed))
    }

    /// Apply brightness, contrast and saturation, to one frame or every frame.
    pub fn adjust_colour(
        &mut self,
        request: &ColourAdjustRequest,
    ) -> Result<EditorOutcome, String> {
        let setting = adjust::ColourAdjust {
            brightness: request.brightness,
            contrast: request.contrast,
            saturation: request.saturation,
        };
        let changed = self.apply_adjust(request.all_frames, |frame| {
            adjust::apply_colour_adjust_all_frames(std::slice::from_ref(frame), setting)
        })?;
        Ok(self.outcome(changed))
    }

    /// Remove a chroma-key background, from one frame or every frame.
    pub fn remove_background(
        &mut self,
        request: &RemoveBackgroundRequest,
    ) -> Result<EditorOutcome, String> {
        let key = adjust::ChromaKey {
            key: request.key,
            tolerance: request.tolerance,
            feather: request.feather,
        };
        let changed = self.apply_adjust(request.all_frames, |frame| {
            adjust::remove_background_all_frames(std::slice::from_ref(frame), key)
        })?;
        Ok(self.outcome(changed))
    }

    /// Sample a key colour for background removal, without changing the document.
    #[must_use]
    pub fn sample_key_color(&self, layer: usize, x: u32, y: u32) -> Option<[u8; 3]> {
        let image = self
            .document
            .current_frame()
            .and_then(|frame| frame.layer(layer))?
            .image();
        adjust::sample_key_colour(image, x, y)
    }

    // -- Layers: text, overlays and the layer list --------------------------

    /// Rasterize the request's text and push it as a new layer on top.
    ///
    /// The whole text block is drawn off the webview: shadow and outline are
    /// baked into the layer's pixels, while position, rotation and opacity ride
    /// the layer transform so the layer list can keep adjusting them.
    pub fn add_text_layer(&mut self, request: &TextLayerRequest) -> Result<EditorOutcome, String> {
        if request.text.trim().is_empty() {
            return Err("enter some text before adding a text layer".to_string());
        }
        let wrap_width = if request.wrap {
            self.document.current_frame().map(Frame::width)
        } else {
            None
        };
        let image = text::render(
            &request.text,
            request.color,
            request.size,
            request.shadow,
            request.outline,
            wrap_width,
        )?;
        let name = text::layer_name(&request.text);
        let transform = Transform {
            x: request.x,
            y: request.y,
            rotation: if request.rotation.is_finite() {
                request.rotation
            } else {
                0.0
            },
            scale: 1.0,
        };
        self.push_layer_with(image, &name, transform, sane_opacity(request.opacity))
    }

    /// Add one already-decoded image as a new draggable overlay layer on top.
    pub fn add_overlay_layer(
        &mut self,
        image: PropImage,
        name: Option<String>,
    ) -> Result<EditorOutcome, String> {
        let count = self.document.current_frame().map_or(0, Frame::layer_count);
        let name = name
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| format!("Overlay {count}"));
        self.push_layer_with(image, &name, Transform::default(), 1.0)
    }

    /// Apply a partial update to one layer, snapshotting only on a real change.
    pub fn update_layer(&mut self, request: &LayerUpdateRequest) -> Result<EditorOutcome, String> {
        let Some(frame) = self.document.current_frame() else {
            return Ok(self.unchanged());
        };
        if request.layer >= frame.layer_count() {
            return Ok(self.unchanged());
        }
        let mut next = frame.clone();
        if let Some(layer) = next.layer_mut(request.layer) {
            if let Some(name) = request
                .name
                .as_deref()
                .map(str::trim)
                .filter(|name| !name.is_empty())
            {
                layer.name = name.to_string();
            }
            if let Some(opacity) = request.opacity {
                layer.opacity = sane_opacity(opacity);
            }
            if let Some(visible) = request.visible {
                layer.visible = visible;
            }
            if let Some(x) = request.x {
                layer.transform.x = self.snapped(x, layer.transform.y).x;
            }
            if let Some(y) = request.y {
                layer.transform.y = self.snapped(layer.transform.x, y).y;
            }
            if let Some(rotation) = request.rotation {
                if rotation.is_finite() {
                    layer.transform.rotation = rotation;
                }
            }
            if let Some(scale) = request.scale {
                if scale.is_finite() {
                    layer.transform.scale = scale;
                }
            }
        }
        if next == *frame {
            return Ok(self.unchanged());
        }
        self.commit_frames(vec![(self.document.current_index(), next)]);
        Ok(self.outcome(true))
    }

    /// Move a layer between overlay slots; the base layer stays at index 0.
    pub fn move_layer(&mut self, from: usize, to: usize) -> EditorOutcome {
        let Some(frame) = self.document.current_frame() else {
            return self.unchanged();
        };
        let count = frame.layer_count();
        if from < 1 || to < 1 || from >= count || to >= count || from == to {
            return self.unchanged();
        }
        let mut next = frame.clone();
        let layers = next.layers_mut();
        if from < to {
            layers[from..=to].rotate_left(1);
        } else {
            layers[to..=from].rotate_right(1);
        }
        self.commit_frames(vec![(self.document.current_index(), next)]);
        self.outcome(true)
    }

    /// Delete an overlay or text layer; the base layer cannot be removed.
    pub fn delete_layer(&mut self, index: usize) -> EditorOutcome {
        let Some(frame) = self.document.current_frame() else {
            return self.unchanged();
        };
        if index == 0 || index >= frame.layer_count() {
            return self.unchanged();
        }
        let mut next = frame.clone();
        if next.remove_layer(index).is_none() {
            return self.unchanged();
        }
        self.commit_frames(vec![(self.document.current_index(), next)]);
        self.outcome(true)
    }

    /// Push `image` as the top layer, then set its transform and opacity.
    ///
    /// The snapshot is taken before the layer exists, so one undo removes the
    /// new layer and everything the call set on it.
    fn push_layer_with(
        &mut self,
        image: PropImage,
        name: &str,
        transform: Transform,
        opacity: f32,
    ) -> Result<EditorOutcome, String> {
        if self.document.current_frame().is_none() {
            return Ok(self.unchanged());
        }
        self.document.push_snapshot();
        if self.document.add_layer(name, image).is_none() {
            return Ok(self.unchanged());
        }
        if let Some(frame) = self.document.current_frame_mut() {
            if let Some(layer) = frame.layers_mut().last_mut() {
                layer.transform = transform;
                layer.opacity = opacity;
            }
        }
        Ok(self.outcome(true))
    }

    // -- Frames: clipboard --------------------------------------------------

    /// Copy a frame into the editor clipboard (not a document mutation).
    pub fn copy_frame(&mut self, index: usize) -> EditorOutcome {
        let copied = self.document.copy_frame(index);
        self.outcome(copied)
    }

    /// Insert a copy of the clipboard after the current frame.
    pub fn paste_frame(&mut self) -> EditorOutcome {
        if !self.document.has_clipboard() {
            return self.unchanged();
        }
        self.document.push_snapshot();
        let changed = self.document.paste_after_current().is_some();
        self.outcome(changed)
    }

    // -- Save and export ----------------------------------------------------

    /// Write the current single-frame prop into My Bag.
    ///
    /// A multi-frame document is refused: encoding an animated `.prp` is a
    /// separate, gated task. The pixels are composited, the S20 encoding policy
    /// is applied, and the blob is handed to the bag store's writer. Placement
    /// decides the record's id: overwrite reuses the opened record's id, while
    /// the new-prop options allocate below or above every id already in My Bag.
    pub fn save_to_bag(
        &mut self,
        bag: &BagService,
        request: &SavePropRequest,
    ) -> Result<EditorSaveResult, String> {
        if self.document.frame_count() > 1 {
            return Err(ANIMATED_SAVE_REFUSED.to_string());
        }
        let image = self
            .document
            .composite_current()
            .map_err(|error| format!("the frame could not be composited for saving: {error}"))?;
        palace_prop::encoding::encoding_for_pixels(&image)
            .map_err(|error| format!("this frame cannot be saved as a .prp: {error}"))?;
        let flags = prop_flags(request);
        let blob = palace_prop::encode_s20_blob(&image, request.h_offset, request.v_offset, flags)
            .map_err(|error| format!("the prop could not be encoded: {error}"))?;
        let crc = palace_prop::asset_crc(&blob[palace_prop::HEADER_LEN..]);
        let name = clean_name(request.name.as_deref());

        let (id, outcome) = match request.placement {
            SavePlacement::OverwriteInPlace => {
                let EditorOrigin::BagProp {
                    id,
                    crc: previous_crc,
                    ..
                } = &self.origin
                else {
                    return Err(
                        "Overwrite in place needs a prop opened from the bag; use New prop at \
                         start of bag or New prop at end of bag for a new prop."
                            .to_string(),
                    );
                };
                let (id, previous_crc) = (*id, *previous_crc);
                let _ = bag.remove_prop(id, previous_crc)?;
                let outcome = bag.add_prop(id, crc, &blob, name.as_deref())?;
                (id, outcome)
            }
            SavePlacement::NewPropAtStart | SavePlacement::NewPropAtEnd => {
                let id = match request.prop_id {
                    Some(explicit) => explicit,
                    None => allocate_prop_id(bag, request.placement)?,
                };
                let outcome = bag.add_prop(id, crc, &blob, name.as_deref())?;
                (id, outcome)
            }
        };

        self.origin = EditorOrigin::BagProp {
            collection: MY_BAG.to_string(),
            id,
            crc,
            name: name.clone(),
        };
        self.saved_undo_depth = Some(self.document.history().undo_len());
        let state = self.state();
        Ok(EditorSaveResult {
            outcome,
            id,
            crc,
            name,
            message: save_message(outcome, id, crc),
            state,
        })
    }

    /// Encode the selected frame as a standalone PNG.
    pub fn export_png(&self) -> Result<Vec<u8>, String> {
        self.frame_image(self.document.current_index())?
            .to_png_bytes()
            .map_err(|error| format!("the current frame could not be encoded as PNG: {error}"))
    }

    /// Encode the document as a WebP: one frame is a still, several are animated.
    pub fn export_webp(&self, frame_duration_ms: u32) -> Result<Vec<u8>, String> {
        let images = (0..self.document.frame_count())
            .map(|index| self.frame_image(index))
            .collect::<Result<Vec<_>, _>>()?;
        if images.len() == 1 {
            return webp::encode_single(&images[0]);
        }
        webp::encode_animation(&images, frame_duration_ms.max(1))
    }

    /// Run a per-frame operation over the selection, snapshotting only on a change.
    ///
    /// Frame operations are pure and return the frame they would produce, so
    /// this can compare before mutating: a no-op (a stroke on a missing layer)
    /// leaves the undo history untouched. The selection is restored afterwards.
    fn apply_frame_ops(
        &mut self,
        all_frames: bool,
        mut operation: impl FnMut(&Frame) -> Result<Option<Frame>, palace_prop::PropError>,
    ) -> Result<bool, String> {
        let planned = self.plan_frame_ops(all_frames, &mut operation)?;
        if planned.is_empty() {
            return Ok(false);
        }
        self.commit_frames(planned);
        Ok(true)
    }

    /// Collect the frames a frame operation would replace, in document order.
    fn plan_frame_ops(
        &self,
        all_frames: bool,
        operation: &mut impl FnMut(&Frame) -> Result<Option<Frame>, palace_prop::PropError>,
    ) -> Result<Vec<(usize, Frame)>, String> {
        let targets = self.target_frames(all_frames);
        let mut planned = Vec::new();
        for index in targets {
            let Some(frame) = self.document.frame(index) else {
                continue;
            };
            let replacement = operation(frame)
                .map_err(|error| format!("the operation failed on frame {index}: {error}"))?;
            if let Some(next) = replacement {
                if next != *frame {
                    planned.push((index, next));
                }
            }
        }
        Ok(planned)
    }

    /// Run a whole-frame adjustment, snapshotting only on a change.
    fn apply_adjust(
        &mut self,
        all_frames: bool,
        mut operation: impl FnMut(&Frame) -> Result<Vec<Frame>, palace_prop::PropError>,
    ) -> Result<bool, String> {
        let planned = self.plan_frame_ops(all_frames, &mut |frame| {
            Ok(operation(frame)?.into_iter().next())
        })?;
        if planned.is_empty() {
            return Ok(false);
        }
        self.commit_frames(planned);
        Ok(true)
    }

    /// The frame indices an operation touches, selection first for a single frame.
    fn target_frames(&self, all_frames: bool) -> Vec<usize> {
        if all_frames {
            (0..self.document.frame_count()).collect()
        } else {
            vec![self.document.current_index()]
        }
    }

    /// Snapshot, then write the planned frames back, restoring the selection.
    fn commit_frames(&mut self, planned: Vec<(usize, Frame)>) {
        self.document.push_snapshot();
        self.write_frames(planned);
    }

    /// Write planned frames back without touching the undo history.
    ///
    /// Used by the drag-stroke path, which takes its single snapshot itself so a
    /// multi-segment drag does not fill the history.
    fn write_frames(&mut self, planned: Vec<(usize, Frame)>) {
        let selected = self.document.current_index();
        for (index, frame) in planned {
            if self.document.set_current(index) {
                if let Some(slot) = self.document.current_frame_mut() {
                    *slot = frame;
                }
            }
        }
        let _ = self.document.set_current(selected);
    }

    /// Bundle a `changed` flag with the post-operation state.
    fn outcome(&self, changed: bool) -> EditorOutcome {
        EditorOutcome {
            changed,
            state: self.state(),
        }
    }
}

/// The prop flag word a save request selects.
fn prop_flags(request: &SavePropRequest) -> u16 {
    let mut flags = 0u16;
    if request.head {
        flags |= palace_prop::FLAG_HEAD;
    }
    if request.ghost {
        flags |= palace_prop::FLAG_GHOST;
    }
    if request.rare {
        flags |= palace_prop::FLAG_RARE;
    }
    if request.bounce {
        flags |= palace_prop::FLAG_BOUNCE;
    }
    if request.animate {
        flags |= palace_prop::FLAG_ANIMATE;
    }
    flags
}

/// Trim a name; blank or whitespace-only becomes unnamed.
fn clean_name(name: Option<&str>) -> Option<String> {
    name.map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
}

/// Allocate a fresh id below or above every id already in My Bag.
fn allocate_prop_id(bag: &BagService, placement: SavePlacement) -> Result<u32, String> {
    let entries = bag.snapshot().entries();
    let ids = entries
        .iter()
        .filter(|entry| entry.collection == MY_BAG)
        .map(|entry| entry.id as i32);
    let allocated = match placement {
        SavePlacement::NewPropAtStart => ids.min().map_or(1, |id| id.saturating_sub(1)),
        SavePlacement::NewPropAtEnd => ids.max().map_or(1, |id| id.saturating_add(1)),
        SavePlacement::OverwriteInPlace => 0,
    };
    Ok(allocated as u32)
}

/// A one-line status for a save outcome.
fn save_message(outcome: BagOutcome, id: u32, crc: u32) -> String {
    match outcome {
        BagOutcome::Added | BagOutcome::Duplicated => {
            format!("Saved prop {id} to My Bag ({crc:#010x})")
        }
        BagOutcome::AlreadyPresent => {
            format!("Prop {id} ({crc:#010x}) is already in My Bag")
        }
        other => format!("My Bag reported {other:?} for prop {id}"),
    }
}

/// Where the open document came from, for the editor's status line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EditorOrigin {
    /// A blank canvas created in the editor.
    Blank,
    /// A clone of a bag prop; the collection is only read, never written.
    BagProp {
        /// The collection the prop came from.
        collection: String,
        /// The asset id, as the catalog lists it (unsigned view).
        id: u32,
        /// The payload CRC that completes the identity.
        crc: u32,
        /// The record's name, when it had one.
        name: Option<String>,
    },
    /// An image decoded from imported bytes.
    Image,
}

/// One layer of the selected frame, for the layer list.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EditorLayerSummary {
    /// Document-unique layer id.
    pub id: u64,
    /// Display name (`Base`, an overlay's name, ...).
    pub name: String,
    /// Pixel width.
    pub width: u32,
    /// Pixel height.
    pub height: u32,
    /// Draw opacity, as stored (clamped at composite time).
    pub opacity: f32,
    /// Whether the layer is drawn.
    pub visible: bool,
    /// Horizontal placement of the layer's top-left, in frame pixels.
    pub x: i32,
    /// Vertical placement of the layer's top-left, in frame pixels.
    pub y: i32,
    /// Clockwise rotation in degrees about the layer centre.
    pub rotation: f32,
    /// Uniform scale; `1.0` is natural size.
    pub scale: f32,
}

/// One frame of the document, for the frame strip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EditorFrameSummary {
    /// Position in the frame list (0-based).
    pub index: usize,
    /// Canvas width in pixels.
    pub width: u32,
    /// Canvas height in pixels.
    pub height: u32,
    /// How many layers the frame carries, base included.
    pub layer_count: usize,
    /// Whether this is the selected frame.
    pub selected: bool,
}

/// Everything the editor chrome reports from the Rust-side session.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EditorState {
    /// Where the document came from.
    pub origin: EditorOrigin,
    /// Canvas width in pixels (the selected frame's).
    pub width: u32,
    /// Canvas height in pixels.
    pub height: u32,
    /// How many frames the document holds.
    pub frame_count: usize,
    /// The selected frame's index.
    pub frame_index: usize,
    /// One summary per frame, in order.
    pub frames: Vec<EditorFrameSummary>,
    /// The selected frame's layers, base first.
    pub layers: Vec<EditorLayerSummary>,
    /// Whether an undo would change the document.
    pub can_undo: bool,
    /// Whether a redo would change the document.
    pub can_redo: bool,
    /// Whether a frame has been copied and can be pasted.
    pub can_paste: bool,
    /// Snapshots currently available to undo.
    pub undo_depth: usize,
    /// Snapshots currently available to redo.
    pub redo_depth: usize,
    /// Pixel bytes the bounded history currently holds.
    pub undo_bytes: usize,
    /// Whether the document has unsaved edits.
    ///
    /// A snapshot is pushed only immediately before a mutation, so an empty
    /// undo stack means the document still matches what was opened. A later
    /// save task will compare against the revision it wrote instead.
    pub dirty: bool,
}

/// The result of an operation that may or may not have changed the document.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EditorOutcome {
    /// Whether the document changed.
    pub changed: bool,
    /// The document's state after the attempt (unchanged when refused).
    pub state: EditorState,
}

// -- Guides ----------------------------------------------------------------

/// A point in canvas pixels, as the UI draws it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct GuidePointState {
    /// Horizontal coordinate; `0` is the left edge.
    pub x: i32,
    /// Vertical coordinate; `0` is the top edge.
    pub y: i32,
}

/// A straight guide line, as its two endpoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct GuideLineState {
    /// The first endpoint.
    pub from: GuidePointState,
    /// The second endpoint.
    pub to: GuidePointState,
}

/// The grid lines for one canvas.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GuideGridState {
    /// Vertical lines, left to right.
    pub vertical: Vec<GuideLineState>,
    /// Horizontal lines, top to bottom.
    pub horizontal: Vec<GuideLineState>,
}

/// The centre cross for one canvas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct GuideCentreState {
    /// The vertical centre line.
    pub vertical: GuideLineState,
    /// The horizontal centre line.
    pub horizontal: GuideLineState,
}

/// The rule-of-thirds lines for one canvas.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GuideThirdsState {
    /// The one-third and two-thirds vertical lines.
    pub vertical: Vec<GuideLineState>,
    /// The one-third and two-thirds horizontal lines.
    pub horizontal: Vec<GuideLineState>,
}

/// The safe-area rectangle for one canvas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct GuideRectState {
    /// Left edge.
    pub x: i32,
    /// Top edge.
    pub y: i32,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

/// Every guide for one canvas: grid, centre, safe area and thirds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GuideGeometryState {
    /// Canvas width the geometry was built for.
    pub width: u32,
    /// Canvas height the geometry was built for.
    pub height: u32,
    /// The grid lines.
    pub grid: GuideGridState,
    /// The centre cross.
    pub centre: GuideCentreState,
    /// The safe-area rectangle.
    pub safe_area: GuideRectState,
    /// The rule-of-thirds lines.
    pub thirds: GuideThirdsState,
}

/// The editor's six guide switches, as the webview reads and writes them.
///
/// This mirrors [`GuideToggles`] one-for-one. Field names stay snake_case so a
/// request round-trips through serde unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
pub struct GuideTogglesState {
    /// Ghost neighbouring frames.
    pub onion: bool,
    /// Draw the pixel grid.
    pub grid: bool,
    /// Snap free points to grid lines.
    pub snap_grid: bool,
    /// Snap free points to the centre cross.
    pub snap_centre: bool,
    /// Draw the safe-area rectangle.
    pub safe_area: bool,
    /// Draw the rule-of-thirds lines.
    pub thirds: bool,
}

impl From<GuideToggles> for GuideTogglesState {
    fn from(toggles: GuideToggles) -> Self {
        GuideTogglesState {
            onion: toggles.onion,
            grid: toggles.grid,
            snap_grid: toggles.snap_grid,
            snap_centre: toggles.snap_centre,
            safe_area: toggles.safe_area,
            thirds: toggles.thirds,
        }
    }
}

impl From<GuideTogglesState> for GuideToggles {
    fn from(toggles: GuideTogglesState) -> Self {
        GuideToggles {
            onion: toggles.onion,
            grid: toggles.grid,
            snap_grid: toggles.snap_grid,
            snap_centre: toggles.snap_centre,
            safe_area: toggles.safe_area,
            thirds: toggles.thirds,
        }
    }
}

/// The guide view state: which guides are on, their pitch and the canvas size.
///
/// This is presentation state, never document state. Toggling a guide pushes no
/// snapshot, so it can neither dirty the document nor be undone.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GuideState {
    /// The six switches.
    pub toggles: GuideTogglesState,
    /// Grid pitch in pixels.
    pub grid_spacing: u32,
    /// Safe-area inset per side in pixels.
    pub safe_area_inset: u32,
    /// How many frames each side onion skin ghosts.
    pub onion_radius: usize,
    /// The open canvas width in pixels (`0` when nothing is open).
    pub width: u32,
    /// The open canvas height in pixels (`0` when nothing is open).
    pub height: u32,
    /// The selected frame's index.
    pub frame_index: usize,
    /// How many frames the document holds.
    pub frame_count: usize,
}

/// One frame to ghost while onion skinning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct OnionNeighbourState {
    /// Index into the document's frame list.
    pub index: usize,
    /// Signed distance from the current frame.
    pub offset: i32,
}

/// The guide settings a session carries: switches plus the tunable pitches.
#[derive(Debug, Clone, Copy)]
struct GuideSettings {
    toggles: GuideToggles,
    grid_spacing: u32,
    safe_area_inset: u32,
    onion_radius: usize,
}

impl Default for GuideSettings {
    fn default() -> Self {
        GuideSettings {
            toggles: GuideToggles::ALL_OFF,
            grid_spacing: DEFAULT_GRID_SPACING,
            safe_area_inset: DEFAULT_SAFE_AREA_INSET,
            onion_radius: DEFAULT_ONION_RADIUS,
        }
    }
}

/// Convert a crate point into the webview's point shape.
fn guide_point(point: Point) -> GuidePointState {
    GuidePointState {
        x: point.x,
        y: point.y,
    }
}

/// Convert a crate line into the webview's line shape.
fn guide_line(line: GuideLine) -> GuideLineState {
    GuideLineState {
        from: guide_point(line.from),
        to: guide_point(line.to),
    }
}

/// Convert a crate grid into the webview's grid shape.
fn guide_grid(grid: GridLines) -> GuideGridState {
    GuideGridState {
        vertical: grid.vertical.into_iter().map(guide_line).collect(),
        horizontal: grid.horizontal.into_iter().map(guide_line).collect(),
    }
}

/// Convert a crate centre cross into the webview's shape.
fn guide_centre(centre: CentreCross) -> GuideCentreState {
    GuideCentreState {
        vertical: guide_line(centre.vertical),
        horizontal: guide_line(centre.horizontal),
    }
}

/// Convert crate thirds lines into the webview's shape.
fn guide_thirds(thirds: ThirdsLines) -> GuideThirdsState {
    GuideThirdsState {
        vertical: thirds.vertical.into_iter().map(guide_line).collect(),
        horizontal: thirds.horizontal.into_iter().map(guide_line).collect(),
    }
}

/// Convert a crate safe-area rectangle into the webview's shape.
fn guide_rect(rect: Rect) -> GuideRectState {
    GuideRectState {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
    }
}

/// Convert a whole crate geometry bundle into the webview's shape.
fn guide_geometry_state(geometry: GuideGeometry) -> GuideGeometryState {
    GuideGeometryState {
        width: geometry.width,
        height: geometry.height,
        grid: guide_grid(geometry.grid),
        centre: guide_centre(geometry.centre),
        safe_area: guide_rect(geometry.safe_area),
        thirds: guide_thirds(geometry.thirds),
    }
}

/// Run an editor operation on the blocking pool.
///
/// Decoding an image, encoding a PNG, reading a collection and the
/// whole-document copies a snapshot makes are all CPU or disk work; the webview
/// thread only ever sees the finished, typed result.
async fn editor_task<T, F>(slot: &State<'_, EditorSlot>, task: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(&EditorSlot) -> Result<T, String> + Send + 'static,
{
    let slot = slot.inner().clone();
    tauri::async_runtime::spawn_blocking(move || task(&slot))
        .await
        .map_err(|error| format!("editor task failed: {error}"))?
}

/// Start a blank editor session of the given canvas size.
#[tauri::command]
pub async fn editor_open_blank(
    slot: State<'_, EditorSlot>,
    width: u32,
    height: u32,
) -> Result<EditorState, String> {
    editor_task(&slot, move |slot| {
        let session = EditorSession::blank(width, height)?;
        let state = session.state();
        slot.set(session);
        Ok(state)
    })
    .await
}

/// Open a clone of one bag prop's pixels for editing.
///
/// Reads the named collection's `.prp`, finds the exact `(id, crc)` record and
/// decodes its blob. The collection file is never written: the original stays
/// untouched until an explicit save exists.
#[tauri::command]
pub async fn editor_open_bag_prop(
    state: State<'_, AppState>,
    slot: State<'_, EditorSlot>,
    collection: String,
    id: u32,
    crc: u32,
) -> Result<EditorState, String> {
    let bag = state.bag.clone();
    editor_task(&slot, move |slot| {
        let session = open_bag_prop(&bag, &collection, id, crc)?;
        let state = session.state();
        slot.set(session);
        Ok(state)
    })
    .await
}

/// Decode an imported image (PNG/JPG/BMP/GIF/WebP) into a new session.
#[tauri::command]
pub async fn editor_open_image(
    slot: State<'_, EditorSlot>,
    bytes: Vec<u8>,
) -> Result<EditorState, String> {
    editor_task(&slot, move |slot| {
        let session = EditorSession::from_image_bytes(&bytes)?;
        let state = session.state();
        slot.set(session);
        Ok(state)
    })
    .await
}

/// The open document's state; `null` when no session is open.
#[tauri::command]
pub fn editor_state(slot: State<'_, EditorSlot>) -> Option<EditorState> {
    slot.with_session(EditorSession::state)
}

/// The selected (composited) pixels of one frame, as PNG bytes for the canvas.
#[tauri::command]
pub async fn editor_frame_png(
    slot: State<'_, EditorSlot>,
    frame_index: usize,
) -> Result<Vec<u8>, String> {
    editor_task(&slot, move |slot| {
        let image = slot
            .with_session(|session| session.frame_image(frame_index))
            .ok_or_else(|| NO_SESSION.to_string())?;
        let image = image?;
        image
            .to_png_bytes()
            .map_err(|error| format!("frame {frame_index} could not be encoded as PNG: {error}"))
    })
    .await
}

/// Undo one edit; `changed` is false when there is nothing to undo.
#[tauri::command]
pub async fn editor_undo(slot: State<'_, EditorSlot>) -> Result<EditorOutcome, String> {
    editor_task(&slot, move |slot| {
        slot.with_session_mut(|session| Ok(session.undo()))
    })
    .await
}

/// Redo the most recently undone edit; `changed` is false when there is nothing
/// to redo.
#[tauri::command]
pub async fn editor_redo(slot: State<'_, EditorSlot>) -> Result<EditorOutcome, String> {
    editor_task(&slot, move |slot| {
        slot.with_session_mut(|session| Ok(session.redo()))
    })
    .await
}

/// Add a blank frame after the current one and select it.
#[tauri::command]
pub async fn editor_add_frame(slot: State<'_, EditorSlot>) -> Result<EditorOutcome, String> {
    editor_task(&slot, move |slot| {
        slot.with_session_mut(|session| Ok(session.add_frame()))
    })
    .await
}

/// Duplicate the frame at `index` directly after itself and select the copy.
#[tauri::command]
pub async fn editor_duplicate_frame(
    slot: State<'_, EditorSlot>,
    index: usize,
) -> Result<EditorOutcome, String> {
    editor_task(&slot, move |slot| {
        slot.with_session_mut(|session| Ok(session.duplicate_frame(index)))
    })
    .await
}

/// Delete the frame at `index`; the document's last frame cannot be removed.
#[tauri::command]
pub async fn editor_delete_frame(
    slot: State<'_, EditorSlot>,
    index: usize,
) -> Result<EditorOutcome, String> {
    editor_task(&slot, move |slot| {
        slot.with_session_mut(|session| Ok(session.delete_frame(index)))
    })
    .await
}

/// Move the frame at `from` so it sits at `to`, keeping the selection on the
/// same frame.
#[tauri::command]
pub async fn editor_move_frame(
    slot: State<'_, EditorSlot>,
    from: usize,
    to: usize,
) -> Result<EditorOutcome, String> {
    editor_task(&slot, move |slot| {
        slot.with_session_mut(|session| Ok(session.move_frame(from, to)))
    })
    .await
}

/// Select the frame at `index` for painting and the frame strip.
#[tauri::command]
pub async fn editor_select_frame(
    slot: State<'_, EditorSlot>,
    index: usize,
) -> Result<EditorOutcome, String> {
    editor_task(&slot, move |slot| {
        slot.with_session_mut(|session| Ok(session.select_frame(index)))
    })
    .await
}

/// Paint an opaque brush stroke.
#[tauri::command]
pub async fn editor_paint_stroke(
    slot: State<'_, EditorSlot>,
    request: StrokeRequest,
) -> Result<EditorOutcome, String> {
    editor_task(&slot, move |slot| {
        slot.with_session_mut(|session| session.paint_stroke(&request))
    })
    .await
}

/// Erase along a stroke (clear alpha).
#[tauri::command]
pub async fn editor_erase_stroke(
    slot: State<'_, EditorSlot>,
    request: StrokeRequest,
) -> Result<EditorOutcome, String> {
    editor_task(&slot, move |slot| {
        slot.with_session_mut(|session| session.erase_stroke(&request))
    })
    .await
}

/// Begin a pointer drag stroke (brush or eraser): its first segment and the
/// drag's single undo snapshot.
#[tauri::command]
pub async fn editor_begin_stroke(
    slot: State<'_, EditorSlot>,
    request: StrokeSessionRequest,
) -> Result<EditorOutcome, String> {
    editor_task(&slot, move |slot| {
        slot.with_session_mut(|session| session.begin_stroke(&request))
    })
    .await
}

/// Extend the in-progress pointer drag stroke; shares the begin's snapshot.
#[tauri::command]
pub async fn editor_extend_stroke(
    slot: State<'_, EditorSlot>,
    request: StrokeSessionRequest,
) -> Result<EditorOutcome, String> {
    editor_task(&slot, move |slot| {
        slot.with_session_mut(|session| session.extend_stroke(&request))
    })
    .await
}

/// Flood-fill a region.
#[tauri::command]
pub async fn editor_flood_fill(
    slot: State<'_, EditorSlot>,
    request: FloodFillRequest,
) -> Result<EditorOutcome, String> {
    editor_task(&slot, move |slot| {
        slot.with_session_mut(|session| session.flood_fill(&request))
    })
    .await
}

/// Sample a layer pixel's colour; `null` when the pixel is out of range.
#[tauri::command]
pub async fn editor_pick_color(
    slot: State<'_, EditorSlot>,
    layer: usize,
    x: u32,
    y: u32,
) -> Result<Option<[u8; 4]>, String> {
    editor_task(&slot, move |slot| {
        Ok(slot
            .with_session(|session| session.pick_color(layer, x, y))
            .flatten())
    })
    .await
}

/// Clear a frame's pixels to transparent.
#[tauri::command]
pub async fn editor_clear_frame(
    slot: State<'_, EditorSlot>,
    request: FrameScopeRequest,
) -> Result<EditorOutcome, String> {
    editor_task(&slot, move |slot| {
        slot.with_session_mut(|session| session.clear_frame(request.all_frames))
    })
    .await
}

/// Flip the selected frame, or every frame, horizontally or vertically.
#[tauri::command]
pub async fn editor_flip(
    slot: State<'_, EditorSlot>,
    request: FlipRequest,
) -> Result<EditorOutcome, String> {
    editor_task(&slot, move |slot| {
        slot.with_session_mut(|session| session.flip(&request))
    })
    .await
}

/// Rotate the selected frame, or every frame, by a quarter turn.
#[tauri::command]
pub async fn editor_rotate(
    slot: State<'_, EditorSlot>,
    request: RotateRequest,
) -> Result<EditorOutcome, String> {
    editor_task(&slot, move |slot| {
        slot.with_session_mut(|session| session.rotate(&request))
    })
    .await
}

/// Crop the selected frame, or every frame, to a rectangle.
#[tauri::command]
pub async fn editor_crop(
    slot: State<'_, EditorSlot>,
    request: CropRequest,
) -> Result<EditorOutcome, String> {
    editor_task(&slot, move |slot| {
        slot.with_session_mut(|session| session.crop(&request))
    })
    .await
}

/// Crop the selected frame, or every frame, to its largest centred square.
#[tauri::command]
pub async fn editor_crop_square(
    slot: State<'_, EditorSlot>,
    request: FrameScopeRequest,
) -> Result<EditorOutcome, String> {
    editor_task(&slot, move |slot| {
        slot.with_session_mut(|session| session.crop_square(request.all_frames))
    })
    .await
}

/// Shape-crop the selected frame, or every frame.
#[tauri::command]
pub async fn editor_shape_crop(
    slot: State<'_, EditorSlot>,
    request: ShapeCropRequest,
) -> Result<EditorOutcome, String> {
    editor_task(&slot, move |slot| {
        slot.with_session_mut(|session| session.shape_crop(&request))
    })
    .await
}

/// Resize the selected frame, or every frame.
#[tauri::command]
pub async fn editor_resize(
    slot: State<'_, EditorSlot>,
    request: ResizeRequest,
) -> Result<EditorOutcome, String> {
    editor_task(&slot, move |slot| {
        slot.with_session_mut(|session| session.resize(&request))
    })
    .await
}

/// Apply brightness and contrast.
#[tauri::command]
pub async fn editor_adjust_brightness_contrast(
    slot: State<'_, EditorSlot>,
    request: BrightnessContrastRequest,
) -> Result<EditorOutcome, String> {
    editor_task(&slot, move |slot| {
        slot.with_session_mut(|session| session.adjust_brightness_contrast(&request))
    })
    .await
}

/// Apply brightness, contrast and saturation.
#[tauri::command]
pub async fn editor_adjust_colour(
    slot: State<'_, EditorSlot>,
    request: ColourAdjustRequest,
) -> Result<EditorOutcome, String> {
    editor_task(&slot, move |slot| {
        slot.with_session_mut(|session| session.adjust_colour(&request))
    })
    .await
}

/// Remove a chroma-key background.
#[tauri::command]
pub async fn editor_remove_background(
    slot: State<'_, EditorSlot>,
    request: RemoveBackgroundRequest,
) -> Result<EditorOutcome, String> {
    editor_task(&slot, move |slot| {
        slot.with_session_mut(|session| session.remove_background(&request))
    })
    .await
}

/// Sample a pixel's colour as a chroma key; `null` when it is transparent.
#[tauri::command]
pub async fn editor_sample_key_color(
    slot: State<'_, EditorSlot>,
    layer: usize,
    x: u32,
    y: u32,
) -> Result<Option<[u8; 3]>, String> {
    editor_task(&slot, move |slot| {
        Ok(slot
            .with_session(|session| session.sample_key_color(layer, x, y))
            .flatten())
    })
    .await
}

/// Copy a frame into the editor clipboard.
#[tauri::command]
pub async fn editor_copy_frame(
    slot: State<'_, EditorSlot>,
    index: usize,
) -> Result<EditorOutcome, String> {
    editor_task(&slot, move |slot| {
        slot.with_session_mut(|session| Ok(session.copy_frame(index)))
    })
    .await
}

/// Paste the copied frame after the current frame.
#[tauri::command]
pub async fn editor_paste_frame(slot: State<'_, EditorSlot>) -> Result<EditorOutcome, String> {
    editor_task(&slot, move |slot| {
        slot.with_session_mut(|session| Ok(session.paste_frame()))
    })
    .await
}

/// Save the current single-frame prop into My Bag.
///
/// A multi-frame document is refused with [`ANIMATED_SAVE_REFUSED`]. On success
/// the shared bag snapshot is rebuilt, so the browser sees the new record.
#[tauri::command]
pub async fn editor_save_prop(
    state: State<'_, AppState>,
    slot: State<'_, EditorSlot>,
    bag_slot: State<'_, BagSlot>,
    request: SavePropRequest,
) -> Result<EditorSaveResult, String> {
    let bag = state.bag.clone();
    let editor = slot.inner().clone();
    let bag_slot = bag_slot.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let result = editor.with_session_mut(|session| session.save_to_bag(&bag, &request))?;
        bag_slot.set(Arc::new(bag.snapshot()));
        Ok::<EditorSaveResult, String>(result)
    })
    .await
    .map_err(|error| format!("editor task failed: {error}"))?
}

/// Export the current frame as a standalone PNG.
#[tauri::command]
pub async fn editor_export_png(slot: State<'_, EditorSlot>) -> Result<Vec<u8>, String> {
    editor_task(&slot, move |slot| {
        slot.with_session(EditorSession::export_png)
            .unwrap_or_else(|| Err(NO_SESSION.to_string()))
    })
    .await
}

/// Export the document as a WebP: a still, or an animation when multi-frame.
#[tauri::command]
pub async fn editor_export_webp(
    slot: State<'_, EditorSlot>,
    frame_duration_ms: u32,
) -> Result<Vec<u8>, String> {
    editor_task(&slot, move |slot| {
        slot.with_session(|session| session.export_webp(frame_duration_ms))
            .unwrap_or_else(|| Err(NO_SESSION.to_string()))
    })
    .await
}

/// Rasterize text and add it as a new layer.
#[tauri::command]
pub async fn editor_add_text(
    slot: State<'_, EditorSlot>,
    request: TextLayerRequest,
) -> Result<EditorOutcome, String> {
    editor_task(&slot, move |slot| {
        slot.with_session_mut(|session| session.add_text_layer(&request))
    })
    .await
}

/// Decode an image and add it as a draggable overlay layer.
#[tauri::command]
pub async fn editor_import_overlay(
    slot: State<'_, EditorSlot>,
    bytes: Vec<u8>,
    name: Option<String>,
) -> Result<EditorOutcome, String> {
    editor_task(&slot, move |slot| {
        let image = decode_rgba_image(&bytes)?;
        slot.with_session_mut(|session| session.add_overlay_layer(image, name))
    })
    .await
}

/// Apply a partial update (name, opacity, visibility, transform) to one layer.
#[tauri::command]
pub async fn editor_update_layer(
    slot: State<'_, EditorSlot>,
    request: LayerUpdateRequest,
) -> Result<EditorOutcome, String> {
    editor_task(&slot, move |slot| {
        slot.with_session_mut(|session| session.update_layer(&request))
    })
    .await
}

/// Move a layer within the selected frame; the base layer stays at index 0.
#[tauri::command]
pub async fn editor_move_layer(
    slot: State<'_, EditorSlot>,
    from: usize,
    to: usize,
) -> Result<EditorOutcome, String> {
    editor_task(&slot, move |slot| {
        slot.with_session_mut(|session| Ok(session.move_layer(from, to)))
    })
    .await
}

/// Delete an overlay or text layer; the base layer cannot be removed.
#[tauri::command]
pub async fn editor_delete_layer(
    slot: State<'_, EditorSlot>,
    index: usize,
) -> Result<EditorOutcome, String> {
    editor_task(&slot, move |slot| {
        slot.with_session_mut(|session| Ok(session.delete_layer(index)))
    })
    .await
}

// -- Guides ----------------------------------------------------------------

/// The guide switches, pitches and canvas size for the open document.
#[tauri::command]
pub async fn editor_guide_state(slot: State<'_, EditorSlot>) -> Result<GuideState, String> {
    editor_task(&slot, move |slot| {
        slot.with_session(EditorSession::guide_state)
            .ok_or_else(|| NO_SESSION.to_string())
    })
    .await
}

/// Replace the guide switches; view-only, so the document is untouched.
#[tauri::command]
pub async fn editor_set_guide_toggles(
    slot: State<'_, EditorSlot>,
    toggles: GuideTogglesState,
) -> Result<GuideState, String> {
    editor_task(&slot, move |slot| {
        slot.with_session_mut(|session| Ok(session.set_guide_toggles(toggles)))
    })
    .await
}

/// Set the onion-skin radius in frames each side; view-only.
#[tauri::command]
pub async fn editor_set_onion_radius(
    slot: State<'_, EditorSlot>,
    radius: usize,
) -> Result<GuideState, String> {
    editor_task(&slot, move |slot| {
        slot.with_session_mut(|session| Ok(session.set_onion_radius(radius)))
    })
    .await
}

/// Every guide line for the open canvas.
#[tauri::command]
pub async fn editor_guide_geometry(
    slot: State<'_, EditorSlot>,
) -> Result<GuideGeometryState, String> {
    editor_task(&slot, move |slot| {
        slot.with_session(EditorSession::guide_geometry)
            .ok_or_else(|| NO_SESSION.to_string())?
            .ok_or_else(|| "the editor has no current frame".to_string())
    })
    .await
}

/// The frames to ghost around the current one, nearest first.
#[tauri::command]
pub async fn editor_onion_neighbours(
    slot: State<'_, EditorSlot>,
) -> Result<Vec<OnionNeighbourState>, String> {
    editor_task(&slot, move |slot| {
        slot.with_session(EditorSession::onion_neighbours)
            .ok_or_else(|| NO_SESSION.to_string())
    })
    .await
}

/// Snap a point through the active guide switches for the open canvas.
#[tauri::command]
pub async fn editor_snap_point(
    slot: State<'_, EditorSlot>,
    x: i32,
    y: i32,
) -> Result<GuidePointState, String> {
    editor_task(&slot, move |slot| {
        slot.with_session(|session| session.snap_point(x, y))
            .ok_or_else(|| NO_SESSION.to_string())
    })
    .await
}

/// Read one bag collection's `.prp`, find `(id, crc)` and clone its pixels into
/// a new editor session. Nothing is written, so the file is byte-identical
/// before and after the open.
fn open_bag_prop(
    bag: &BagService,
    collection: &str,
    id: u32,
    crc: u32,
) -> Result<EditorSession, String> {
    let info = bag
        .collections()?
        .into_iter()
        .find(|info| info.name == collection)
        .ok_or_else(|| format!("the bag has no collection named {collection:?}"))?;
    let bytes = std::fs::read(&info.path)
        .map_err(|error| format!("could not read {}: {error}", info.path))?;
    let roster =
        Roster::parse(&bytes).map_err(|error| format!("could not parse {}: {error}", info.path))?;
    let key = PropKey::new(id as i32, crc);
    let blob = roster
        .blob_for(key)
        .ok_or_else(|| format!("{collection:?} does not hold prop {id} with crc {crc:#010x}"))?;
    let prop = palace_prop::decode(blob)
        .map_err(|error| format!("prop {id} in {collection:?} could not be decoded: {error}"))?;
    let name = roster
        .record_for(key)
        .and_then(|record| record.name.clone());
    Ok(EditorSession::from_bag_prop(
        &prop,
        collection.to_string(),
        id,
        crc,
        name,
    ))
}

/// Decode one imported image into an editor document.
///
/// The dimensions are read and validated *before* the pixels are decoded, so an
/// oversized image is refused from its header rather than after allocating its
/// buffer. The result is RGBA8 with the file's alpha preserved.
fn decode_image(bytes: &[u8]) -> Result<EditorDocument, String> {
    let image = decode_rgba_image(bytes)?;
    Ok(EditorDocument::from_prop_image(&image))
}

/// Decode one imported image (PNG/JPG/BMP/GIF/WebP) into RGBA pixels.
///
/// Dimensions are validated from the header before the pixels are allocated, so
/// an oversized image is refused cheaply. The result keeps the file's alpha.
fn decode_rgba_image(bytes: &[u8]) -> Result<PropImage, String> {
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| format!("could not read the image header: {error}"))?;
    let format = reader.format().ok_or_else(|| {
        "the image format is not recognised (the editor decodes PNG, JPG, BMP, GIF and WebP)"
            .to_string()
    })?;
    let (width, height) = reader
        .into_dimensions()
        .map_err(|error| format!("could not read the image dimensions: {error}"))?;
    validate_canvas(width, height)?;
    let decoded = image::load_from_memory_with_format(bytes, format)
        .map_err(|error| format!("could not decode the {format:?} image: {error}"))?;
    let rgba = decoded.to_rgba8();
    PropImage::from_rgba(rgba.width(), rgba.height(), rgba.into_raw())
        .map_err(|error| format!("could not build the image: {error}"))
}

/// Refuse canvases that are empty, beyond [`MAX_DIMENSION`] per side, or beyond
/// the [`MAX_PIXELS`] budget — the same limits the model enforces.
fn validate_canvas(width: u32, height: u32) -> Result<(), String> {
    if width == 0 || height == 0 {
        return Err(format!("the canvas is empty ({width}x{height})"));
    }
    if width > MAX_DIMENSION as u32 || height > MAX_DIMENSION as u32 {
        return Err(format!(
            "the canvas {width}x{height} exceeds the {MAX_DIMENSION}-pixel-per-side limit"
        ));
    }
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(format!(
            "the canvas {width}x{height} exceeds the {MAX_PIXELS}-pixel budget"
        ));
    }
    Ok(())
}

/// Text rasterization for the text tool, at the app layer.
///
/// `palace_prop::editor` takes RGBA in and gives RGBA out, so the face and the
/// rasterizer live here, never in the dependency-light crate. The face is the
/// same embedded Liberation Sans Bold (SIL OFL 1.1) that `palace-render` bundles
/// for name tags, and `fontdue` is the lightweight rasterizer already in the
/// workspace graph. Rendering is deterministic: one request always yields the
/// same bytes, so a composite can be compared or hashed directly.
///
/// Drop shadow and outline are drawn, not blurred: the outline is a Max-filter
/// dilation of the glyph coverage and the shadow is that silhouette offset by one
/// pixel, the same approximation `palace-render` documents for its own text.
mod text {
    use fontdue::{Font, FontSettings};
    use palace_prop::PropImage;

    /// The embedded face, shared with `palace-render`'s glyph module.
    const FONT_TTF: &[u8] =
        include_bytes!("../../crates/palace-render/assets/LiberationSans-Bold.ttf");

    /// Font sizes are clamped to this range before rasterizing.
    const MIN_FONT_PX: f32 = 4.0;
    const MAX_FONT_PX: f32 = 256.0;

    /// Drop-shadow displacement, in pixels, and its alpha.
    const SHADOW_OFFSET: i32 = 1;
    const SHADOW_ALPHA: f32 = 0.6;

    /// Outline dilation radius, in pixels.
    const OUTLINE_RADIUS: i32 = 1;

    /// The parsed face, parsed at most once for the life of the process.
    fn font() -> Option<&'static Font> {
        use std::sync::OnceLock;
        static FONT: OnceLock<Option<Font>> = OnceLock::new();
        FONT.get_or_init(|| Font::from_bytes(FONT_TTF, FontSettings::default()).ok())
            .as_ref()
    }

    /// One rasterized glyph: its advance, ink box and 8-bit coverage mask.
    struct Glyph {
        advance: f32,
        xmin: i32,
        ymin: i32,
        width: usize,
        height: usize,
        coverage: Vec<u8>,
    }

    /// Rasterize one character at `px`.
    fn rasterize(font: &Font, character: char, px: f32) -> Glyph {
        let (metrics, coverage) = font.rasterize(character, px);
        Glyph {
            advance: metrics.advance_width,
            xmin: metrics.xmin,
            ymin: metrics.ymin,
            width: metrics.width,
            height: metrics.height,
            coverage,
        }
    }

    /// The advance width of `text` at `px`.
    fn measure(font: &Font, text: &str, px: f32) -> f32 {
        text.chars()
            .map(|character| font.metrics(character, px).advance_width)
            .sum()
    }

    /// How many leading characters of `text` fit inside `width`.
    fn fit_chars(font: &Font, text: &str, px: f32, width: f32) -> usize {
        let mut cursor = 0.0;
        for (index, character) in text.chars().enumerate() {
            let advance = font.metrics(character, px).advance_width;
            if index > 0 && cursor + advance > width {
                return index;
            }
            cursor += advance;
        }
        text.chars().count()
    }

    /// Greedy word wrap of one paragraph; a word wider than the limit is broken.
    fn wrap_paragraph(font: &Font, paragraph: &str, px: f32, width: f32, lines: &mut Vec<String>) {
        let mut current = String::new();
        for word in paragraph.split_whitespace() {
            let candidate = if current.is_empty() {
                word.to_string()
            } else {
                format!("{current} {word}")
            };
            if current.is_empty() || measure(font, &candidate, px) <= width {
                current = candidate;
            } else {
                lines.push(std::mem::take(&mut current));
                current = word.to_string();
            }
            while measure(font, &current, px) > width {
                let split = fit_chars(font, &current, px, width);
                if split == 0 || split >= current.chars().count() {
                    break;
                }
                let tail = current.chars().skip(split).collect::<String>();
                current = current.chars().take(split).collect();
                lines.push(std::mem::take(&mut current));
                current = tail;
            }
        }
        if !current.is_empty() {
            lines.push(current);
        }
    }

    /// Split into lines: on newlines always, and to `wrap_width` when given.
    fn layout_lines(font: &Font, text: &str, px: f32, wrap_width: Option<u32>) -> Vec<String> {
        let mut lines = Vec::new();
        for paragraph in text.split('\n') {
            match wrap_width {
                Some(width) if width > 0 => {
                    wrap_paragraph(font, paragraph, px, width as f32, &mut lines);
                }
                _ => lines.push(paragraph.to_string()),
            }
        }
        if lines.is_empty() {
            lines.push(String::new());
        }
        lines
    }

    /// Paint one glyph's coverage into `mask`, maximizing where glyphs overlap.
    fn paint(mask: &mut [u8], width: usize, height: usize, left: i32, top: i32, glyph: &Glyph) {
        for row in 0..glyph.height {
            let y = top + row as i32;
            if y < 0 || y as usize >= height {
                continue;
            }
            for col in 0..glyph.width {
                let x = left + col as i32;
                if x < 0 || x as usize >= width {
                    continue;
                }
                let coverage = glyph.coverage[row * glyph.width + col];
                if coverage == 0 {
                    continue;
                }
                let at = y as usize * width + x as usize;
                mask[at] = mask[at].max(coverage);
            }
        }
    }

    /// Dilate `mask` over the integer offsets within `radius` (a Max filter).
    fn dilate(mask: &[u8], width: usize, height: usize, radius: i32) -> Vec<u8> {
        let mut out = vec![0u8; mask.len()];
        for y in 0..height {
            for x in 0..width {
                let mut best = 0u8;
                for dy in -radius..=radius {
                    for dx in -radius..=radius {
                        if dx * dx + dy * dy > radius * radius {
                            continue;
                        }
                        let sx = x as i32 + dx;
                        let sy = y as i32 + dy;
                        if sx < 0 || sy < 0 || sx as usize >= width || sy as usize >= height {
                            continue;
                        }
                        best = best.max(mask[sy as usize * width + sx as usize]);
                    }
                }
                out[y * width + x] = best;
            }
        }
        out
    }

    /// A mask sample at `(x, y)`, or `0` when out of bounds.
    fn sample(mask: &[u8], width: usize, height: usize, x: i32, y: i32) -> u8 {
        if x < 0 || y < 0 || x as usize >= width || y as usize >= height {
            return 0;
        }
        mask[y as usize * width + x as usize]
    }

    /// Composite a solid `color` over `dst` with straight-alpha source-over.
    fn over(dst: &mut [u8; 4], color: [u8; 3], alpha: f32) {
        let sa = alpha.clamp(0.0, 1.0);
        if sa <= 0.0 {
            return;
        }
        let da = f32::from(dst[3]) / 255.0;
        let oa = sa + da * (1.0 - sa);
        if oa <= 0.0 {
            *dst = [0, 0, 0, 0];
            return;
        }
        for channel in 0..3 {
            let src = f32::from(color[channel]) / 255.0;
            let below = f32::from(dst[channel]) / 255.0;
            dst[channel] = (((src * sa + below * da * (1.0 - sa)) / oa) * 255.0)
                .round()
                .clamp(0.0, 255.0) as u8;
        }
        dst[3] = (oa * 255.0).round().clamp(0.0, 255.0) as u8;
    }

    /// Black or white, whichever contrasts with the fill.
    fn contrasting(color: [u8; 4]) -> [u8; 3] {
        let luma =
            0.299 * f32::from(color[0]) + 0.587 * f32::from(color[1]) + 0.114 * f32::from(color[2]);
        if luma > 128.0 {
            [0, 0, 0]
        } else {
            [255, 255, 255]
        }
    }

    /// A layer name derived from the text: `Text: <head>`.
    pub(super) fn layer_name(text: &str) -> String {
        let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if flat.is_empty() {
            return "Text".to_string();
        }
        let head: String = flat.chars().take(20).collect();
        if flat.chars().count() > 20 {
            format!("Text: {head}…")
        } else {
            format!("Text: {head}")
        }
    }

    /// Rasterize `text` into an RGBA image ready to become a layer.
    ///
    /// `wrap_width` is the frame width when word-wrap is on, so a wrapped block
    /// is exactly as wide as the prop. `shadow` and `outline` are baked into the
    /// image; position, rotation and opacity stay in the layer transform.
    pub(super) fn render(
        text: &str,
        color: [u8; 4],
        size: f32,
        shadow: bool,
        outline: bool,
        wrap_width: Option<u32>,
    ) -> Result<PropImage, String> {
        let font =
            font().ok_or_else(|| "the embedded text face could not be loaded".to_string())?;
        let px = size.clamp(MIN_FONT_PX, MAX_FONT_PX);
        let metrics = font
            .horizontal_line_metrics(px)
            .ok_or_else(|| "the embedded text face reported no line metrics".to_string())?;
        let ascent = metrics.ascent.max(px * 0.5);
        let descent = metrics.descent.min(0.0);
        let line_height = (ascent - descent).max(1.0).ceil();

        let lines = layout_lines(font, text, px, wrap_width);
        let outline_radius = if outline { OUTLINE_RADIUS } else { 0 };
        let shadow_offset = if shadow { SHADOW_OFFSET } else { 0 };
        let pad = outline_radius.max(shadow_offset) + 1;

        let ink_width = lines
            .iter()
            .map(|line| measure(font, line, px))
            .fold(0.0f32, f32::max);
        let content_width = match wrap_width {
            Some(width) if width > 0 => width as f32,
            _ => ink_width.ceil().max(1.0),
        };
        let width = content_width.ceil() as i64 + i64::from(pad) * 2;
        let height = (lines.len() as f32 * line_height).ceil() as i64 + i64::from(pad) * 2;
        let width =
            u32::try_from(width.max(1)).map_err(|_| "the text block is too wide".to_string())?;
        let height =
            u32::try_from(height.max(1)).map_err(|_| "the text block is too tall".to_string())?;
        let (w, h) = (width as usize, height as usize);

        let mut mask = vec![0u8; w * h];
        for (index, line) in lines.iter().enumerate() {
            let origin_y = pad + (index as f32 * line_height) as i32;
            let mut cursor = pad as f32;
            for character in line.chars() {
                let glyph = rasterize(font, character, px);
                if glyph.width > 0 && glyph.height > 0 {
                    let left = cursor.floor() as i32 + glyph.xmin;
                    let top = origin_y
                        + (ascent - (glyph.ymin + glyph.height as i32) as f32).floor() as i32;
                    paint(&mut mask, w, h, left, top, &glyph);
                }
                cursor += glyph.advance;
            }
        }

        let outline_mask = if outline {
            dilate(&mask, w, h, outline_radius)
        } else {
            Vec::new()
        };
        let silhouette: &[u8] = if outline { &outline_mask } else { &mask };
        let fill_alpha = f32::from(color[3]) / 255.0;
        let outline_color = contrasting(color);

        let mut rgba = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let mut dst = [0u8, 0, 0, 0];
                if shadow {
                    let coverage = sample(
                        silhouette,
                        w,
                        h,
                        x as i32 - shadow_offset,
                        y as i32 - shadow_offset,
                    );
                    over(
                        &mut dst,
                        [0, 0, 0],
                        f32::from(coverage) / 255.0 * SHADOW_ALPHA,
                    );
                }
                if outline {
                    let coverage = outline_mask[y * w + x];
                    over(&mut dst, outline_color, f32::from(coverage) / 255.0);
                }
                let coverage = mask[y * w + x];
                if coverage > 0 && fill_alpha > 0.0 {
                    over(
                        &mut dst,
                        [color[0], color[1], color[2]],
                        f32::from(coverage) / 255.0 * fill_alpha,
                    );
                }
                let at = (y * w + x) * 4;
                rgba[at..at + 4].copy_from_slice(&dst);
            }
        }

        PropImage::from_rgba(width, height, rgba)
            .map_err(|error| format!("the text layer could not be built: {error}"))
    }
}

/// The WebP container writer: stills through the `image` encoder, animation by
/// muxing those lossless frame bitstreams into a VP8X/ANIM/ANMF container.
///
/// `image` (via `image-webp`) decodes WebP and animation but only encodes
/// single-frame lossless WebP, so an animated file is built here from the
/// already-present encoder rather than by adding a second, native encoder
/// dependency. Each frame is encoded standalone, its `VP8L` chunk lifted out,
/// and wrapped in an `ANMF` chunk. The container layout follows the WebP
/// container spec and is verified by decoding the result.
mod webp {
    use super::PropImage;

    /// Encode one RGBA image as a standalone lossless WebP file.
    pub fn encode_single(image: &PropImage) -> Result<Vec<u8>, String> {
        use image::codecs::webp::WebPEncoder;
        let mut out = Vec::new();
        WebPEncoder::new_lossless(&mut out)
            .encode(
                image.as_rgba(),
                image.width(),
                image.height(),
                image::ExtendedColorType::Rgba8,
            )
            .map_err(|error| format!("the frame could not be encoded as WebP: {error}"))?;
        Ok(out)
    }

    /// Encode several frames as one animated WebP, each shown `duration_ms`.
    pub fn encode_animation(frames: &[PropImage], duration_ms: u32) -> Result<Vec<u8>, String> {
        if frames.is_empty() {
            return Err("there are no frames to export".to_string());
        }
        let canvas_width = frames.iter().map(PropImage::width).max().unwrap_or(1);
        let canvas_height = frames.iter().map(PropImage::height).max().unwrap_or(1);
        let mut chunks = Vec::new();
        push_chunk(
            &mut chunks,
            b"VP8X",
            &vp8x_data(canvas_width, canvas_height),
        );
        push_chunk(&mut chunks, b"ANIM", &anim_data());
        for frame in frames {
            let file = encode_single(frame)?;
            let payload = vp8l_payload(&file)?;
            let mut data = Vec::with_capacity(24 + payload.len());
            data.extend_from_slice(&u24(0));
            data.extend_from_slice(&u24(0));
            data.extend_from_slice(&u24(frame.width().saturating_sub(1)));
            data.extend_from_slice(&u24(frame.height().saturating_sub(1)));
            data.extend_from_slice(&u24(duration_ms));
            data.push(0x03);
            push_chunk(&mut data, b"VP8L", payload);
            push_chunk(&mut chunks, b"ANMF", &data);
        }
        let mut out = Vec::with_capacity(12 + chunks.len());
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&((4 + chunks.len()) as u32).to_le_bytes());
        out.extend_from_slice(b"WEBP");
        out.extend_from_slice(&chunks);
        Ok(out)
    }

    /// The `VP8X` payload: canvas size and the animation + alpha flags.
    fn vp8x_data(width: u32, height: u32) -> [u8; 10] {
        let mut data = [0u8; 10];
        data[0] = 0x02 | 0x10;
        data[4..7].copy_from_slice(&u24(width.saturating_sub(1)));
        data[7..10].copy_from_slice(&u24(height.saturating_sub(1)));
        data
    }

    /// The `ANIM` payload: transparent background, loop forever.
    fn anim_data() -> [u8; 6] {
        [0, 0, 0, 0, 0, 0]
    }

    /// Append a RIFF chunk, padded to an even length.
    fn push_chunk(out: &mut Vec<u8>, tag: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(tag);
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(data);
        if !data.len().is_multiple_of(2) {
            out.push(0);
        }
    }

    /// A little-endian 24-bit integer.
    fn u24(value: u32) -> [u8; 3] {
        let bytes = value.to_le_bytes();
        [bytes[0], bytes[1], bytes[2]]
    }

    /// Lift the `VP8L` frame sub-chunk out of a standalone WebP file.
    fn vp8l_payload(file: &[u8]) -> Result<&[u8], String> {
        if file.len() < 12 || &file[0..4] != b"RIFF" || &file[8..12] != b"WEBP" {
            return Err("the WebP encoder produced an unexpected file".to_string());
        }
        let mut position = 12;
        while position + 8 <= file.len() {
            let tag = &file[position..position + 4];
            let size = u32::from_le_bytes([
                file[position + 4],
                file[position + 5],
                file[position + 6],
                file[position + 7],
            ]) as usize;
            let start = position + 8;
            let end = start
                .checked_add(size)
                .filter(|end| *end <= file.len())
                .ok_or_else(|| "the WebP encoder produced a truncated file".to_string())?;
            if tag == b"VP8L" {
                return Ok(&file[start..end]);
            }
            position = end + (size & 1);
        }
        Err("the WebP encoder produced no VP8L frame".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use palace_prop::bag_folder::BagContext;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A 2x2 lossless WebP with alpha, generated once and kept as bytes: the
    /// `image` crate decodes WebP but does not encode it, so WebP support needs
    /// real input rather than a round trip.
    const WEBP_2X2: [u8; 60] = [
        0x52, 0x49, 0x46, 0x46, 0x34, 0x00, 0x00, 0x00, 0x57, 0x45, 0x42, 0x50, 0x56, 0x50, 0x38,
        0x4c, 0x27, 0x00, 0x00, 0x00, 0x2f, 0x01, 0x40, 0x00, 0x10, 0x1f, 0x30, 0xff, 0x02, 0x82,
        0x22, 0xff, 0x47, 0x13, 0x10, 0x14, 0xf9, 0x3f, 0x9a, 0x80, 0xa0, 0xe8, 0xba, 0xe5, 0x02,
        0xec, 0xa6, 0x82, 0x9a, 0xb6, 0x0d, 0x58, 0xfc, 0x26, 0x1d, 0x11, 0xfd, 0x8f, 0x03, 0x00,
    ];

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// A temp directory that removes itself, so a test never writes to the
    /// real bag folder.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "palace-app-editor-{tag}-{}-{n}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("create temp dir");
            TempDir(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A bag service over a synthetic, empty bag folder.
    fn bag_service(root: &Path) -> BagService {
        BagService::open(BagContext {
            bag_root: Some(root.to_path_buf()),
            home: Some(root.to_path_buf()),
        })
    }

    /// A real S20 prop blob (header + compressed payload) and its payload CRC.
    /// Alpha 0 stays transparent and 255 stays opaque through S20 quantisation;
    /// the 128 alpha lands at the nearest quantised step.
    fn prop_blob() -> (Vec<u8>, u32) {
        let image = PropImage::from_rgba(
            2,
            2,
            vec![
                255, 0, 0, 255, // opaque red
                0, 255, 0, 128, // half-alpha green
                0, 0, 255, 0, // transparent blue
                255, 255, 255, 255, // opaque white
            ],
        )
        .expect("a 2x2 RGBA image");
        let blob = palace_prop::encode_s20_blob(&image, 0, 0, 0).expect("the prop encodes");
        let crc = palace_prop::asset_crc(&blob[palace_prop::HEADER_LEN..]);
        (blob, crc)
    }

    fn frame_png(session: &EditorSession, index: usize) -> Vec<u8> {
        session
            .frame_image(index)
            .expect("the frame exists")
            .to_png_bytes()
            .expect("the frame encodes as PNG")
    }

    /// One segment of a pointer drag stroke.
    fn drag_segment(mode: StrokeMode, from: (i32, i32), to: (i32, i32)) -> StrokeSessionRequest {
        StrokeSessionRequest {
            mode,
            stroke: StrokeRequest {
                from_x: from.0,
                from_y: from.1,
                to_x: to.0,
                to_y: to.1,
                color: [255, 0, 0, 255],
                size: 1,
                layer: 0,
                all_frames: false,
            },
        }
    }

    #[test]
    fn a_pointer_drag_is_one_undo_step_however_many_segments_it_has() {
        let mut session = EditorSession::blank(6, 3).expect("6x3 is a valid canvas");

        let begun = session
            .begin_stroke(&drag_segment(StrokeMode::Brush, (1, 1), (1, 1)))
            .expect("the opening segment paints");
        assert!(begun.changed);
        assert_eq!(begun.state.undo_depth, 1, "the drag takes one snapshot");

        for x in 2..=5 {
            let step = session
                .extend_stroke(&drag_segment(StrokeMode::Brush, (x - 1, 1), (x, 1)))
                .expect("an extension paints");
            assert!(step.changed);
            assert_eq!(step.state.undo_depth, 1, "extensions reuse the snapshot");
        }

        let base = session
            .document
            .current_frame()
            .and_then(|frame| frame.base())
            .expect("the frame has a base layer")
            .image();
        for x in 1..=5 {
            assert_eq!(
                base.pixel(x, 1),
                Some([255, 0, 0, 255]),
                "pixel {x} is painted"
            );
        }

        let undone = session.undo();
        assert!(undone.changed);
        assert_eq!(
            undone.state.undo_depth, 0,
            "one undo removes the whole drag"
        );
    }

    #[test]
    fn a_drag_takes_its_snapshot_on_the_first_real_change() {
        let mut session = EditorSession::blank(3, 1).expect("3x1 is a valid canvas");

        // Erasing a transparent canvas changes nothing, so the drag must not
        // have snapshotted yet.
        let begun = session
            .begin_stroke(&drag_segment(StrokeMode::Erase, (0, 0), (0, 0)))
            .expect("the opening segment runs");
        assert!(!begun.changed);
        assert_eq!(
            begun.state.undo_depth, 0,
            "a no-op opening takes no snapshot"
        );

        // The first segment that changes a pixel takes the drag's single snapshot.
        let painted = session
            .extend_stroke(&drag_segment(StrokeMode::Brush, (0, 0), (2, 0)))
            .expect("the first real change paints");
        assert!(painted.changed);
        assert_eq!(painted.state.undo_depth, 1);

        let unchanged = session
            .extend_stroke(&drag_segment(StrokeMode::Brush, (2, 0), (2, 0)))
            .expect("re-painting the same pixel is a no-op");
        assert!(!unchanged.changed);
        assert_eq!(unchanged.state.undo_depth, 1, "no second snapshot");
    }

    #[test]
    fn a_stroke_session_request_deserializes_from_its_wire_shape() {
        let json = r#"{"mode":"erase","fromX":1,"fromY":2,"toX":3,"toY":4,
                       "color":[10,20,30,255],"size":5,"layer":1,"allFrames":true}"#;
        let request: StrokeSessionRequest =
            serde_json::from_str(json).expect("the flattened wire shape deserializes");
        assert_eq!(request.mode, StrokeMode::Erase);
        assert_eq!(
            (
                request.stroke.from_x,
                request.stroke.from_y,
                request.stroke.to_x,
                request.stroke.to_y
            ),
            (1, 2, 3, 4)
        );
        assert_eq!(request.stroke.color, [10, 20, 30, 255]);
        assert_eq!(request.stroke.size, 5);
        assert_eq!(request.stroke.layer, 1);
        assert!(request.stroke.all_frames);

        // `mode` also defaults to the brush when the field is absent.
        let without_mode = json.replacen(r#""mode":"erase","#, "", 1);
        let defaulted: StrokeSessionRequest =
            serde_json::from_str(&without_mode).expect("a stroke without a mode deserializes");
        assert_eq!(defaulted.mode, StrokeMode::Brush);
    }

    #[test]
    fn erase_mode_clears_alpha_where_brush_mode_paints_it() {
        let mut session = EditorSession::blank(3, 1).expect("3x1 is a valid canvas");

        session
            .begin_stroke(&drag_segment(StrokeMode::Brush, (0, 0), (2, 0)))
            .expect("the brush paints");
        assert_eq!(session.pick_color(0, 1, 0), Some([255, 0, 0, 255]));

        session
            .begin_stroke(&drag_segment(StrokeMode::Erase, (1, 0), (1, 0)))
            .expect("the eraser clears");
        assert_eq!(
            session.pick_color(0, 1, 0),
            Some([0, 0, 0, 0]),
            "the eraser clears alpha"
        );
        assert_eq!(
            session.pick_color(0, 0, 0),
            Some([255, 0, 0, 255]),
            "neighbouring pixels survive"
        );

        assert_eq!(
            session.undo().state.undo_depth,
            1,
            "the eraser is its own step"
        );
        assert_eq!(session.undo().state.undo_depth, 0, "then the brush step");
    }

    #[test]
    fn an_empty_slot_reports_no_session_and_mutations_are_refused() {
        let slot = EditorSlot::default();
        assert!(slot.with_session(EditorSession::state).is_none());
        let error = slot
            .with_session_mut(|session| Ok(session.undo()))
            .expect_err("undo without a session must fail");
        assert_eq!(error, NO_SESSION);
    }

    #[test]
    fn a_blank_session_round_trips_state_and_a_frame_png() {
        let slot = EditorSlot::default();
        let session = EditorSession::blank(3, 2).expect("3x2 is a valid canvas");
        let state = session.state();
        assert_eq!((state.width, state.height), (3, 2));
        assert_eq!(state.frame_count, 1);
        assert_eq!(state.frame_index, 0);
        assert_eq!(state.origin, EditorOrigin::Blank);
        assert_eq!(state.layers.len(), 1, "a blank frame has one base layer");
        assert_eq!(state.layers[0].name, "Base");
        assert_eq!((state.layers[0].width, state.layers[0].height), (3, 2));
        assert!(!state.can_undo && !state.can_redo);
        assert_eq!((state.undo_depth, state.redo_depth), (0, 0));
        assert!(!state.dirty, "a freshly opened document is not dirty");
        assert_eq!(
            state.frames,
            vec![EditorFrameSummary {
                index: 0,
                width: 3,
                height: 2,
                layer_count: 1,
                selected: true,
            }]
        );
        slot.set(session);

        let png = slot
            .with_session(|session| session.frame_image(0))
            .expect("the slot holds a session")
            .expect("frame 0 exists")
            .to_png_bytes()
            .expect("the frame encodes");
        let decoded = image::load_from_memory(&png).expect("the frame PNG decodes");
        assert_eq!((decoded.width(), decoded.height()), (3, 2));
        for pixel in decoded.to_rgba8().pixels() {
            assert_eq!(pixel.0[3], 0, "a blank frame is fully transparent");
        }

        let missing = slot
            .with_session(|session| session.frame_image(1))
            .expect("the slot holds a session");
        assert!(
            missing.is_err(),
            "frame 1 of a one-frame document is out of range"
        );
    }

    #[test]
    fn a_bag_prop_opens_with_its_dimensions_and_the_file_is_untouched() {
        let dir = TempDir::new("bag-open");
        let bag = bag_service(dir.path());
        let (blob, crc) = prop_blob();
        assert_eq!(
            bag.add_prop(7, crc, &blob, Some("Dot"))
                .expect("the prop is added"),
            crate::bag::BagOutcome::Added
        );
        let path = palace_prop::bag_folder::my_bag_path(dir.path());
        let before = std::fs::read(&path).expect("My Bag was written");

        let mut session =
            open_bag_prop(&bag, "My Bag", 7, crc).expect("the bag prop opens for editing");
        let state = session.state();
        assert_eq!((state.width, state.height), (2, 2));
        assert_eq!(
            state.origin,
            EditorOrigin::BagProp {
                collection: "My Bag".to_string(),
                id: 7,
                crc,
                name: Some("Dot".to_string()),
            }
        );

        // Clone-and-edit: mutating the session must not reach the collection.
        session.document.push_snapshot();
        session
            .document
            .current_frame_mut()
            .expect("the document has a current frame")
            .layer_mut(0)
            .expect("the frame has a base layer")
            .set_pixel(0, 0, [12, 34, 56, 255])
            .expect("the pixel is in range");
        let after = std::fs::read(&path).expect("My Bag still reads");
        assert_eq!(
            before, after,
            "opening and editing a clone must never write the collection"
        );

        assert!(open_bag_prop(&bag, "My Bag", 7, crc ^ 0xFFFF_FFFF).is_err());
        assert!(open_bag_prop(&bag, "No Such Shelf", 7, crc).is_err());
    }

    #[test]
    fn undo_and_redo_change_the_returned_frame() {
        let mut session = EditorSession::blank(2, 2).expect("2x2 is a valid canvas");
        let before = frame_png(&session, 0);
        assert!(!session.undo().changed, "an empty history cannot undo");

        session.document.push_snapshot();
        session
            .document
            .current_frame_mut()
            .expect("the document has a current frame")
            .layer_mut(0)
            .expect("the frame has a base layer")
            .set_pixel(0, 0, [255, 0, 0, 255])
            .expect("the pixel is in range");
        let painted = frame_png(&session, 0);
        assert_ne!(before, painted, "painting changes the composited frame");

        let outcome = session.undo();
        assert!(outcome.changed);
        assert!(!outcome.state.can_undo);
        assert!(outcome.state.can_redo);
        assert_eq!((outcome.state.undo_depth, outcome.state.redo_depth), (0, 1));
        assert!(!outcome.state.dirty, "undoing the only edit is clean again");
        assert_eq!(
            frame_png(&session, 0),
            before,
            "undo restores the exact prior pixels"
        );

        let outcome = session.redo();
        assert!(outcome.changed);
        assert!(outcome.state.can_undo && !outcome.state.can_redo);
        assert!(outcome.state.dirty);
        assert_eq!(
            frame_png(&session, 0),
            painted,
            "redo restores the exact painted pixels"
        );

        assert!(!session.redo().changed, "an empty redo branch cannot redo");
    }

    #[test]
    fn frame_operations_change_the_document_and_report_their_result() {
        let mut session = EditorSession::blank(2, 2).expect("2x2 is a valid canvas");

        let outcome = session.add_frame();
        assert!(outcome.changed);
        assert_eq!(outcome.state.frame_count, 2);
        assert_eq!(outcome.state.frame_index, 1, "the new frame is selected");

        let outcome = session.duplicate_frame(0);
        assert!(outcome.changed);
        assert_eq!(outcome.state.frame_count, 3);
        assert_eq!(outcome.state.frame_index, 1, "the copy follows its source");

        let outcome = session.move_frame(0, 2);
        assert!(outcome.changed);
        assert_eq!(
            outcome.state.frame_index, 0,
            "the selection follows its frame"
        );
        assert_eq!(
            outcome
                .state
                .frames
                .iter()
                .map(|frame| frame.index)
                .collect::<Vec<_>>(),
            vec![0, 1, 2]
        );

        let outcome = session.delete_frame(0);
        assert!(outcome.changed);
        assert_eq!(outcome.state.frame_count, 2);

        for refused in [
            session.delete_frame(9),
            session.move_frame(0, 9),
            session.move_frame(0, 0),
            session.duplicate_frame(9),
            session.select_frame(9),
        ] {
            assert!(!refused.changed, "an invalid operation changes nothing");
        }

        session.delete_frame(0);
        let last = session.delete_frame(0);
        assert!(!last.changed, "the last frame cannot be deleted");
        assert_eq!(last.state.frame_count, 1);
        assert!(last.state.can_undo);
    }

    #[test]
    fn an_imported_png_yields_correct_dimensions_and_alpha() {
        let rgba = image::RgbaImage::from_raw(
            3,
            2,
            vec![
                255, 0, 0, 255, // opaque red
                0, 255, 0, 128, // half-alpha green
                0, 0, 255, 0, // fully transparent
                10, 20, 30, 255, 40, 50, 60, 64, 70, 80, 90, 255,
            ],
        )
        .expect("3x2 RGBA");
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(rgba)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .expect("the test image encodes as PNG");

        let session = EditorSession::from_image_bytes(&bytes).expect("the PNG imports");
        let state = session.state();
        assert_eq!((state.width, state.height), (3, 2));
        assert_eq!(state.origin, EditorOrigin::Image);
        let pixels = session
            .document
            .current_frame()
            .expect("the document has a frame")
            .base()
            .expect("the frame has a base layer")
            .image()
            .as_rgba();
        assert_eq!(&pixels[0..4], &[255, 0, 0, 255]);
        assert_eq!(pixels[7], 128, "a half-alpha pixel keeps its alpha");
        assert_eq!(pixels[11], 0, "a transparent pixel stays transparent");
    }

    #[test]
    fn every_supported_import_format_decodes_to_its_dimensions() {
        let rgba = image::RgbaImage::from_raw(
            2,
            2,
            vec![
                255, 0, 0, 255, //
                0, 255, 0, 128, //
                0, 0, 255, 0, //
                255, 255, 255, 255,
            ],
        )
        .expect("2x2 RGBA");
        let dynamic = image::DynamicImage::ImageRgba8(rgba);
        for format in [
            image::ImageFormat::Png,
            image::ImageFormat::Jpeg,
            image::ImageFormat::Bmp,
            image::ImageFormat::Gif,
        ] {
            let mut bytes = Vec::new();
            dynamic
                .write_to(&mut std::io::Cursor::new(&mut bytes), format)
                .expect("the test image encodes");
            let state = EditorSession::from_image_bytes(&bytes)
                .expect("the image imports")
                .state();
            assert_eq!(
                (state.width, state.height),
                (2, 2),
                "{format:?} must decode to its dimensions"
            );
        }

        let state = EditorSession::from_image_bytes(&WEBP_2X2)
            .expect("the WebP imports")
            .state();
        assert_eq!((state.width, state.height), (2, 2));
    }

    #[test]
    fn an_oversized_import_is_refused_from_its_header() {
        // 5000x1 is over the 4096-pixel-per-side limit; the header is enough to
        // refuse it, so nothing has to decode the pixels.
        let rgba = image::RgbaImage::new(5_000, 1);
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(rgba)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .expect("the test image encodes");
        let error = EditorSession::from_image_bytes(&bytes)
            .expect_err("an oversized canvas must be refused");
        assert!(error.contains("4096"), "the error names the limit: {error}");

        let error = EditorSession::from_image_bytes(b"not an image")
            .expect_err("junk bytes must be refused");
        assert!(!error.is_empty());
    }

    /// A save request with every flag explicit.
    fn save_request(placement: SavePlacement) -> SavePropRequest {
        SavePropRequest {
            placement,
            name: Some("Saved Prop".to_string()),
            prop_id: None,
            head: true,
            ghost: false,
            rare: false,
            bounce: false,
            animate: false,
            h_offset: -4,
            v_offset: 6,
        }
    }

    /// The single pixel channels `quantize` round-trips exactly.
    fn quantised(pixel: [u8; 4]) -> [u8; 4] {
        let q = |channel: u8| ((palace_prop::quantize(channel) * 255) / 31) as u8;
        [q(pixel[0]), q(pixel[1]), q(pixel[2]), q(pixel[3])]
    }

    /// Decode the `(id, crc)` record the bag lists, straight from My Bag's file.
    fn load_bag_prop(bag: &BagService, id: u32, crc: u32) -> Prop {
        let listed = bag
            .snapshot()
            .entries()
            .into_iter()
            .find(|entry| entry.id == id && entry.crc == crc)
            .expect("the saved record is listed in My Bag");
        assert_eq!(listed.collection, "My Bag");
        let my_bag = bag
            .collections()
            .expect("collections")
            .into_iter()
            .find(|collection| collection.name == "My Bag")
            .expect("My Bag exists");
        let bytes = std::fs::read(&my_bag.path).expect("My Bag reads");
        let roster = Roster::parse(&bytes).expect("My Bag parses");
        let blob = roster
            .blob_for(PropKey::new(id as i32, crc))
            .expect("the record's blob is present")
            .to_vec();
        palace_prop::decode(&blob).expect("the saved prop decodes")
    }

    #[test]
    fn a_paint_stroke_changes_the_frame_png_and_undo_restores_it() {
        let mut session = EditorSession::blank(4, 4).expect("4x4 is valid");
        let before = frame_png(&session, 0);
        let outcome = session
            .paint_stroke(&StrokeRequest {
                from_x: 1,
                from_y: 1,
                to_x: 2,
                to_y: 1,
                color: [255, 0, 0, 255],
                size: 1,
                layer: 0,
                all_frames: false,
            })
            .expect("the stroke applies");
        assert!(outcome.changed, "a stroke reports a change");
        assert!(outcome.state.dirty, "a stroke dirties the document");
        let painted = frame_png(&session, 0);
        assert_ne!(before, painted, "painting changes the composited frame");

        assert_eq!(
            session.pick_color(0, 1, 1),
            Some([255, 0, 0, 255]),
            "the painted pixel reads back"
        );

        assert!(session.undo().changed, "the stroke is undoable");
        assert_eq!(
            frame_png(&session, 0),
            before,
            "undo restores the exact prior pixels"
        );
    }

    #[test]
    fn a_stroke_on_a_missing_layer_is_a_clean_no_op() {
        let mut session = EditorSession::blank(2, 2).expect("2x2 is valid");
        let outcome = session
            .paint_stroke(&StrokeRequest {
                from_x: 0,
                from_y: 0,
                to_x: 0,
                to_y: 0,
                color: [255, 0, 0, 255],
                size: 1,
                layer: 9,
                all_frames: false,
            })
            .expect("a missing layer is not an error");
        assert!(!outcome.changed, "nothing changed");
        assert!(
            !outcome.state.can_undo,
            "a no-op must not pollute the undo history"
        );
    }

    #[test]
    fn a_rotate_swaps_the_canvas_dimensions() {
        let mut session = EditorSession::blank(4, 2).expect("4x2 is valid");
        let outcome = session
            .rotate(&RotateRequest {
                turn: QuarterTurnChoice::Clockwise,
                all_frames: false,
            })
            .expect("the rotate applies");
        assert!(outcome.changed);
        assert_eq!((outcome.state.width, outcome.state.height), (2, 4));
    }

    #[test]
    fn a_resize_with_constrain_aspect_keeps_the_ratio() {
        let mut session = EditorSession::blank(4, 2).expect("4x2 is valid");
        let outcome = session
            .resize(&ResizeRequest {
                width: 8,
                height: 8,
                constrain_aspect: true,
                resample: ResampleChoice::Bilinear,
                all_frames: false,
            })
            .expect("the resize applies");
        assert!(outcome.changed);
        assert_eq!((outcome.state.width, outcome.state.height), (8, 4));
    }

    #[test]
    fn adjustments_and_flood_fill_change_pixels() {
        let mut session = EditorSession::blank(2, 2).expect("2x2 is valid");
        session
            .paint_stroke(&StrokeRequest {
                from_x: 0,
                from_y: 0,
                to_x: 1,
                to_y: 1,
                color: [0, 0, 0, 255],
                size: 1,
                layer: 0,
                all_frames: false,
            })
            .expect("paint a black base");

        let before = frame_png(&session, 0);
        let outcome = session
            .adjust_brightness_contrast(&BrightnessContrastRequest {
                brightness: 0.5,
                contrast: 0.0,
                all_frames: false,
            })
            .expect("brightness applies");
        assert!(outcome.changed);
        assert_ne!(before, frame_png(&session, 0), "brightness changed pixels");

        let outcome = session
            .flood_fill(&FloodFillRequest {
                x: 0,
                y: 0,
                color: [0, 255, 0, 255],
                tolerance: 0.1,
                layer: 0,
                all_frames: false,
            })
            .expect("fill applies");
        assert!(outcome.changed);
    }

    #[test]
    fn saving_a_prop_writes_a_my_bag_record_that_re_reads_pixel_identical() {
        let dir = TempDir::new("save-new");
        let bag = bag_service(dir.path());
        let mut session = EditorSession::blank(2, 2).expect("2x2 is valid");
        let source = [
            [255, 0, 0, 255],
            [0, 255, 255, 255],
            [0, 0, 0, 0],
            [255, 255, 255, 255],
        ];
        for (index, color) in source.iter().enumerate() {
            let x = index as u32 % 2;
            let y = index as u32 / 2;
            session
                .document
                .current_frame_mut()
                .expect("a current frame")
                .layer_mut(0)
                .expect("a base layer")
                .set_pixel(x, y, *color)
                .expect("the pixel is in range");
        }

        let result = session
            .save_to_bag(&bag, &save_request(SavePlacement::NewPropAtEnd))
            .expect("the prop saves");
        assert_eq!(result.outcome, crate::bag::BagOutcome::Added);
        assert_eq!(result.id, 1, "the first prop takes id 1");
        assert_eq!(
            result.state.origin,
            EditorOrigin::BagProp {
                collection: "My Bag".to_string(),
                id: 1,
                crc: result.crc,
                name: Some("Saved Prop".to_string()),
            }
        );
        assert!(!result.state.dirty, "a save marks the document clean");

        let prop = load_bag_prop(&bag, result.id, result.crc);
        assert_eq!((prop.image.width(), prop.image.height()), (2, 2));
        assert!(prop.header.is_head(), "the HEAD flag round-trips");
        assert!(!prop.header.is_ghost());
        assert_eq!((prop.header.h_offset, prop.header.v_offset), (-4, 6));
        for (index, expected) in source.iter().enumerate() {
            let x = index as u32 % 2;
            let y = index as u32 / 2;
            assert_eq!(
                prop.image.pixel(x, y),
                Some(quantised(*expected)),
                "pixel {index} re-reads after S20 quantisation"
            );
        }
    }

    #[test]
    fn overwrite_in_place_replaces_the_opened_record() {
        let dir = TempDir::new("save-overwrite");
        let bag = bag_service(dir.path());
        let (blob, crc) = prop_blob();
        bag.add_prop(7, crc, &blob, Some("Dot"))
            .expect("the prop is added");

        let mut session = open_bag_prop(&bag, "My Bag", 7, crc).expect("the prop opens");
        session
            .paint_stroke(&StrokeRequest {
                from_x: 0,
                from_y: 0,
                to_x: 1,
                to_y: 1,
                color: [255, 0, 0, 255],
                size: 1,
                layer: 0,
                all_frames: false,
            })
            .expect("paint the clone");

        let result = session
            .save_to_bag(&bag, &save_request(SavePlacement::OverwriteInPlace))
            .expect("the overwrite saves");
        assert_eq!(result.id, 7, "overwrite keeps the opened id");
        assert_ne!(result.crc, crc, "changed pixels carry a new CRC");
        assert_eq!(
            bag.snapshot().entries().len(),
            1,
            "overwrite leaves exactly one record, never a duplicate"
        );
        assert_eq!(
            load_bag_prop(&bag, 7, result.crc)
                .image
                .pixel(0, 0)
                .expect("the pixel is in range")[0],
            255
        );
    }

    #[test]
    fn saving_a_multi_frame_document_is_refused() {
        let dir = TempDir::new("save-animated");
        let bag = bag_service(dir.path());
        let mut session = EditorSession::blank(2, 2).expect("2x2 is valid");
        assert!(session.add_frame().changed);

        let error = session
            .save_to_bag(&bag, &save_request(SavePlacement::NewPropAtEnd))
            .expect_err("a multi-frame document must be refused");
        assert!(
            error.contains("animated") && error.contains("more than one frame"),
            "the refusal names the gated animated encoder: {error}"
        );
        assert!(
            bag.snapshot().entries().is_empty(),
            "a refused save writes nothing"
        );
    }

    #[test]
    fn overwrite_in_place_needs_a_bag_origin() {
        let dir = TempDir::new("save-plain");
        let bag = bag_service(dir.path());
        let mut session = EditorSession::blank(2, 2).expect("2x2 is valid");
        let error = session
            .save_to_bag(&bag, &save_request(SavePlacement::OverwriteInPlace))
            .expect_err("a blank document has nothing to overwrite");
        assert!(error.contains("opened from the bag"), "{error}");
    }

    #[test]
    fn exporting_writes_a_png_and_a_single_frame_webp() {
        let session = EditorSession::blank(3, 2).expect("3x2 is valid");
        let png = session.export_png().expect("the PNG exports");
        let decoded = image::load_from_memory(&png).expect("the PNG decodes");
        assert_eq!((decoded.width(), decoded.height()), (3, 2));

        let webp = session.export_webp(100).expect("the WebP exports");
        let decoded = image::load_from_memory(&webp).expect("the WebP decodes");
        assert_eq!((decoded.width(), decoded.height()), (3, 2));
        let decoder =
            image_webp::WebPDecoder::new(std::io::Cursor::new(&webp)).expect("the WebP parses");
        assert!(!decoder.is_animated(), "a one-frame document is a still");
    }

    #[test]
    fn exporting_a_multi_frame_document_writes_an_animated_webp() {
        let mut session = EditorSession::blank(2, 2).expect("2x2 is valid");
        assert!(session.add_frame().changed);
        assert!(session.add_frame().changed);
        assert_eq!(session.state().frame_count, 3);

        let webp = session.export_webp(80).expect("the animation exports");
        let mut decoder =
            image_webp::WebPDecoder::new(std::io::Cursor::new(&webp)).expect("the WebP parses");
        assert!(decoder.is_animated(), "three frames are an animation");
        assert_eq!(decoder.num_frames(), 3);
        assert_eq!((decoder.dimensions()), (2, 2));

        let buffer_size = decoder.output_buffer_size().expect("a frame buffer size");
        let mut buffer = vec![0u8; buffer_size];
        let mut durations = Vec::new();
        while let Ok(duration) = decoder.read_frame(&mut buffer) {
            durations.push(duration);
        }
        assert_eq!(durations, vec![80, 80, 80]);
    }

    // -- Text, overlays and the layer list ----------------------------------

    /// A stable digest of an image's pixels and size.
    fn digest(image: &PropImage) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        (image.width(), image.height()).hash(&mut hasher);
        image.as_rgba().hash(&mut hasher);
        hasher.finish()
    }

    /// Whether any pixel in the rectangle has non-zero alpha.
    fn has_ink(image: &PropImage, x0: u32, y0: u32, x1: u32, y1: u32) -> bool {
        (y0..y1.min(image.height())).any(|y| {
            (x0..x1.min(image.width())).any(|x| image.pixel(x, y).is_some_and(|pixel| pixel[3] > 0))
        })
    }

    /// A fully opaque `side` x `side` square of one colour.
    fn solid(side: u32, color: [u8; 4]) -> PropImage {
        PropImage::from_rgba(side, side, color.repeat((side * side) as usize))
            .expect("a solid square")
    }

    fn text_request(text: &str) -> TextLayerRequest {
        TextLayerRequest {
            text: text.to_string(),
            color: [255, 255, 255, 255],
            size: 12.0,
            x: 4,
            y: 6,
            rotation: 0.0,
            opacity: 1.0,
            shadow: false,
            outline: false,
            wrap: false,
        }
    }

    #[test]
    fn a_text_layer_rasterizes_at_its_position_and_is_deterministic() {
        let mut session = EditorSession::blank(64, 32).expect("64x32 is valid");
        let request = text_request("Hi");
        let outcome = session.add_text_layer(&request).expect("the text renders");
        assert!(outcome.changed);
        assert_eq!(outcome.state.layers.len(), 2, "base plus one text layer");
        assert!(outcome.state.layers[1].name.starts_with("Text:"));
        assert_eq!(
            (outcome.state.layers[1].x, outcome.state.layers[1].y),
            (4, 6)
        );

        let image = session.frame_image(0).expect("the frame composites");
        assert!(
            has_ink(&image, 4, 6, 22, 22),
            "ink sits where the text was placed"
        );
        assert!(
            !has_ink(&image, 40, 24, 64, 32),
            "no ink reaches the far corner"
        );
        assert_eq!(
            digest(&image),
            digest(&session.frame_image(0).expect("composites again")),
            "the same document composites identically"
        );

        let mut twin = EditorSession::blank(64, 32).expect("64x32 is valid");
        twin.add_text_layer(&request).expect("the text renders");
        assert_eq!(
            digest(&session.frame_image(0).expect("composites")),
            digest(&twin.frame_image(0).expect("composites")),
            "the same request renders identical bytes"
        );

        let mut moved = EditorSession::blank(64, 32).expect("64x32 is valid");
        moved
            .add_text_layer(&TextLayerRequest {
                x: 30,
                ..request.clone()
            })
            .expect("the text renders");
        assert_ne!(
            digest(&session.frame_image(0).expect("composites")),
            digest(&moved.frame_image(0).expect("composites")),
            "moving the text moves the ink"
        );
    }

    #[test]
    fn text_shadow_and_outline_add_ink() {
        let plain = text::render("A", [255, 255, 255, 255], 12.0, false, false, None)
            .expect("plain text renders");
        let shadowed = text::render("A", [255, 255, 255, 255], 12.0, true, false, None)
            .expect("shadowed text renders");
        let outlined = text::render("A", [255, 255, 255, 255], 12.0, false, true, None)
            .expect("outlined text renders");
        let ink = |image: &PropImage| {
            image
                .as_rgba()
                .chunks_exact(4)
                .filter(|pixel| pixel[3] > 0)
                .count()
        };
        assert!(ink(&shadowed) > ink(&plain), "a drop shadow adds pixels");
        assert!(ink(&outlined) > ink(&plain), "an outline adds pixels");
        assert_ne!(digest(&plain), digest(&outlined));
    }

    #[test]
    fn word_wrap_clamps_the_text_block_to_the_frame_width() {
        let text = "wrapping text wider than the canvas";

        let mut wrapped = EditorSession::blank(24, 64).expect("24x64 is valid");
        wrapped
            .add_text_layer(&TextLayerRequest {
                wrap: true,
                ..text_request(text)
            })
            .expect("wrapped text renders");
        assert_eq!(
            wrapped.state().layers[1].width,
            26,
            "wrapped text is the frame width plus one pixel of padding each side"
        );

        let mut unwrapped = EditorSession::blank(24, 64).expect("24x64 is valid");
        unwrapped
            .add_text_layer(&text_request(text))
            .expect("unwrapped text renders");
        assert!(
            unwrapped.state().layers[1].width > 26,
            "an unwrapped line can overflow the frame"
        );
    }

    #[test]
    fn an_overlays_position_rotation_scale_and_opacity_change_the_composite() {
        let mut session = EditorSession::blank(8, 8).expect("8x8 is valid");
        session
            .add_overlay_layer(solid(2, [255, 0, 0, 255]), None)
            .expect("the overlay is added");
        let at_origin = digest(&session.frame_image(0).expect("composites"));
        assert_eq!(
            session.frame_image(0).expect("composites").pixel(0, 0),
            Some([255, 0, 0, 255])
        );

        session
            .update_layer(&LayerUpdateRequest {
                layer: 1,
                x: Some(4),
                y: Some(4),
                ..LayerUpdateRequest::default()
            })
            .expect("position applies");
        let moved = digest(&session.frame_image(0).expect("composites"));
        assert_ne!(at_origin, moved, "moving the overlay changes the composite");
        assert_eq!(
            session.frame_image(0).expect("composites").pixel(0, 0),
            Some([0, 0, 0, 0]),
            "the old position is now transparent"
        );
        assert_eq!(
            session.frame_image(0).expect("composites").pixel(4, 4),
            Some([255, 0, 0, 255])
        );

        session
            .update_layer(&LayerUpdateRequest {
                layer: 1,
                rotation: Some(45.0),
                ..LayerUpdateRequest::default()
            })
            .expect("rotation applies");
        let rotated = digest(&session.frame_image(0).expect("composites"));
        assert_ne!(moved, rotated, "rotating the overlay changes the composite");

        session
            .update_layer(&LayerUpdateRequest {
                layer: 1,
                rotation: Some(0.0),
                scale: Some(2.0),
                ..LayerUpdateRequest::default()
            })
            .expect("scale applies");
        let scaled = digest(&session.frame_image(0).expect("composites"));
        assert_ne!(rotated, scaled, "scaling the overlay changes the composite");

        session
            .update_layer(&LayerUpdateRequest {
                layer: 1,
                scale: Some(1.0),
                opacity: Some(0.5),
                ..LayerUpdateRequest::default()
            })
            .expect("opacity applies");
        let alpha = session
            .frame_image(0)
            .expect("composites")
            .pixel(4, 4)
            .expect("a pixel")[3];
        assert!(
            alpha > 0 && alpha < 255,
            "half opacity yields a partial alpha: {alpha}"
        );
        assert_ne!(digest(&session.frame_image(0).expect("composites")), moved);
    }

    #[test]
    fn reordering_layers_changes_z_order() {
        let mut session = EditorSession::blank(2, 2).expect("2x2 is valid");
        session
            .add_overlay_layer(solid(2, [255, 0, 0, 255]), None)
            .expect("red is added");
        session
            .add_overlay_layer(solid(2, [0, 0, 255, 255]), None)
            .expect("blue is added");
        let pixel = |session: &EditorSession| {
            session
                .frame_image(0)
                .expect("composites")
                .pixel(0, 0)
                .expect("a pixel")
        };
        assert_eq!(
            pixel(&session),
            [0, 0, 255, 255],
            "the last added layer is on top"
        );

        let outcome = session.move_layer(2, 1);
        assert!(outcome.changed, "moving an overlay is a change");
        assert_eq!(
            pixel(&session),
            [255, 0, 0, 255],
            "after the reorder the red layer is on top"
        );

        assert!(
            !session.move_layer(0, 2).changed,
            "the base layer cannot move"
        );
        assert!(
            !session.move_layer(1, 1).changed,
            "a move to the same slot is a no-op"
        );
    }

    #[test]
    fn hiding_a_layer_hides_it_without_deleting_it() {
        let mut session = EditorSession::blank(2, 2).expect("2x2 is valid");
        session
            .add_overlay_layer(solid(2, [255, 0, 0, 255]), None)
            .expect("the overlay is added");

        let hidden = session
            .update_layer(&LayerUpdateRequest {
                layer: 1,
                visible: Some(false),
                ..LayerUpdateRequest::default()
            })
            .expect("visibility applies");
        assert!(hidden.changed);
        assert_eq!(
            hidden.state.layers.len(),
            2,
            "the layer is hidden, not deleted"
        );
        assert!(!hidden.state.layers[1].visible);
        assert!(
            hidden.state.layers[0].visible,
            "hiding one layer leaves the base alone"
        );
        assert_eq!(
            session
                .frame_image(0)
                .expect("composites")
                .pixel(0, 0)
                .expect("a pixel")[3],
            0,
            "a hidden overlay contributes nothing"
        );

        session
            .update_layer(&LayerUpdateRequest {
                layer: 1,
                visible: Some(true),
                ..LayerUpdateRequest::default()
            })
            .expect("visibility applies");
        assert_eq!(
            session.frame_image(0).expect("composites").pixel(0, 0),
            Some([255, 0, 0, 255]),
            "showing the layer restores it"
        );

        assert!(
            !session.delete_layer(0).changed,
            "the base layer cannot be deleted"
        );
        let deleted = session.delete_layer(1);
        assert!(deleted.changed);
        assert_eq!(deleted.state.layers.len(), 1, "delete removes the overlay");
    }

    #[test]
    fn a_nameless_overlay_gets_a_numbered_name() {
        let mut session = EditorSession::blank(2, 2).expect("2x2 is valid");
        session
            .add_overlay_layer(solid(2, [1, 2, 3, 255]), None)
            .expect("the overlay is added");
        assert_eq!(session.state().layers[1].name, "Overlay 1");

        session
            .add_overlay_layer(solid(2, [4, 5, 6, 255]), Some("Badge".to_string()))
            .expect("the named overlay is added");
        assert_eq!(session.state().layers[2].name, "Badge");
    }

    #[test]
    fn an_imported_overlay_decodes_and_layers_over_the_base() {
        let rgba = image::RgbaImage::from_raw(
            2,
            1,
            vec![
                255, 0, 0, 255, //
                0, 255, 0, 128,
            ],
        )
        .expect("2x1 RGBA");
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(rgba)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .expect("the test image encodes");

        let mut session = EditorSession::blank(8, 8).expect("8x8 is valid");
        let image = decode_rgba_image(&bytes).expect("the overlay decodes");
        let outcome = session
            .add_overlay_layer(image, None)
            .expect("the overlay is added");
        assert!(outcome.changed);
        assert_eq!(outcome.state.layers[1].width, 2);
        assert_eq!(session.state().layers[1].height, 1);
        assert_eq!(
            session
                .frame_image(0)
                .expect("composites")
                .pixel(1, 0)
                .expect("a pixel")[3],
            128,
            "the overlay keeps its alpha"
        );
    }

    // -- Guides -------------------------------------------------------------

    #[test]
    fn guide_geometry_for_a_44x44_canvas_is_the_model_geometry() {
        let session = EditorSession::blank(44, 44).expect("44x44 is a valid canvas");
        let geometry = session
            .guide_geometry()
            .expect("an open canvas has guide geometry");

        assert_eq!((geometry.width, geometry.height), (44, 44));

        let xs: Vec<i32> = geometry.grid.vertical.iter().map(|line| line.from.x).collect();
        assert_eq!(xs, vec![0, 4, 8, 12, 16, 20, 24, 28, 32, 36, 40]);
        assert_eq!(geometry.grid.vertical.len(), 11, "eleven cells across 44 px");
        assert!(geometry
            .grid
            .vertical
            .iter()
            .all(|line| line.from.y == 0 && line.to.y == 44));

        assert_eq!(geometry.centre.vertical.from.x, 22);
        assert_eq!(geometry.centre.horizontal.from.y, 22);
        assert_eq!(
            (
                geometry.safe_area.x,
                geometry.safe_area.y,
                geometry.safe_area.width,
                geometry.safe_area.height
            ),
            (4, 4, 36, 36)
        );
        assert_eq!(geometry.thirds.vertical[0].from.x, 15);
        assert_eq!(geometry.thirds.vertical[1].from.x, 29);
        assert_eq!(geometry.thirds.horizontal[0].from.y, 15);
        assert_eq!(geometry.thirds.horizontal[1].from.y, 29);
    }

    #[test]
    fn toggling_every_guide_on_and_off_never_changes_the_document() {
        let mut session = EditorSession::blank(44, 44).expect("44x44 is a valid canvas");
        session
            .paint_stroke(&StrokeRequest {
                from_x: 1,
                from_y: 1,
                to_x: 13,
                to_y: 17,
                color: [255, 0, 0, 255],
                size: 1,
                layer: 0,
                all_frames: false,
            })
            .expect("the stroke paints");

        let before = frame_png(&session, 0);
        let undo_before = session.state().undo_depth;

        let on = session.set_guide_toggles(GuideTogglesState {
            onion: true,
            grid: true,
            snap_grid: true,
            snap_centre: true,
            safe_area: true,
            thirds: true,
        });
        assert!(on.toggles.onion && on.toggles.grid && on.toggles.snap_centre);
        assert_eq!(
            session
                .guide_geometry()
                .expect("geometry while every guide is on")
                .grid
                .vertical
                .len(),
            11
        );
        let _ = session.onion_neighbours();
        let _ = session.snap_point(7, 7);

        let off = session.set_guide_toggles(GuideTogglesState::default());
        assert_eq!(off.toggles, GuideTogglesState::default());

        assert_eq!(
            frame_png(&session, 0),
            before,
            "guides must never alter a rendered frame"
        );
        assert_eq!(
            session.state().undo_depth,
            undo_before,
            "toggling guides is not an edit and must not snapshot"
        );
    }

    #[test]
    fn snapping_follows_the_switches_and_the_open_canvas() {
        let mut session = EditorSession::blank(44, 44).expect("44x44 is a valid canvas");
        assert_eq!(
            session.snap_point(7, 7),
            GuidePointState { x: 7, y: 7 },
            "with every guide off the point is returned unchanged"
        );

        session.set_guide_toggles(GuideTogglesState {
            snap_grid: true,
            ..GuideTogglesState::default()
        });
        assert_eq!(session.snap_point(7, 7), GuidePointState { x: 8, y: 8 });

        session.set_guide_toggles(GuideTogglesState {
            snap_centre: true,
            ..GuideTogglesState::default()
        });
        assert_eq!(
            session.snap_point(7, 7),
            GuidePointState { x: 22, y: 22 },
            "centre wins when both switches are on"
        );
    }

    #[test]
    fn snapping_moves_a_painted_stroke_onto_the_grid() {
        let mut session = EditorSession::blank(44, 44).expect("44x44 is a valid canvas");
        session.set_guide_toggles(GuideTogglesState {
            snap_grid: true,
            ..GuideTogglesState::default()
        });
        session
            .paint_stroke(&StrokeRequest {
                from_x: 1,
                from_y: 1,
                to_x: 6,
                to_y: 6,
                color: [255, 0, 0, 255],
                size: 1,
                layer: 0,
                all_frames: false,
            })
            .expect("the snapped stroke paints");

        assert_eq!(
            session.pick_color(0, 0, 0),
            Some([255, 0, 0, 255]),
            "the stroke start moved to the grid origin"
        );
        assert_eq!(
            session.pick_color(0, 8, 8),
            Some([255, 0, 0, 255]),
            "the stroke end moved to the 8 px intersection"
        );
    }

    #[test]
    fn onion_neighbours_are_right_at_the_first_middle_and_last_frame() {
        let mut session = EditorSession::blank(4, 4).expect("4x4 is a valid canvas");
        for _ in 0..4 {
            session.add_frame();
        }
        assert_eq!(session.state().frame_count, 5);

        session.select_frame(0);
        assert_eq!(
            session.onion_neighbours(),
            vec![OnionNeighbourState {
                index: 1,
                offset: 1
            }],
            "the first frame ghosts only the one after it"
        );

        session.select_frame(2);
        assert_eq!(
            session.onion_neighbours(),
            vec![
                OnionNeighbourState {
                    index: 1,
                    offset: -1
                },
                OnionNeighbourState {
                    index: 3,
                    offset: 1
                },
            ],
            "the middle frame ghosts one each side, earlier first"
        );

        session.select_frame(4);
        assert_eq!(
            session.onion_neighbours(),
            vec![OnionNeighbourState {
                index: 3,
                offset: -1
            }],
            "the last frame ghosts only the one before it"
        );

        session.set_onion_radius(2);
        session.select_frame(0);
        assert_eq!(
            session.onion_neighbours(),
            vec![
                OnionNeighbourState {
                    index: 1,
                    offset: 1
                },
                OnionNeighbourState {
                    index: 2,
                    offset: 2
                },
            ],
            "a radius of two reaches two frames forward at the first frame"
        );
    }

    #[test]
    fn guide_toggles_deserialize_from_their_wire_shape() {
        let json = r#"{"onion":true,"grid":true,"snap_grid":false,
                       "snap_centre":true,"safe_area":false,"thirds":true}"#;
        let toggles: GuideTogglesState =
            serde_json::from_str(json).expect("the guide toggles deserialize");
        assert!(toggles.onion && toggles.grid && toggles.snap_centre && toggles.thirds);
        assert!(!toggles.snap_grid);
        assert!(!toggles.safe_area);
    }
}
