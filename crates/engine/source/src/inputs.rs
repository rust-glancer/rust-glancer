//! Query inputs carry the inventory's observations across operations.

use std::sync::Arc;

use rg_std::NativeOsString;

use crate::SourceEntry;

#[ajika::input(key = NativeOsString)]
pub struct SourceInput {
    #[input(arc, stamp = entry_stamp)]
    entry: SourceEntry,
}

impl SourceInput {
    /// Install an entry under the same canonical path that was used to capture it.
    pub fn set_captured(
        update: &mut ajika::InputUpdate,
        entry: Arc<SourceEntry>,
    ) -> ajika::Result<()> {
        let path = NativeOsString::from_os_str(entry.path().as_os_str());
        Self::at(path).set_entry(update, entry)
    }
}

#[ajika::input(key = NativeOsString)]
pub struct SourcePresence {
    exists: bool,
}

fn entry_stamp(entry: &SourceEntry) -> ajika::Result<ajika::Stamp> {
    // Evicting saved text doesn't change what a reader can observe. SourceEntry verifies the
    // revision if it has to reload those bytes from disk.
    Ok(ajika::Stamp::from_bytes(*entry.revision().as_bytes()))
}
