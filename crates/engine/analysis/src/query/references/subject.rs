//! Request-local reference identity and safe scan hints.
//!
//! A reference lookup starts by resolving the cursor to one or more declarations. `ReferenceSubject`
//! stores those declarations and their source locations. `ReferenceSearchHints` stores optional
//! shortcuts, for example "this local binding only needs its owning body" or "candidate references
//! must be spelled `field`".
//!
//! Hints are only used to skip work. The actual reference decision still comes from resolving a
//! source symbol back to the selected declarations.
//!
//! Named items are searched by their declaration spelling. For example, searching `User` after
//! `use User as Account` finds the import's `User`, but does not follow uses spelled `Account`.
//! Find References also searches `Self` for nominal types. A type rename only edits its name.
//! Modules also search `self` and `super`. Crate roots add `crate` and the crate name.

use std::collections::HashSet;

use rg_ir_model::{
    BodyKey, CrateRef, DefMapRef, FileId, SemanticItemRef, Span, identity::DeclarationRef,
};
use rg_ir_view::{
    IndexedViewDb, SymbolKind,
    item::declaration::{Declaration, DeclarationView},
    source::{IndexedSourceSurface, ReferenceSourceSelection, SourceOccurrenceView},
};
use rg_std::UniqueVec;

use super::{ReferenceSearchKind, ReferenceSearchLabel};
use crate::{model::SymbolAt, source_symbol::SourceSymbol};

/// Request-local declaration identity for one reference lookup.
pub(super) struct ReferenceSubject {
    declarations: HashSet<DeclarationRef>,
    declaration_locations: Vec<ReferenceSourceLocation>,
}

impl ReferenceSubject {
    /// Prepare source locations and scan hints for the declarations selected at the cursor.
    pub(super) fn resolve(
        db: &IndexedViewDb<'_>,
        declarations: &[DeclarationRef],
        kind: ReferenceSearchKind,
    ) -> anyhow::Result<(Self, ReferenceSearchHints)> {
        let declaration_view = DeclarationView::new(db);
        let mut declaration_locations = Vec::new();
        let mut exact_labels = UniqueVec::new();
        let mut exact_labels_supported = true;
        let mut local_owners = Some(UniqueVec::new());
        let mut modules = Vec::new();
        let mut types = Vec::new();
        let mut macro_definitions = Vec::new();

        for declaration_ref in declarations {
            let declaration = declaration_view.declaration(*declaration_ref)?;

            // A crate root has no `mod` declaration or name span, but its crate name and relative
            // keywords still narrow the scan. Collect those spellings even without a declaration.
            if let DeclarationRef::Module(module) = *declaration_ref {
                modules.push(module);
                let labels = ReferenceSearchLabel::for_module(db, module)?;
                if labels.is_empty() {
                    exact_labels_supported = false;
                }
                exact_labels.extend(labels);
            } else if let Some(declaration) = &declaration {
                if let DeclarationRef::LocalDef(definition) = *declaration_ref
                    && declaration.kind() == SymbolKind::Macro
                {
                    macro_definitions.push(definition);
                }
                if let DeclarationRef::Item(SemanticItemRef::TypeDef(ty)) = *declaration_ref {
                    types.push(ty);
                }
                if let Some(label) = ReferenceSearchHints::exact_label_for_declaration(declaration)
                {
                    exact_labels.push(label);
                    if kind == ReferenceSearchKind::FindReferences
                        && matches!(
                            declaration.kind(),
                            SymbolKind::Struct
                                | SymbolKind::Enum
                                | SymbolKind::Union
                                | SymbolKind::TypeAlias
                        )
                    {
                        exact_labels.push(
                            ReferenceSearchLabel::identifier("Self")
                                .expect("Self is an identifier"),
                        );
                    }
                } else {
                    exact_labels_supported = false;
                }
            } else {
                exact_labels_supported = false;
            }

            // Body identity survives expansion and includes. Its whole worklist contains nested
            // declarations too; a physical file alone would both scan unrelated functions and
            // miss uses in an included file. Keep every owner represented by a macro argument.
            if let Some(owners) = &mut local_owners {
                if let DefMapRef::Body(body) = declaration_ref.origin() {
                    owners.push(body.key());
                } else {
                    local_owners = None;
                }
            }

            // Navigation may lead to a whole macro invocation. Only an actual source occurrence
            // can be projected back into references, which rename also consumes as editable text.
            if let Some(declaration) = declaration
                && let Some(span) = declaration.editable_span()
            {
                declaration_locations.push(ReferenceSourceLocation {
                    declaration: *declaration_ref,
                    crate_ref: declaration.crate_ref(),
                    file_id: declaration.file_id(),
                    span,
                });
            }
        }

        // One filter covers every selected declaration. If any target has an unknown or unsupported
        // spelling, a partial list could hide its references. An empty list disables filtering.
        if !exact_labels_supported {
            exact_labels = UniqueVec::new();
        }
        // A copied macro argument may also name a value. Its uses still need the full body scan.
        let macro_definitions =
            (macro_definitions.len() == declarations.len()).then_some(macro_definitions);

        let subject = Self {
            declarations: declarations.iter().copied().collect(),
            declaration_locations,
        };
        let hints = ReferenceSearchHints {
            selection: ReferenceSourceSelection {
                labels: exact_labels.into_vec(),
                modules,
                types,
                macro_definitions,
            },
            local_owners: local_owners.map(UniqueVec::into_vec),
        };
        Ok((subject, hints))
    }

