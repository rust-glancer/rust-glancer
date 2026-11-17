//! Resolve saved declaration occurrences against the source snapshot of this operation.
//!
//! Saved declarations retain source IDs even when byte positions change. This module joins their
//! source identity to the file tables read through the operation's query context. Generated syntax
//! takes an extra step: its occurrences identify input tokens, which lead back to those file tables.

use rg_def_map::{GeneratedExpansionLocations, GeneratedLocations, ItemSource, ItemSourceKind};
use rg_ir_model::{CrateRef, FileId, PackageSlot, SourceId, SourceLocation, Span};
use rg_item_tree::{DeclarationSourceStore, SourceLocations, file_declaration_sources};
use rg_source::QueryContext;

use crate::target_declarations;

/// The location tables needed to read occurrences from one saved declaration source.
///
/// Open this view using the declaration's [`ItemSource`], then resolve its [`SourceId`] values
/// through [`SourceLocations`]. The source chooses the table; an ID alone cannot do that.
///
/// Opening the view performs the query reads and can fail. Afterward, occurrence lookup only
/// reads borrowed tables: it performs no further queries, though a missing or unmapped occurrence
/// can still return `None`. Reuse the opened view while walking one declaration's signature.
pub enum SavedDeclarationSourceView<'db> {
    File(&'db DeclarationSourceStore<Span>),
    Generated {
        locations: &'db GeneratedExpansionLocations,
        inputs: Vec<((PackageSlot, FileId), &'db DeclarationSourceStore<Span>)>,
    },
}

impl<'db> SavedDeclarationSourceView<'db> {
    /// Acquire the file mappings for this saved source. Builtin derives can inherit another
    /// source's IDs, so follow that chain until reaching an actual file or expansion table.
    /// Body-owned declarations need the body reader, which also knows about current editor bodies.
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
                    let declarations = target_declarations(db, krate)?;
                    let sources = declarations.generated_sources.value_ref(db)?;
                    let locations = sources
                        .get(generated.source)
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
                        "saved source reader received a body declaration",
                    ));
                }
            }
        }
    }
}

impl SourceLocations for SavedDeclarationSourceView<'_> {
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
                    editable: location.editable,
                })
            }
        }
    }
}
