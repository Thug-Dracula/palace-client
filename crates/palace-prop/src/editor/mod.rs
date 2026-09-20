//! The prop editor's document model: frames, layers and bounded undo/redo.
//!
//! This module owns the editor's pixel state so all pixel work happens in Rust,
//! off the webview. It is deliberately pure: it touches no filesystem and no
//! network, and it decodes no image formats. Callers hand it RGBA bytes (or an
//! already-decoded [`Prop`]) and read RGBA frames back.
//!
//! # Shape
//!
//! An [`EditorDocument`] is an ordered list of [`Frame`]s; a frame is an ordered
//! layer stack whose index 0 is the base and whose later layers are overlays; a
//! [`Layer`] is RGBA pixels plus opacity, a placement [`Transform`] and a
//! visibility flag. [`EditorDocument::composite_current`] flattens the current
//! frame's layers into one RGBA image, which is what save/export will consume.
//!
//! The model is RGBA-in / RGBA-out. Format decoders (JPG, BMP, GIF, WebP) belong
//! to the app layer, not to this dependency-light crate; only the crate's
//! existing `png` dependency and [`PropImage`] are reused here.
//!
//! # Undo and redo
//!
//! The document wraps a bounded [`History`]. The caller pushes a snapshot
//! immediately before a mutation ([`EditorDocument::push_snapshot`]), exactly as
//! the reference editor pushes before a paint stroke, then undoes or redoes to
//! swap whole-document states. Both stacks are capped by depth and by pixel bytes
//! and evict the oldest states first; see [`History`] for the caps and their
//! rationale. Frame operations are not snapped automatically: the tool that
//! performs them decides when a state is worth keeping.
//!
//! # Frame operations
//!
//! Frames can be added blank, duplicated, deleted, reordered, copied and pasted
//! after the current frame. These are pixel-level operations only: encoding a
//! multi-frame (animated) prop is a later, gated task and does not live here.
//!
//! # Errors
//!
//! Pixel construction goes through [`PropImage`] and returns the crate's
//! [`crate::Result`]. Index-based frame and layer operations return `Option` or
//! `bool` instead, so the caller can decide whether a bad index is an error.

pub mod frame;
pub mod history;
pub mod layer;
pub mod tools;

pub use frame::Frame;
pub use history::{History, MAX_UNDO_BYTES, MAX_UNDO_SNAPSHOTS};
pub use layer::{Layer, LayerId, Transform};

use crate::error::{PropError, Result, MAX_DIMENSION, MAX_PIXELS};
use crate::image::PropImage;
use crate::Prop;

/// The canvas size a default document starts with, matching Palace props.
pub const DEFAULT_CANVAS: u32 = 44;

/// A whole-document snapshot: everything undo/redo must restore.
///
/// Layer ids are kept, so a restored layer is the same layer. The document's
/// id counter is deliberately **not** part of the snapshot: it only ever grows, so
/// a layer created after an undo cannot collide with one the undo brought back.
#[derive(Debug, Clone, PartialEq)]
pub struct DocumentState {
    /// The document's frames, in order.
    pub frames: Vec<Frame>,
    /// The selected frame index.
    pub current: usize,
}

impl DocumentState {
    /// Bundle frames and a selection index.
    #[must_use]
    pub fn new(frames: Vec<Frame>, current: usize) -> Self {
        DocumentState { frames, current }
    }

    /// Pixel bytes this state holds, across every layer of every frame.
    #[must_use]
    pub fn memory_bytes(&self) -> usize {
        self.frames
            .iter()
            .flat_map(|frame| frame.layers())
            .map(|layer| layer.image().as_rgba().len())
            .sum()
    }
}

/// The prop editor's document: ordered frames, a selection, a clipboard and a
/// bounded history.
#[derive(Debug, Clone)]
pub struct EditorDocument {
    frames: Vec<Frame>,
    current: usize,
    next_layer_id: u64,
    clipboard: Option<Frame>,
    history: History,
}

impl Default for EditorDocument {
    fn default() -> Self {
        EditorDocument::with_frame(Frame::blank(DEFAULT_CANVAS, DEFAULT_CANVAS))
    }
}

impl EditorDocument {
    // -- Import constructors ------------------------------------------------

    /// A document with one blank, transparent frame of the given size.
    ///
    /// Refused when a dimension is zero, exceeds [`MAX_DIMENSION`], or the total
    /// pixel count exceeds [`MAX_PIXELS`].
    pub fn blank(width: u32, height: u32) -> Result<Self> {
        validate_canvas(width, height)?;
        Ok(EditorDocument::with_frame(Frame::blank(width, height)))
    }

