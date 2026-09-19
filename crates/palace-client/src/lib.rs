//! Headless Palace client runtime.
//!
//! Owns the connection to a pserver, the session state derived from it, the
//! live asset intake, and the room frame the UI presents. It has no windowing
//! and no Tauri types: it hands out [`ClientEvent`]s and holds the current
//! frame as PNG bytes behind a [`FrameStore`], so any frontend can sit on it.
//!
//! ```text
//!   palace-wire ─┐
//!   palace-room ─┤            ┌─ ClientEvent (status, rooms, users, chat, screen)
//!   palace-asset ┼─ Runtime ──┤
//!   palace-render┘            └─ FrameStore (latest PNG + view geometry)
//! ```
//!
//! The connection, the asset pipeline and the compositor all run on one
//! blocking worker thread; media fetching runs on a second. Nothing here blocks
//! a UI thread, and nothing here returns raw buffers through a text channel —
//! frames are PNG bytes fetched by the presentation layer.

#![forbid(unsafe_code)]
#![cfg_attr(
    not(test),
    deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)
)]

pub mod assets;
pub mod avatar_images;
pub mod avatars;
pub mod error;
pub mod frame;
pub mod runtime;
pub mod secret;
pub mod session;
pub mod state;
pub mod trace;
pub mod xtlk;

pub use avatar_images::{encode_face_cell, AvatarImageStore};
pub use avatars::{AvatarArt, AvatarPartState, AvatarRoster, AvatarState};
pub use error::{ClientError, Result};
pub use frame::{FrameStore, ScreenState, ViewGeometry};
pub use palace_wire::messages::{ClientIdentity, Puid};
pub use palace_wire::registration::RegistrationCode;
pub use runtime::{
    ClientCommand, ClientConfig, ClientEvent, ClientEventStream, ClientHandle, ClientRuntime,
};
pub use secret::Secret;
pub use state::{ChatKind, ChatLine, ConnectionStatus, RoomInfo, ServerBanner, UserInfo};
pub use trace::Tracer;
pub use xtlk::{decrypt, encrypt};
