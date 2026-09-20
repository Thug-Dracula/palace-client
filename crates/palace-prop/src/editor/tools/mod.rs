//! Editor tool operations: paint, transform, and adjustments.
//!
//! Implemented by plan tasks T23, T24 and T25. Each submodule is owned by one
//! worker; these stubs reserve the modules so parallel tasks can each own a
//! single file while the crate keeps compiling.

pub mod adjust;
pub mod guides;
pub mod paint;
pub mod playback;
pub mod transform;
