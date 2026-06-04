//! Semantic rename planning over indexed source occurrences.
//!
//! Rename builds on the same symbol identity used by goto-definition and references, but keeps a
//! stricter policy: only declaration-like names with unambiguous source occurrences become edits.

use anyhow::Context as _;
use rg_ir_model::identity::DeclarationRef;
use rg_ir_view::{
    item::declaration::{Declaration, DeclarationView},
    source::IndexedSourceSurface,
};
use rg_parse::Span;
use rg_syntax::{Edition, SyntaxKind};

use crate::{
    Analysis, ReferenceQuery, SymbolKind,
    model::{RenameEdit, RenameResult, RenameTarget, SymbolAt},
    source_symbol::{SourceSymbol, SourceSymbolResolver, SourceSymbolRole},
};

use super::references::ReferenceResolver;

/// Plans semantic rename edits from the same source-symbol identity used by references.
///
/// The resolver sits at the analysis boundary: it decides whether the cursor is on a
/// declaration-like subject that is safe to rename, asks the references query for all matching
/// source occurrences, and then rewrites each occurrence according to its surface syntax.
pub(crate) struct RenameResolver<'a, 'db> {
    analysis: &'a Analysis<'db>,
}

impl<'a, 'db> RenameResolver<'a, 'db> {
    /// Creates a rename resolver for one analysis snapshot.
    pub(crate) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self { analysis }
    }

    /// Returns the source span and placeholder shown by editor prepare-rename requests.
    pub(crate) fn prepare_rename(
        &self,
        target: rg_ir_model::TargetRef,
        file_id: rg_parse::FileId,
        offset: u32,
    ) -> anyhow::Result<Option<RenameTarget>> {
        let Some(symbol) = self
            .analysis
            .source_symbol_at_for_query(target, file_id, offset)?
        else {
            return Ok(None);
        };

        self.rename_target_for_symbol(symbol)
    }

    /// Produces all text edits needed to rename the symbol selected at `offset`.
    pub(crate) fn rename(
        &self,
        target: rg_ir_model::TargetRef,
        file_id: rg_parse::FileId,
        offset: u32,
        new_name: &str,
        query: ReferenceQuery<'_>,
    ) -> anyhow::Result<Option<RenameResult>> {
        // Reject invalid replacement names before doing any semantic work, so callers get a clear
        // error instead of an empty rename caused by an impossible edit.
        anyhow::ensure!(
            Self::is_supported_new_name(new_name),
            "rename target `{new_name}` is not a supported Rust identifier"
        );

        // First resolve the cursor the same way hover/goto/references do. If there is no semantic
        // symbol under the cursor, rename is simply unavailable at this position.
        let Some(symbol) = self
            .analysis
            .source_symbol_at_for_query(target, file_id, offset)?
        else {
            return Ok(None);
        };

        // Prepare-rename policy is reused here so direct rename requests cannot edit symbols that
        // the editor would have rejected during its prepare phase.
        let Some(rename_target) = self.rename_target_for_symbol(symbol.clone())? else {
            return Ok(None);
        };

        // The selected symbol must resolve to one canonical declaration. References use that
        // declaration set as the semantic identity for every occurrence we might edit.
        let declarations = self.unique_declarations_for_symbol(symbol.symbol().clone())?;

        // Reference resolution gives us source occurrences. Rename then handles the final surface
        // rewrite, because shorthand records may need a larger replacement than the selected span.
        let edits = ReferenceResolver::new(self.analysis, query)
            .source_symbols_matching_declarations(&declarations)?
            .into_iter()
            .map(|symbol| self.rename_edit_for_symbol(symbol, &rename_target.placeholder, new_name))
            .collect::<anyhow::Result<Vec<_>>>()
            .context("while attempting to plan rename edits")?;

        // A semantic occurrence can be reachable through more than one source candidate. Collapse
        // exact duplicates and fail loudly if two planned edits would fight over the same text.
        let edits = Self::normalize_rename_edits(edits)?;

        Ok(Some(RenameResult {
            target: rename_target,
            edits,
        }))
    }

    /// Converts one matched source occurrence into the concrete source edit for its spelling.
    fn rename_edit_for_symbol(
        &self,
        symbol: SourceSymbol,
        old_name: &str,
        new_name: &str,
    ) -> anyhow::Result<RenameEdit> {
        // Plain references can replace the selected text directly. Shorthand record syntax carries
        // two names in one span, so those occurrences expand into explicit `field: value` spelling.
        let (span, old_text, new_text) = match symbol.surface().clone() {
            IndexedSourceSurface::Plain | IndexedSourceSurface::RecordFieldKeyExplicit => {
                (symbol.span(), old_name.to_string(), new_name.to_string())
            }
            IndexedSourceSurface::RecordExprShorthandFieldKey { .. } => (
                symbol.span(),
                old_name.to_string(),
                format!("{new_name}: {old_name}"),
            ),
            IndexedSourceSurface::RecordExprShorthandValue { key, .. } => (
                symbol.span(),
                old_name.to_string(),
                format!("{}: {new_name}", key.declaration_label()),
            ),
            IndexedSourceSurface::RecordPatShorthandFieldKey {
                field_span,
                pat_span,
            } => {
                let old_text = self.source_text_for_span(&symbol, field_span)?;
                let pat_text = self.source_text_for_span(&symbol, pat_span)?;
                (field_span, old_text, format!("{new_name}: {pat_text}"))
            }
            IndexedSourceSurface::RecordPatShorthandBinding {
                key,
                field_span,
                pat_span,
                binding_name_span,
            } => {
                let old_text = self.source_text_for_span(&symbol, field_span)?;
                let pat_text = self.source_text_for_span(&symbol, pat_span)?;
                let pat_text = Self::replace_text_inside_span(
                    pat_span,
                    binding_name_span,
                    &pat_text,
                    new_name,
                )
                .context("while attempting to rewrite record pattern shorthand binding")?;
                (
                    field_span,
                    old_text,
                    format!("{}: {pat_text}", key.declaration_label()),
                )
            }
        };

        Ok(RenameEdit {
            target: symbol.target(),
            file_id: symbol.file_id(),
            span,
            old_text,
            new_text,
        })
    }

    /// Reads the exact source text covered by a rename surface span.
    fn source_text_for_span(&self, symbol: &SourceSymbol, span: Span) -> anyhow::Result<String> {
        self.analysis
            .source_text_for_span(symbol.target().package, symbol.file_id(), span)
            .with_context(|| "while attempting to read source text for rename edit")
    }

    /// Replaces a child span inside already-loaded parent source text.
    fn replace_text_inside_span(
        parent_span: Span,
        child_span: Span,
        parent_text: &str,
        new_text: &str,
    ) -> anyhow::Result<String> {
        anyhow::ensure!(
            parent_span.text.start <= child_span.text.start
                && child_span.text.end <= parent_span.text.end,
            "rename child span is outside parent span"
        );

        let start = usize::try_from(child_span.text.start - parent_span.text.start)
            .context("while attempting to compute rename child span start")?;
        let end = usize::try_from(child_span.text.end - parent_span.text.start)
            .context("while attempting to compute rename child span end")?;
        anyhow::ensure!(
            parent_text.get(start..end).is_some(),
            "rename child span does not align with source text"
        );

        let mut rewritten =
            String::with_capacity(parent_text.len() - (end - start) + new_text.len());
        rewritten.push_str(&parent_text[..start]);
        rewritten.push_str(new_text);
        rewritten.push_str(&parent_text[end..]);
        Ok(rewritten)
    }

    /// Sorts rename edits, removes duplicates, and rejects conflicting overlaps.
    fn normalize_rename_edits(mut edits: Vec<RenameEdit>) -> anyhow::Result<Vec<RenameEdit>> {
        edits.sort_by_key(|edit| {
            (
                edit.target.package.0,
                edit.target.target.0,
                edit.file_id.0,
                edit.span.text.start,
                edit.span.text.end,
            )
        });

        let mut normalized: Vec<RenameEdit> = Vec::new();
        for edit in edits {
            let Some(previous) = normalized.last() else {
                normalized.push(edit);
                continue;
            };
            if previous.target != edit.target || previous.file_id != edit.file_id {
                normalized.push(edit);
                continue;
            }
            if previous.span == edit.span {
                if previous.old_text == edit.old_text && previous.new_text == edit.new_text {
                    continue;
                }
                anyhow::bail!("rename produced conflicting edits for the same source span");
            }
            if previous.span.text.start < edit.span.text.end
                && edit.span.text.start < previous.span.text.end
            {
                anyhow::bail!("rename produced overlapping source edits");
            }
            normalized.push(edit);
        }

        Ok(normalized)
    }

    /// Applies rename eligibility rules to a resolved source symbol.
    fn rename_target_for_symbol(
        &self,
        symbol: SourceSymbol,
    ) -> anyhow::Result<Option<RenameTarget>> {
        // Structural candidates exist so references can highlight syntax tied to a symbol, but
        // they are not independently renameable cursor targets.
        if symbol.role() == SourceSymbolRole::Structural {
            return Ok(None);
        }

        // Rename needs one declaration identity. Ambiguous symbols are safe for navigation-style
        // features, but producing edits from them would risk changing unrelated code.
        let declarations = self.unique_declarations_for_symbol(symbol.symbol().clone())?;
        let [declaration_ref] = &declarations[..] else {
            return Ok(None);
        };
        let Some(declaration) =
            DeclarationView::new(self.analysis.view_db()).declaration(*declaration_ref)?
        else {
            return Ok(None);
        };

        // If the cursor was on a declaration occurrence, require it to be the canonical source
        // declaration. This avoids renaming through generated or alternate declaration-like spans.
        if !Self::declaration_occurrence_matches_canonical(&symbol, &declaration) {
            return Ok(None);
        }
        if !Self::is_renameable_declaration(&declaration) {
            return Ok(None);
        }

        // Path-like symbols expose the selected segment label. If it already disagrees with the
        // canonical declaration name, the occurrence is probably an alias or keyword-like path.
        if let Some(label) = Self::selected_label(symbol.symbol())
            && label != declaration.name()
        {
            return Ok(None);
        }

        Ok(Some(RenameTarget {
            file_id: symbol.file_id(),
            span: symbol.span(),
            placeholder: declaration.name().to_string(),
        }))
    }

    /// Resolves a source symbol to declarations while preserving first-seen order.
    fn unique_declarations_for_symbol(
        &self,
        symbol: SymbolAt,
    ) -> anyhow::Result<Vec<DeclarationRef>> {
        let declarations =
            SourceSymbolResolver::new(self.analysis.view_db()).declarations_for_symbol(symbol)?;
        let mut unique = Vec::new();
        for declaration in declarations {
            if !unique.contains(&declaration) {
                unique.push(declaration);
            }
        }
        Ok(unique)
    }

    /// Checks that declaration-like cursor symbols point at the canonical declaration span.
    fn declaration_occurrence_matches_canonical(
        symbol: &SourceSymbol,
        declaration: &Declaration,
    ) -> bool {
        if !matches!(symbol.symbol(), SymbolAt::Declaration { .. }) {
            return true;
        }

        symbol.target() == declaration.target()
            && symbol.file_id() == declaration.file_id()
            && symbol.span() == declaration.selection_span()
    }

    /// Returns whether the declaration kind and canonical name can safely be renamed.
    fn is_renameable_declaration(declaration: &Declaration) -> bool {
        if matches!(
            declaration.kind(),
            SymbolKind::Impl | SymbolKind::Macro | SymbolKind::Module
        ) {
            return false;
        }

        let name = declaration.name();
        !name.is_empty()
            && name != "<unsupported>"
            && !name.starts_with('#')
            && !matches!(name, "self" | "Self" | "crate" | "super")
    }

    /// Returns the visible label selected by path-like cursor symbols.
    fn selected_label(symbol: &SymbolAt) -> Option<String> {
        match symbol {
            SymbolAt::TypePath { path, .. }
            | SymbolAt::ValuePath { path, .. }
            | SymbolAt::UsePath { path, .. } => path.last_segment_label(),
            SymbolAt::RecordField { key, .. } => Some(key.declaration_label()),
            SymbolAt::FunctionBody { .. }
            | SymbolAt::Declaration { .. }
            | SymbolAt::Expr { .. } => None,
        }
    }

    /// Returns whether a requested replacement can be emitted as a Rust identifier token.
    fn is_supported_new_name(name: &str) -> bool {
        // TODO: Support non-ASCII identifiers once rename edits verify lexer token boundaries.
        let name = match name.strip_prefix("r#") {
            Some(raw) => raw,
            // Technically we are being more restrictive than needed here, but it's unlikely to affect
            // anyone realistically & it'll probably be a PITA to drag edition here, so it's fine.
            None if SyntaxKind::from_keyword(name, Edition::CURRENT).is_none() => name,
            None => return false,
        };

        let mut chars = name.chars();
        let Some(first) = chars.next() else {
            return false;
        };
        (first == '_' || first.is_ascii_alphabetic())
            && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
            && !matches!(name, "self" | "Self" | "crate" | "super")
    }
}
