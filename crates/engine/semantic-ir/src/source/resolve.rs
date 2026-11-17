//! Select semantic owners before acquiring declarations at a source position.

use rg_ir_model::{CrateRef, FileId, ItemStoreId, ItemStoreRef, Span};
use rg_item_tree::SourceLocations as _;
use rg_source::QueryContext;

use crate::{ItemSourceView, complete_source_ownership, target_declarations};

/// Choose the source surface an operation needs. A written outline and an exhaustive reference
/// search are different: the latter must also see macros elsewhere that include this same file.
#[derive(Debug, Clone, Copy)]
pub enum SourceSelection {
    /// Written containers at a cursor position. If the file is only known through includes,
    /// reverse discovery supplies its enclosing owners.
    Written {
        file: FileId,
        offset: u32,
    },
    /// Written containers touching a requested range, including its endpoints. Owners that
    /// contain the range still contribute, even when their headers are outside it.
    WrittenRange {
        file: FileId,
        range: Span,
    },
    /// Every interpretation of this physical file, including reused include input. May expand
    /// unrelated owners to discover whether their macros include this file.
    AllInFile(FileId),
    All,
}

impl SourceSelection {
    pub fn at(file: FileId, offset: u32) -> Self {
        Self::Written { file, offset }
    }

    pub fn in_range(file: FileId, range: Span) -> Self {
        Self::WrittenRange { file, range }
    }

    /// Cursor and viewport reads use the same owner filtering; a cursor has zero-width bounds.
    pub(crate) fn bounds(self) -> Option<Span> {
        match self {
            Self::Written { offset, .. } => Some(Span {
                start: offset,
                end: offset,
            }),
            Self::WrittenRange { range, .. } => Some(range),
            Self::AllInFile(_) | Self::All => None,
        }
    }

    pub fn offset(self) -> Option<u32> {
        match self {
            Self::Written { offset, .. } => Some(offset),
            _ => None,
        }
    }

    pub fn file(self) -> Option<FileId> {
        match self {
            Self::Written { file, .. }
            | Self::WrittenRange { file, .. }
            | Self::AllInFile(file) => Some(file),
            Self::All => None,
        }
    }
}

/// Find which declaration containers may contribute items at a source position.
///
/// A header in this file can select its own member store without discovering unrelated macros.
/// A file known only through an include needs reverse ownership discovery. The complete result
/// keeps all include owners; it never guesses one owner from the physical source identity.
pub(crate) struct DeclarationSourceResolver<'db> {
    db: &'db QueryContext<'db>,
    krate: CrateRef,
}

impl<'db> DeclarationSourceResolver<'db> {
    pub(crate) fn new(db: &'db QueryContext<'db>, krate: CrateRef) -> Self {
        Self { db, krate }
    }

    /// Resolve the selected containers before reading their declarations. Exact declaration
    /// ranges are checked by the consumer after their stores are acquired.
    pub(crate) fn member_stores(
        &self,
        selection: SourceSelection,
    ) -> ajika::Result<Vec<ItemStoreRef>> {
        let target = target_declarations(self.db, self.krate)?;
        let Some(file) = selection.file() else {
            return Ok(target.items.value_ref(self.db)?.member_stores().collect());
        };
        let known = target.source_ownership.value_ref(self.db)?;
        let indexed = known.known_owners(file);
        let owners = if matches!(selection, SourceSelection::AllInFile(_)) || indexed.is_empty() {
            complete_source_ownership(self.db, self.krate)?.owners(file)
        } else {
            indexed
        };
        // A cursor or viewport needs header locations to narrow the enclosing trait/impl stores.
        // Whole-file reads can use ownership alone, without acquiring locations.
        let Some(bounds) = selection.bounds() else {
            return Ok(owners
                .iter()
                .copied()
                .filter(|owner| owner.id != ItemStoreId::Scope)
                .collect());
        };
        let items = target.items.value_ref(self.db)?;
        let definitions = target.definitions.value_ref(self.db)?.view(self.db);
        let mut selected = Vec::new();
        for &owner in owners {
            let (source, location) = match owner.id {
                ItemStoreId::Trait(id) => {
                    let header = items
                        .trait_data(self.db, id)?
                        .expect("ownership names a collected trait");
                    let data = definitions
                        .local_def(header.local_def.local_def)?
                        .expect("trait has collected definition");
                    (header.source, data.location)
                }
                ItemStoreId::Impl(id) => {
                    let header = items
                        .impl_data(self.db, id)?
                        .expect("ownership names a collected impl");
                    let data = definitions
                        .local_impl(header.local_impl.local_impl)?
                        .expect("impl has collected definition");
                    (header.source, data.location)
                }
                ItemStoreId::Scope => continue,
            };
            // The include's offsets belong to its own file, not the enclosing owner's header.
            if source.file_id == file {
                let locations = ItemSourceView::new(self.db, self.krate, source)?;
                if !locations
                    .location(location)
                    .is_some_and(|location| location.span.touches_span(bounds))
                {
                    continue;
                }
            }
            selected.push(owner);
        }
        Ok(selected)
    }
}
