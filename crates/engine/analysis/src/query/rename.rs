//! Semantic rename planning over indexed source occurrences.
//!
//! Rename builds on the same symbol identity used by goto-definition and references, but keeps a
//! stricter policy: only declaration-like names with unambiguous source occurrences become edits.

use rg_ir_model::identity::DeclarationRef;
use rg_ir_view::item::declaration::{Declaration, DeclarationView};
use rg_syntax::{Edition, SyntaxKind};

use crate::{
    Analysis, ReferenceQuery, SymbolKind,
    model::{RenameEdit, RenameResult, RenameTarget, SymbolAt},
    source_symbol::{SourceSymbol, SourceSymbolResolver, SourceSymbolRole},
};

pub(crate) struct RenameResolver<'a, 'db> {
    analysis: &'a Analysis<'db>,
}

impl<'a, 'db> RenameResolver<'a, 'db> {
    pub(crate) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self { analysis }
    }

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

    pub(crate) fn rename(
        &self,
        target: rg_ir_model::TargetRef,
        file_id: rg_parse::FileId,
        offset: u32,
        new_name: &str,
        query: ReferenceQuery<'_>,
    ) -> anyhow::Result<Option<RenameResult>> {
        anyhow::ensure!(
            Self::is_supported_new_name(new_name),
            "rename target `{new_name}` is not a supported Rust identifier"
        );

        let Some(rename_target) = self.prepare_rename(target, file_id, offset)? else {
            return Ok(None);
        };
        let references = self
            .analysis
            .references(target, file_id, offset, query)?
            .into_iter()
            .map(|reference| RenameEdit {
                target: reference.target,
                file_id: reference.file_id,
                span: reference.span,
                old_text: rename_target.placeholder.clone(),
                new_text: new_name.to_string(),
            })
            .collect();

        Ok(Some(RenameResult {
            target: rename_target,
            edits: references,
        }))
    }

    fn rename_target_for_symbol(
        &self,
        symbol: SourceSymbol,
    ) -> anyhow::Result<Option<RenameTarget>> {
        if symbol.role() == SourceSymbolRole::Structural {
            return Ok(None);
        }

        let declarations = self.unique_declarations_for_symbol(symbol.symbol().clone())?;
        let [declaration_ref] = &declarations[..] else {
            return Ok(None);
        };
        let Some(declaration) =
            DeclarationView::new(self.analysis.view_db()).declaration(*declaration_ref)?
        else {
            return Ok(None);
        };
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

    fn selected_label(symbol: &SymbolAt) -> Option<String> {
        match symbol {
            SymbolAt::TypePath { path, .. }
            | SymbolAt::ValuePath { path, .. }
            | SymbolAt::UsePath { path, .. } => path.last_segment_label(),
            SymbolAt::FunctionBody { .. }
            | SymbolAt::Declaration { .. }
            | SymbolAt::Expr { .. } => None,
        }
    }

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
