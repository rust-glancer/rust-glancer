//! Connects authored documentation to indexed declaration and module identities.

use anyhow::Context as _;
use rg_def_map::{DefMapSource, ModuleOrigin};
use rg_ir_model::{CrateRef, FileId, ModuleRef, Span, identity::DeclarationRef};
use rg_std::UniqueVec;

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

    /// Pair saved declaration spans with the identities used for link resolution.
    /// A caller can collect this once for a file, then match each documented item's header
    /// against it. Current header offsets must first be translated into saved coordinates.
    ///
    /// Collecting declarations alone avoids requiring body analysis just to find doc owners.
    /// TODO: Include body-local doc owners when their declarations can be read without scanning
    /// body occurrences or making a whole-file highlighting request materialize function bodies.
    pub fn declaration_names(
        &self,
        crate_ref: CrateRef,
        file: FileId,
    ) -> anyhow::Result<UniqueVec<(Span, DeclarationRef)>> {
        let resolution = ResolutionView::new(self.db);
        let mut declarations = UniqueVec::new();
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
        Ok(declarations)
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
