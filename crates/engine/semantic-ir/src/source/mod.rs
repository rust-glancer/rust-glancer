//! Resolve declaration occurrences against the source snapshot of this operation.
//!
//! Declarations retain source IDs even when byte positions change. This module joins their
//! source identity to the file tables read through the operation's query context. Generated syntax
//! takes an extra step: its occurrences identify input tokens, which lead back to those file tables.

mod ownership;
mod resolve;

use rg_def_map::{
    GeneratedBodySyntax, GeneratedExpansionLocations, GeneratedItemRef, GeneratedLocations,
    GeneratedSourceStore, GeneratedSourceStoreId, ItemSource, ItemSourceKind,
};
use rg_ir_model::{
    CrateRef, FileId, ItemStoreId, PackageSlot, SourceId, SourceLocation, SourceLocationKind, Span,
};
use rg_item_tree::{DeclarationSourceStore, SourceLocations, file_declaration_sources};
use rg_source::QueryContext;

pub(crate) use self::resolve::DeclarationSourceResolver;
pub use self::{
    ownership::{SourceOwnership, SourceOwnershipIndex, complete_source_ownership},
    resolve::SourceSelection,
};
use crate::{BodyOwnerData, associated_declarations, target_declarations};

/// The source tables needed to read occurrences and previews from one declaration source.
///
/// Open this view using the declaration's [`ItemSource`], then resolve its [`SourceId`] values
/// through [`SourceLocations`]. The source chooses the table; an ID alone cannot do that.
///
/// Opening the view performs the query reads and can fail. Afterward, occurrence lookup only
/// reads borrowed tables: it performs no further queries, though a missing or unmapped occurrence
/// can still return `None`. Reuse the opened view while walking one declaration's signature.
pub enum ItemSourceView<'db> {
    File(&'db DeclarationSourceStore<Span>),
    Generated {
        locations: &'db GeneratedExpansionLocations,
        inputs: Vec<((PackageSlot, FileId), &'db DeclarationSourceStore<Span>)>,
    },
}

impl<'db> ItemSourceView<'db> {
    /// Acquire the file mappings for this source. Builtin derives can inherit another
    /// source's IDs, so follow that chain until reaching an actual file or expansion table.
    /// Body-owned declarations need the body reader, which owns their resolved occurrence tables.
    pub fn new(
        db: &'db QueryContext<'db>,
        krate: CrateRef,
        mut source: ItemSource,
    ) -> ajika::Result<Self> {
        loop {
            match source.kind {
                ItemSourceKind::ItemTree(_) => {
                    return Ok(Self::File(file_declaration_sources(
                        db,
                        krate.package,
                        source.file_id,
                    )?));
                }
                ItemSourceKind::Generated(generated) | ItemSourceKind::Synthetic(generated) => {
                    // Expansion slots are local to the query that produced them. The source ref
                    // routes directly to module output or one member owner, without searching others.
                    let sources = Self::generated_sources(db, krate, generated.source_store())?;
                    let locations = sources
                        .get(generated.source())
                        .ok_or_else(|| ajika::Error::application("generated source is absent"))?;
                    match locations {
                        GeneratedLocations::Inherited(input) => source = *input,
                        GeneratedLocations::Expansion(locations) => {
                            // The dependency list belongs to the whole expansion. Load it before
                            // walking occurrences, rather than performing queries for each token.
                            let mut inputs = Vec::with_capacity(locations.input_files().len());
                            for &(package, file) in locations.input_files() {
                                inputs.push((
                                    (package, file),
                                    file_declaration_sources(db, package, file)?,
                                ));
                            }
                            return Ok(Self::Generated { locations, inputs });
                        }
                    }
                }
                ItemSourceKind::Body(_) => {
                    return Err(ajika::Error::application(
                        "item source reader received a body declaration",
                    ));
                }
            }
        }
    }

    /// Open body syntax from its original expansion store, independently of its semantic owner.
    pub fn generated_body_syntax(
        db: &'db QueryContext<'db>,
        krate: CrateRef,
        item: GeneratedItemRef,
    ) -> ajika::Result<Option<GeneratedBodySyntax>> {
        Self::generated_sources(db, krate, item.source_store())?.body_syntax(db, krate, item)
    }

    /// Return the invocation that bounds a generated body's exact source occurrences in its file.
    ///
    /// A generated item's full declaration maps to the invocation used to lower its body.
    /// For `make!(name)`, exact occurrences in the generated body must be copied from that
    /// invocation. Read the declaration's origin without reopening the expanded tokens.
    pub fn generated_body_invocation(
        db: &'db QueryContext<'db>,
        owner: &BodyOwnerData,
    ) -> ajika::Result<Option<Span>> {
        let ItemSourceKind::Generated(item) = owner.source.kind else {
            return Ok(None);
        };
        let sources =
            Self::generated_sources(db, owner.module.origin.origin_crate(), item.source_store())?;
        let Some(GeneratedLocations::Expansion(locations)) = sources.get(item.source()) else {
            return Ok(None);
        };
        let Some(location) = locations
            .get(owner.location)
            .filter(|location| !location.editable)
        else {
            return Ok(None);
        };
        let (package, file) = location.input_file();
        if file != owner.file {
            return Ok(None);
        }
        let inputs = file_declaration_sources(db, package, file)?;
        Ok(inputs.token_span(location.origin))
    }

    fn generated_sources(
        db: &'db QueryContext<'db>,
        krate: CrateRef,
        store: GeneratedSourceStoreId,
    ) -> ajika::Result<&'db GeneratedSourceStore> {
        let owner = match store {
            GeneratedSourceStoreId::Target => {
                return target_declarations(db, krate)?
                    .generated_sources
                    .value_ref(db);
            }
            GeneratedSourceStoreId::TraitMembers(id) => ItemStoreId::Trait(id),
            GeneratedSourceStoreId::ImplMembers(id) => ItemStoreId::Impl(id),
        };
        Self::member_sources(db, krate, owner)
    }

    fn member_sources(
        db: &'db QueryContext<'db>,
        krate: CrateRef,
        owner: ItemStoreId,
    ) -> ajika::Result<&'db GeneratedSourceStore> {
        associated_declarations(db, krate, owner)?
            .as_ref()
            .ok_or_else(|| ajika::Error::application("generated source owner is absent"))?
            .sources
            .value_ref(db)
    }

    pub fn initializer_preview(&self, occurrence: SourceId) -> Option<&'db str> {
        match self {
            Self::File(source) => source.initializer_preview(occurrence),
            Self::Generated { locations, .. } => locations.initializer_preview(occurrence),
        }
    }
}

impl SourceLocations for ItemSourceView<'_> {
    fn location(&self, occurrence: SourceId) -> Option<SourceLocation> {
        match self {
            Self::File(locations) => locations.location(occurrence),
            Self::Generated { locations, inputs } => {
                // The first lookup finds an input token reference. Only the input file's table
                // can turn that reference into byte offsets for this operation's snapshot.
                let location = locations.get(occurrence)?;
                let input = location.input_file();
                let sources = inputs.iter().find(|(key, _)| *key == input)?.1;
                Some(SourceLocation {
                    span: sources.token_span(location.origin)?,
                    kind: if location.editable {
                        SourceLocationKind::MacroArgument
                    } else {
                        SourceLocationKind::NavigationFallback
                    },
                })
            }
        }
    }
}
