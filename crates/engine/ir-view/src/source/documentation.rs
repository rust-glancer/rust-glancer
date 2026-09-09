//! Connects authored documentation to indexed declaration and module identities.

use anyhow::Context as _;
use rg_def_map::{DefMapSource, ModuleOrigin};
use rg_ir_model::{CrateRef, FileId, ModuleRef, Span, identity::DeclarationRef};
use rg_std::ExpectedUnique;

pub use rg_item_tree::{DocumentationPlacement, DocumentationSource};

use super::{IndexedSourceFact, IndexedSourceRole, SourceOccurrenceView};
use crate::{IndexedViewDb, lookup::resolution::ResolutionView};

/// Where to find the outer and inner docs of an indexed module.
///
/// For `mod api;`, outer comments sit beside that declaration while inner comments live in
/// `api.rs`. An inline `mod api { ... }` holds both in the declaration's syntax. A crate root
/// has only a definition file, with no `mod` declaration elsewhere.
pub struct ModuleDocumentationSource {
    /// The saved file and span of the `mod` item; absent for a crate root.
    pub declaration: Option<(FileId, Span)>,
    /// The crate root or out-of-line module's definition file. Inline modules have no separate
    /// file; an out-of-line module can also lack one if its file could not be found.
    pub definition: Option<FileId>,
}

/// Find documentation owners by saved header position without scanning every declaration.
/// Most spans cover just a name, but tuple fields and unnamed items can cover wider syntax.
/// Keep overlapping spans so a position with distinct owners remains ambiguous.
pub struct DocumentationDeclarationIndex {
    declarations: Vec<(Span, DeclarationRef)>,
    /// The furthest end seen through each entry. Unlike individual span ends, these are sorted
    /// even when a wider declaration contains another one.
    prefix_ends: Vec<u32>,
}

impl DocumentationDeclarationIndex {
    fn new(mut declarations: Vec<(Span, DeclarationRef)>) -> Self {
        declarations.sort_unstable_by_key(|(span, _)| (span.text.start, span.text.end));
        let mut end = 0;
        let prefix_ends = declarations
            .iter()
            .map(|(span, _)| {
                end = end.max(span.text.end);
                end
            })
            .collect();
        Self {
            declarations,
            prefix_ends,
        }
    }

    /// Match all spans containing this saved byte offset. Repeated occurrences of the same
    /// declaration count as one owner; distinct declarations leave the association unknown.
    pub fn declaration_at(&self, offset: u32) -> Option<DeclarationRef> {
        // Skip the prefix in which every span ends before the cursor, then stop once starts
        // pass it. A containing outer span must remain a candidate even if inner spans ended.
        let first = self.prefix_ends.partition_point(|end| *end <= offset);
        let mut owner = ExpectedUnique::new();
        for (span, declaration) in self.declarations[first..]
            .iter()
            .take_while(|(span, _)| span.text.start <= offset)
        {
            if span.contains(offset) {
                owner.push(*declaration);
            }
        }
        owner.into_option()
    }
}

/// Read the saved declarations and module locations needed to attach source docs to a scope.
/// The syntax being edited may have different offsets, so callers associate its declaration
/// headers with these saved names before choosing a documentation owner.
pub struct DocumentationSourceView<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> DocumentationSourceView<'a, 'db> {
    pub fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self { db }
    }

    /// Index saved declaration spans for link resolution. Build this once for a file, then
    /// look up each documented item's header after translating it into saved coordinates.
    ///
    /// Collecting declarations alone avoids requiring body analysis just to find doc owners.
    /// TODO: Include body-local doc owners when their declarations can be read without scanning
    /// body occurrences or making a whole-file highlighting request materialize function bodies.
    pub fn declaration_index(
        &self,
        crate_ref: CrateRef,
        file: FileId,
    ) -> anyhow::Result<DocumentationDeclarationIndex> {
        let resolution = ResolutionView::new(self.db);
        let mut declarations = Vec::new();
        for occurrence in SourceOccurrenceView::new(self.db)
            .saved_declaration_occurrences(crate_ref, file, None)
            .context("read documentation owner declarations")?
        {
            let (fact, _, _, span, role, _) = occurrence.into_parts();
            if role == IndexedSourceRole::Declaration
                && let IndexedSourceFact::Declaration(declaration) = fact
            {
                declarations.push((
                    span,
                    resolution
                        .canonical_declaration(declaration)
                        .context("canonicalize documentation owner")?,
                ));
            }
        }
        Ok(DocumentationDeclarationIndex::new(declarations))
    }

    /// File-level `//!` comments belong to the module whose definition uses that file.
    pub fn file_module(
        &self,
        crate_ref: CrateRef,
        file: FileId,
    ) -> anyhow::Result<Option<ModuleRef>> {
        self.db
            .def_map
            .module_for_inline_path(crate_ref, file, &[] as &[String])
            .context("read documented file module")
    }

    /// Locate the saved syntax that can contribute this module's outer and inner docs.
    /// Synthetic modules have no authored module syntax to inspect, so they return no source.
    pub fn module_source(
        &self,
        module: ModuleRef,
    ) -> anyhow::Result<Option<ModuleDocumentationSource>> {
        let Some(data) = self
            .db
            .module_data(module)
            .context("read documented module source")?
        else {
            return Ok(None);
        };
        let (declaration, definition) = match data.origin {
            ModuleOrigin::Root { file_id } => (None, Some(file_id)),
            ModuleOrigin::OutOfLine {
                declaration_file,
                declaration_span,
                definition_file,
                ..
            } => (Some((declaration_file, declaration_span)), definition_file),
            ModuleOrigin::Inline {
                declaration_file,
                declaration_span,
            } => (Some((declaration_file, declaration_span)), None),
            ModuleOrigin::Synthetic { .. } => return Ok(None),
        };
        Ok(Some(ModuleDocumentationSource {
            declaration,
            definition,
        }))
    }
}
