//! `palace-wire` — the Palace wire protocol.
//!
//! This crate is deliberately UI-free and dependency-light: it is the layer the
//! later Tauri milestones build on.
//!
//! ## The shape of a Palace session
//!
//! 1. The server speaks first with `MSG_TIYID` (a zero-length frame). Its 12
//!    bytes reveal the session byte order — see [`ByteOrder::from_banner`].
//! 2. The client answers with `MSG_LOGON` (`regi`) carrying an
//!    [`AuxRegistrationRec`].
//! 3. The server replies with a burst of messages (`rep2`, `vers`, `sinf`,
//!    `log `, `HTTP`, `room`, `rprs`, `endr`, `nprs`, …).
//! 4. The connection then stays open. Keepalive is `ping` → `pong`; leaving is
//!    `bye `.
//!
//! ## Endianness is a session-wide decision
//!
//! [`Reader`] and [`Writer`] carry a [`ByteOrder`] and are the *only* place
//! integers are interpreted. No call site performs a conditional swap. Both
//! directions are covered by unit tests with byte fixtures.
//!
//! ## Tolerant parsing
//!
//! Unknown opcodes are values, not errors: [`Message::decode`] yields
//! [`Message::Unknown`], which callers log and skip. Framing errors, by
//! contrast, are fatal because the stream cannot be trusted afterwards.
//!
//! ## Quick start
//!
//! ```
//! use palace_wire::{ByteOrder, Frame};
//! use palace_wire::opcode::LOGON;
//!
//! let frame = Frame::empty(LOGON, 0);
//! let bytes = frame.encode(ByteOrder::Little).unwrap();
//! let back = Frame::decode_from(&bytes, ByteOrder::Little).unwrap();
//! assert_eq!(back, frame);
//! assert_eq!(back.opcode.value(), 0x7265_6769);
//! ```

pub mod byteorder;
pub mod error;
pub mod fixture;
pub mod frame;
pub mod messages;
pub mod opcode;
pub mod registration;

pub use byteorder::{AlignedPString, ByteOrder, Reader, Writer};
pub use error::{Result, WireError, MAX_PAYLOAD_LEN};
pub use fixture::{CapturedFrame, Direction, Fixture};
pub use frame::{is_room_desc_end, navr_frame, read_handshake, Frame, Handshake, HEADER_LEN};
pub use messages::Message;
pub use opcode::Opcode;
pub use registration::RegistrationCode;