    /// A document whose single frame is a base layer built from raw RGBA bytes.
    ///
    /// The bytes must hold exactly `width * height * 4` values; the canvas takes
    /// the given dimensions. Alpha is preserved as given.
    pub fn from_rgba(width: u32, height: u32, rgba: Vec<u8>) -> Result<Self> {
        let image = PropImage::from_rgba(width, height, rgba)?;
        Ok(EditorDocument::with_frame(Frame::from_base(image)))
    }

    /// A document whose single frame is the decoded pixels of an existing prop.
    ///
    /// Only the pixels cross over: the prop's header flags, offsets and script
    /// offset are not model state. Encoding them back is a later task.
    #[must_use]
    pub fn from_prop(prop: &Prop) -> Self {
        EditorDocument::with_frame(Frame::from_base(prop.image.clone()))
    }

    /// A document whose single frame is a clone of an existing image.
    #[must_use]
    pub fn from_prop_image(image: &PropImage) -> Self {
        EditorDocument::with_frame(Frame::from_base(image.clone()))
    }

    /// A document with one blank frame and explicit history caps.
    ///
    /// Mainly for tests and for callers that want a tighter bound than the
    /// [`History`] defaults.
    pub fn blank_with_history_caps(
        width: u32,
        height: u32,
        depth_cap: usize,
        byte_cap: usize,
    ) -> Result<Self> {
        validate_canvas(width, height)?;
        let mut document = EditorDocument::with_frame(Frame::blank(width, height));
        document.history = History::with_caps(depth_cap, byte_cap);
        Ok(document)
    }

    /// Build a document around one already-adopted frame.
    fn with_frame(frame: Frame) -> Self {
        let mut document = EditorDocument {
            frames: Vec::new(),
            current: 0,
            next_layer_id: 0,
            clipboard: None,
            history: History::new(),
        };
        let frame = document.adopt(frame);
        document.frames.push(frame);
        document
    }

    // -- Accessors ----------------------------------------------------------

    /// How many frames the document holds.
    #[must_use]
    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    /// Every frame, in order.
    #[must_use]
    pub fn frames(&self) -> &[Frame] {
        &self.frames
    }

    /// The frame at `index`, or `None` when out of range.
    #[must_use]
    pub fn frame(&self, index: usize) -> Option<&Frame> {
        self.frames.get(index)
    }

    /// The selected frame index.
    #[must_use]
    pub fn current_index(&self) -> usize {
        self.current
    }

    /// Select a frame, returning whether the index was in range.
    pub fn set_current(&mut self, index: usize) -> bool {
        if index < self.frames.len() {
            self.current = index;
            true
        } else {
            false
        }
    }

    /// The selected frame. A document always has at least one frame.
    #[must_use]
    pub fn current_frame(&self) -> Option<&Frame> {
        self.frames.get(self.current)
    }

    /// Mutable access to the selected frame.
    #[must_use]
    pub fn current_frame_mut(&mut self) -> Option<&mut Frame> {
        self.frames.get_mut(self.current)
    }

    /// Flatten the selected frame's layers into one RGBA image.
    ///
    /// This is the "current frame pixels" the UI and a later save path read. A
    /// document always has a frame, so the error is only an invariant guard.
    pub fn composite_current(&self) -> Result<PropImage> {
        match self.frames.get(self.current) {
            Some(frame) => frame.composite(),
            None => Err(PropError::UnencodableImage {
                width: 0,
                height: 0,
            }),
        }
    }

    // -- Frame operations ---------------------------------------------------

    /// Append a blank frame matching the current frame's size, and select it.
    ///
    /// Returns the new frame's index.
    pub fn add_blank_frame(&mut self) -> Option<usize> {
        let (width, height) = match self.frames.get(self.current) {
            Some(frame) => (frame.width(), frame.height()),
            None => (DEFAULT_CANVAS, DEFAULT_CANVAS),
        };
        let frame = self.adopt(Frame::blank(width, height));
        self.frames.push(frame);
        self.current = self.frames.len() - 1;
        Some(self.current)
    }

    /// Duplicate the frame at `index` directly after it, and select the copy.
    ///
    /// The copy gets fresh layer ids so it cannot be confused with its source.
    /// Returns the copy's index.
    pub fn duplicate_frame(&mut self, index: usize) -> Option<usize> {
        let copy = self.frames.get(index)?.clone();
        let copy = self.adopt(copy);
        let at = index + 1;
        self.frames.insert(at, copy);
        self.current = at;
        Some(at)
    }

    /// Delete the frame at `index`, returning it.
    ///
    /// Refused when `index` is out of range or it is the document's only frame: a
    /// document always keeps at least one frame.
    pub fn delete_frame(&mut self, index: usize) -> Option<Frame> {
        if self.frames.len() <= 1 || index >= self.frames.len() {
            return None;
        }
        let removed = self.frames.remove(index);
        if self.current >= self.frames.len() {
            self.current = self.frames.len() - 1;
        } else if self.current > index {
            self.current -= 1;
        }
        Some(removed)
    }

