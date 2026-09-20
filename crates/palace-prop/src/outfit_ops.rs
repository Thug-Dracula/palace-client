//! Outfit operations: save the worn set, apply an outfit, manage outfits.
//!
//! This is the thin behaviour layer over [`crate::outfits`], the outfits store
//! (plan task T7). Every storage detail — the portable `Outfits.prp` document,
//! its parser, its atomic writes and its keying by `(id, crc)` — lives in that
//! module and is reached through [`OutfitStore`]. Nothing here re-implements or
//! bypasses it.
//!
//! # Operations
//!
//! * [`save_current_outfit`] records the props currently worn under a name.
//! * [`apply_outfit`] turns a name back into the list of props to wear.
//! * [`list_outfits`], [`rename_outfit`], [`delete_outfit`] and
//!   [`duplicate_outfit`] manage the stored set.
//!
//! # Applying an outfit that mentions something you do not own
//!
//! An outfit file is portable: it can be copied from another client or another
//! machine, so it may name props that the local collection no longer carries.
//! Applying such an outfit must not fail as a whole. [`apply_outfit`] takes the
//! set of keys the caller can actually wear and splits the outfit's list into
//! what is available (the [`OutfitApplication::worn`] list, in wearing order)
//! and what is not ([`OutfitApplication::missing`], also in outfit order). The
//! caller wears `worn`; the rest is reported so the UI can say what is absent.
//! An outfit is never partially written back or "repaired" because of this.
//!
//! Identity is always the [`PropKey`] pair `(id, crc)`. A bare id is not unique
//! (several records may share an id and differ in crc), so every list here is a
//! list of pairs, exactly as the store persists them.
//!
//! No outfit is ever bound to a keyboard key: an outfit is applied by name.

use std::collections::HashSet;

use crate::error::{PropError, Result};
use crate::outfits::{Outfit, OutfitStore};
use crate::prp::PropKey;

/// The result of preparing an outfit for wearing.
///
/// [`OutfitApplication::worn`] holds the props to put on, in wearing order;
/// [`OutfitApplication::missing`] holds the outfit's references the caller did
/// not offer as available, in the same order. Missing references do not fail an
/// apply — they are reported so the caller can still wear the rest.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OutfitApplication {
    /// The available props to wear, in the outfit's order.
    pub worn: Vec<PropKey>,
    /// The outfit's references that were not in the caller's available set, in
    /// the outfit's order.
    pub missing: Vec<PropKey>,
}

impl OutfitApplication {
    /// Whether every reference in the outfit was available.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.missing.is_empty()
    }

    /// Whether the outfit named nothing at all (empty list).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.worn.is_empty() && self.missing.is_empty()
    }

    /// Total number of references the outfit named, worn plus missing.
    #[must_use]
    pub fn total(&self) -> usize {
        self.worn.len().saturating_add(self.missing.len())
    }
}

/// Record `worn` as the outfit called `name`, then save the file.
///
/// The list is stored verbatim, in order, keyed by `(id, crc)`. The name is
/// trimmed by the store; an empty name is refused. An existing outfit with the
/// same name is replaced in place. Storage is entirely [`OutfitStore::save_worn`].
pub fn save_current_outfit(store: &mut OutfitStore, name: &str, worn: &[PropKey]) -> Result<()> {
    store.save_worn(name, worn)
}

/// Prepare the outfit called `name` for wearing against `available`.
///
/// Returns the references to wear, in order, split from the ones the caller did
/// not list as available. An unknown name is an error; a missing reference is
/// **not** — it is reported in [`OutfitApplication::missing`] and the rest still
/// apply.
///
/// `available` is the identity set the caller can currently wear (for example
/// the keys present in the local collection). It is matched on the exact
/// `(id, crc)` pair, never on the id alone.
pub fn apply_outfit(
    store: &OutfitStore,
    name: &str,
    available: &[PropKey],
) -> Result<OutfitApplication> {
    let outfit = stored_outfit(store, name)?;
    let available: HashSet<PropKey> = available.iter().copied().collect();

    let mut application = OutfitApplication::default();
    for key in &outfit.props {
        if available.contains(key) {
            application.worn.push(*key);
        } else {
            application.missing.push(*key);
        }
    }
    Ok(application)
}

/// Every stored outfit name, in file order.
#[must_use]
pub fn list_outfits(store: &OutfitStore) -> Vec<&str> {
    store.names()
}

/// Rename the outfit `from` to `to`, then save the file.
///
/// Refused when `from` is unknown, when `to` is already another outfit's name,
/// or when `to` is empty. Renaming an outfit to its own name is a no-op.
pub fn rename_outfit(store: &mut OutfitStore, from: &str, to: &str) -> Result<()> {
    store.rename(from, to)
}

/// Delete the outfit called `name`, then save the file.
///
/// Refused when the name is unknown.
pub fn delete_outfit(store: &mut OutfitStore, name: &str) -> Result<()> {
    store.delete(name)
}

/// Copy the outfit `source` to a new outfit `target`, then save the file.
///
/// The copy is inserted directly after its source. Refused when `source` is
/// unknown, when `target` is already taken, or when `target` is empty.
pub fn duplicate_outfit(store: &mut OutfitStore, source: &str, target: &str) -> Result<()> {
    store.duplicate(source, target)
}

/// The stored outfit `name`, or the same "no such outfit" error the store uses.
fn stored_outfit<'a>(store: &'a OutfitStore, name: &str) -> Result<&'a Outfit> {
    let name = name.trim();
    store.get(name).ok_or_else(|| PropError::BagIo {
        detail: format!("outfits: no outfit named \"{name}\""),
    })
}
