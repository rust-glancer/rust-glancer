//! Request-local reference identity and safe scan hints.
//!
//! A reference lookup starts by resolving the cursor to one or more declarations. `ReferenceSubject`
//! stores those declarations and their source locations. `ReferenceSearchHints` stores optional
//! shortcuts, for example "this local binding only needs its own file" or "candidate references
//! must be spelled `field`".
//!
//! Hints are only used to skip work. The actual reference decision still comes from resolving a
//! source symbol back to the selected declarations.

use std::collections::HashSet;

use rg_ir_model::{DefMapRef, identity::DeclarationRef};
use rg_ir_view::{
    IndexedViewDb, SymbolKind,
    item::declaration::{Declaration, DeclarationView},
    source::{IndexedSourceSurface, SourceOccurrenceView},
};
use rg_parse::{FileId, Span};
use rg_std::UniqueVec;

use crate::{model::SymbolAt, source_symbol::SourceSymbol};

use super::search::{ReferenceScanTarget, ReferenceSearchLabel};

/// Request-local declaration identity for one reference lookup.
pub(super) struct ReferenceSubject {
    declarations: HashSet<DeclarationRef>,
    declaration_locations: Vec<ReferenceSourceLocation>,
}

impl ReferenceSubject {
    pub(super) fn resolve(
        db: &IndexedViewDb<'_>,
        declarations: &[DeclarationRef],
    ) -> anyhow::Result<(Self, ReferenceSearchHints)> {
        let declaration_view = DeclarationView::new(db);
        let mut declaration_locations = Vec::new();
        let mut exact_labels = UniqueVec::new();
        let mut exact_labels_supported = true;

        for declaration_ref in declarations {
            let Some(declaration) = declaration_view.declaration(*declaration_ref)? else {
                exact_labels_supported = false;
                continue;
            };

            if let Some(label) =
                ReferenceSearchHints::exact_label_for_declaration(*declaration_ref, &declaration)
            {
                exact_labels.push(label);
            } else {
                exact_labels_supported = false;
            }

            declaration_locations.push(ReferenceSourceLocation {
                declaration: *declaration_ref,
                target: declaration.target(),
                file_id: declaration.file_id(),
                span: declaration.selection_span(),
            });
        }

        if !exact_labels_supported {
            exact_labels = UniqueVec::new();
        }

        let local_scan_target =
            ReferenceSearchHints::build_local_scan_target(declarations, &declaration_locations);
        let subject = Self {
            declarations: declarations.iter().copied().collect(),
            declaration_locations,
        };
        let hints = ReferenceSearchHints {
            exact_labels,
            local_scan_target,
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
    exact_labels: UniqueVec<ReferenceSearchLabel>,
    local_scan_target: Option<ReferenceScanTarget>,
}

impl ReferenceSearchHints {
    pub(super) fn exact_labels(&self) -> Vec<ReferenceSearchLabel> {
        self.exact_labels.iter().cloned().collect()
    }

    pub(super) fn local_scan_target(&self) -> Option<ReferenceScanTarget> {
        self.local_scan_target
    }

    /// Exact labels are intentionally limited to names whose references cannot be imported under
    /// another spelling. Import-aliasable declarations stay on the full semantic path.
    pub(super) fn rejects_candidate(
        &self,
        db: &IndexedViewDb<'_>,
        candidate: &SourceSymbol,
    ) -> anyhow::Result<bool> {
        if self.exact_labels.is_empty() {
            return Ok(false);
        }
        let Some(label) = Self::candidate_source_label(db, candidate)? else {
            return Ok(false);
        };
        let Some(label) = ReferenceSearchLabel::new(&label) else {
            return Ok(false);
        };
        Ok(!self.exact_labels.iter().any(|expected| expected == &label))
    }

    fn exact_label_for_declaration(
        declaration_ref: DeclarationRef,
        declaration: &Declaration,
    ) -> Option<ReferenceSearchLabel> {
        match declaration.kind() {
            SymbolKind::Field | SymbolKind::Method | SymbolKind::Variable => {
                ReferenceSearchLabel::new(declaration.name())
            }
            SymbolKind::Const
            | SymbolKind::Enum
            | SymbolKind::EnumVariant
            | SymbolKind::Function
            | SymbolKind::Impl
            | SymbolKind::Macro
            | SymbolKind::Module
            | SymbolKind::Static
            | SymbolKind::Struct
            | SymbolKind::Trait
            | SymbolKind::TypeAlias
            | SymbolKind::Union => matches!(declaration_ref, DeclarationRef::BodyBinding(_))
                .then(|| ReferenceSearchLabel::new(declaration.name()))
                .flatten(),
        }
    }

    fn candidate_source_label(
        db: &IndexedViewDb<'_>,
        candidate: &SourceSymbol,
    ) -> anyhow::Result<Option<String>> {
        match candidate.symbol() {
            SymbolAt::TypePath { path, .. }
            | SymbolAt::ValuePath { path, .. }
            | SymbolAt::UsePath { path, .. } => Ok(path.last_segment_label()),
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

    fn build_local_scan_target(
        declarations: &[DeclarationRef],
        locations: &[ReferenceSourceLocation],
    ) -> Option<ReferenceScanTarget> {
        if declarations.is_empty()
            || locations.is_empty()
            || !declarations.iter().all(Self::is_body_local_declaration)
        {
            return None;
        }

        let first = locations.first()?;
        if locations
            .iter()
            .all(|location| location.target == first.target && location.file_id == first.file_id)
        {
            Some(ReferenceScanTarget {
                target: first.target,
                file_id: Some(first.file_id),
            })
        } else {
            None
        }
    }

    fn is_body_local_declaration(declaration: &DeclarationRef) -> bool {
        match *declaration {
            DeclarationRef::Module(module) => matches!(module.origin, DefMapRef::Body(_)),
            DeclarationRef::LocalDef(local_def) => matches!(local_def.origin, DefMapRef::Body(_)),
            DeclarationRef::Item(item) => matches!(item.origin(), DefMapRef::Body(_)),
            DeclarationRef::Field(field) => matches!(field.owner.origin, DefMapRef::Body(_)),
            DeclarationRef::EnumVariant(variant) => {
                matches!(variant.origin, DefMapRef::Body(_))
            }
            DeclarationRef::BodyBinding(_) => true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ReferenceSourceLocation {
    pub(super) declaration: DeclarationRef,
    pub(super) target: rg_ir_model::TargetRef,
    pub(super) file_id: FileId,
    pub(super) span: Span,
}