    /// Move the frame at `from` so it sits at `to`, keeping the selection on the
    /// same frame. Returns whether both indices were in range.
    pub fn move_frame(&mut self, from: usize, to: usize) -> bool {
        if from >= self.frames.len() || to >= self.frames.len() {
            return false;
        }
        if from == to {
            return true;
        }
        let frame = self.frames.remove(from);
        self.frames.insert(to, frame);
        self.current = track_move(self.current, from, to);
        true
    }

    /// Copy the frame at `index` into the clipboard. Returns whether it existed.
    pub fn copy_frame(&mut self, index: usize) -> bool {
        match self.frames.get(index) {
            Some(frame) => {
                self.clipboard = Some(frame.clone());
                true
            }
            None => false,
        }
    }

    /// Whether a frame has been copied and not yet overwritten.
    #[must_use]
    pub fn has_clipboard(&self) -> bool {
        self.clipboard.is_some()
    }

    /// Insert a copy of the clipboard directly after the current frame.
    ///
    /// The copy gets fresh layer ids. Returns the new frame's index, or `None`
    /// when the clipboard is empty.
    pub fn paste_after_current(&mut self) -> Option<usize> {
        let copy = self.clipboard.clone()?;
        let copy = self.adopt(copy);
        let at = self.current + 1;
        self.frames.insert(at, copy);
        self.current = at;
        Some(at)
    }

    // -- Layers -------------------------------------------------------------

    /// Add an overlay layer on top of the current frame's stack.
    ///
    /// Returns the new layer's id, or `None` in the impossible case that the
    /// document has no current frame.
    pub fn add_layer(&mut self, name: &str, image: PropImage) -> Option<LayerId> {
        let id = self.alloc_layer_id();
        let layer = Layer::new(id, name, image);
        let frame = self.frames.get_mut(self.current)?;
        frame.push_layer(layer);
        Some(id)
    }

    // -- Undo / redo --------------------------------------------------------

    /// Push the current state as the point to return to.
    ///
    /// Call this immediately before a mutation, matching `PaintSnapshotPush`. It
    /// clears the redo branch, as the reference does.
    pub fn push_snapshot(&mut self) {
        self.history.push(self.snapshot_state());
    }

    /// Restore the most recent snapshot, moving the current state to redo.
    ///
    /// Returns whether an undo happened.
    pub fn undo(&mut self) -> bool {
        let current = self.snapshot_state();
        match self.history.undo(current) {
            Some(state) => {
                self.restore(state);
                true
            }
            None => false,
        }
    }

    /// Reapply the most recently undone state.
    ///
    /// Returns whether a redo happened.
    pub fn redo(&mut self) -> bool {
        let current = self.snapshot_state();
        match self.history.redo(current) {
            Some(state) => {
                self.restore(state);
                true
            }
            None => false,
        }
    }

    /// Whether a snapshot is available to undo.
    #[must_use]
    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    /// Whether a snapshot is available to redo.
    #[must_use]
    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    /// The bounded history, for inspecting caps and memory use.
    #[must_use]
    pub fn history(&self) -> &History {
        &self.history
    }

    /// Capture the restore-able state.
    fn snapshot_state(&self) -> DocumentState {
        DocumentState {
            frames: self.frames.clone(),
            current: self.current,
        }
    }

    /// Replace the document's frames from a snapshot, clamping the selection.
    fn restore(&mut self, state: DocumentState) {
        if state.frames.is_empty() {
            return;
        }
        self.frames = state.frames;
        self.current = state.current.min(self.frames.len() - 1);
    }

    /// Reissue fresh layer ids for every layer of an incoming frame.
    fn adopt(&mut self, mut frame: Frame) -> Frame {
        for layer in frame.layers_mut() {
            let id = self.alloc_layer_id();
            layer.set_id(id);
        }
        frame
    }

    /// Allocate the next document-unique layer id.
    fn alloc_layer_id(&mut self) -> LayerId {
        let id = LayerId::new(self.next_layer_id);
        self.next_layer_id = self.next_layer_id.saturating_add(1);
        id
    }
}

/// Follow the selection through a frame reorder.
fn track_move(current: usize, from: usize, to: usize) -> usize {
    if current == from {
        to
    } else if from < current && current <= to {
        current - 1
    } else if to <= current && current < from {
        current + 1
    } else {
        current
    }
}

/// Refuse canvas sizes that are zero, too wide/tall, or too many pixels.
fn validate_canvas(width: u32, height: u32) -> Result<()> {
    if width == 0 || height == 0 || width > MAX_DIMENSION as u32 || height > MAX_DIMENSION as u32 {
        return Err(PropError::ImplausibleDimensions {
            width: width as i16,
            height: height as i16,
        });
    }
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(PropError::ImageTooLarge {
            width: width as i16,
            height: height as i16,
        });
    }
    Ok(())
}