    pub(super) fn declarations(&self) -> &HashSet<DeclarationRef> {
        &self.declarations
    }

    pub(super) fn declaration_locations(&self) -> &[ReferenceSourceLocation] {
        &self.declaration_locations
    }
}

/// Request-local shortcuts that can narrow a reference scan without changing semantic matching.
pub(super) struct ReferenceSearchHints {
    pub(super) selection: ReferenceSourceSelection,
    local_owners: Option<Vec<BodyKey>>,
}

impl ReferenceSearchHints {
    pub(super) fn local_owners(&self) -> Option<&[BodyKey]> {
        self.local_owners.as_deref()
    }

    /// Skip unrelated spellings before semantic resolution. Imported aliases with another spelling
    /// are not followed; matching names still have to resolve to the selected declaration.
    pub(super) fn rejects_candidate(
        &self,
        db: &IndexedViewDb<'_>,
        candidate: &SourceSymbol,
    ) -> anyhow::Result<bool> {
        if self.selection.labels.is_empty() {
            return Ok(false);
        }
        let Some(label) = Self::candidate_source_label(db, candidate)? else {
            return Ok(false);
        };
        let Some(label) = ReferenceSearchLabel::identifier(&label) else {
            return Ok(false);
        };
        Ok(!self
            .selection
            .labels
            .iter()
            .any(|expected| expected.as_str() == label.as_str()))
    }

    fn exact_label_for_declaration(declaration: &Declaration) -> Option<ReferenceSearchLabel> {
        match declaration.kind() {
            // Anonymous impls have no searchable name.
            SymbolKind::Impl => None,
            _ => declaration
                .semantic_name()
                .and_then(|name| ReferenceSearchLabel::identifier(name)),
        }
    }

    fn candidate_source_label(
        db: &IndexedViewDb<'_>,
        candidate: &SourceSymbol,
    ) -> anyhow::Result<Option<String>> {
        match candidate.symbol() {
            SymbolAt::TypePath { type_path, .. } => Ok(type_path.path().last_segment_label()),
            SymbolAt::ValuePath { path, .. } | SymbolAt::UsePath { path, .. } => {
                Ok(path.last_segment_label())
            }
            SymbolAt::RecordField { key, .. } => Ok(Some(key.declaration_label())),
            SymbolAt::Expr { expr } => {
                if let IndexedSourceSurface::RecordExprShorthandValue { key, .. } =
                    candidate.surface()
                {
                    return Ok(Some(key.declaration_label()));
                }
                SourceOccurrenceView::new(db).expr_source_label(*expr)
            }
            SymbolAt::Declaration { .. } | SymbolAt::FunctionBody { .. } => Ok(None),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ReferenceSourceLocation {
    pub(super) declaration: DeclarationRef,
    pub(super) crate_ref: CrateRef,
    pub(super) file_id: FileId,
    pub(super) span: Span,
}
