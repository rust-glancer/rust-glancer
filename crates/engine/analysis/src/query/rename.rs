//! Semantic rename planning over indexed source occurrences.
//!
//! Rename builds on the same symbol identity used by goto-definition and references, but keeps a
//! stricter policy: only declaration-like names with unambiguous source occurrences become edits.

use std::collections::{HashMap, HashSet};

use anyhow::Context as _;
use rg_ir_model::{
    BodyKey, CrateRef, DefMapRef, FileId, SourceLocationKind, Span, identity::DeclarationRef,
};
use rg_ir_view::{
    display::syntax::SyntaxRenderer,
    item::declaration::{Declaration, DeclarationView},
    source::{IndexedSourceProvenance, IndexedSourceSurface, SourceOccurrenceView},
};
use rg_std::ExpectedUnique;

use super::references::{ReferenceResolver, ReferenceSearchKind};
use crate::{
    Analysis, ReferenceQuery, SymbolKind,
    model::{RenameEdit, RenameResult, RenameTarget, SymbolAt},
    source_symbol::{SourceSymbol, SourceSymbolIndex, SourceSymbolResolver, SourceSymbolRole},
};

struct RenameSubject {
    target: RenameTarget,
    identity: DeclarationRef,
    declaration: Declaration,
}

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
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<RenameTarget>> {
        Ok(self
            .rename_subject(crate_ref, file_id, offset)?
            .map(|(subject, _)| subject.target))
    }

    /// Produces all text edits needed to rename the symbol selected at `offset`.
    pub(crate) fn rename(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
        new_name: &str,
        query: ReferenceQuery<'_>,
    ) -> anyhow::Result<Option<RenameResult>> {
        // Reject invalid replacement names before doing any semantic work, so callers get a clear
        // error instead of an empty rename caused by an impossible edit.
        let new_name = Self::identifier_semantic_spelling(new_name);
        anyhow::ensure!(
            Self::is_supported_new_name(new_name),
            "rename target `{new_name}` is not a supported Rust identifier"
        );

        let Some((subject, mut arguments)) = self.rename_subject(crate_ref, file_id, offset)?
        else {
            return Ok(None);
        };
        let old_name = subject
            .declaration
            .semantic_name()
            .expect("rename subject has a name");

        // Reference resolution gives us source occurrences. Rename then handles the final surface
        // rewrite, because shorthand records may need a larger replacement than the selected span.
        let symbols = ReferenceResolver::new(self.analysis, query)
            .source_symbols_matching_declarations(
                &[subject.identity],
                ReferenceSearchKind::Rename,
            )?;
        let mut edits = Vec::new();
        for symbol in symbols {
            if self.source_symbol_uses_different_spelling(&symbol, old_name)? {
                continue;
            }
            // A rename of `acc` must also update `increment!(acc)`. If we cannot rewrite
            // that use safely, decline the whole rename so the declaration and use stay in sync.
            if !arguments.accepts(&symbol)? {
                return Ok(None);
            }
            edits.push(
                self.rename_edit_for_symbol(symbol, new_name)
                    .context("plan rename edit")?,
            );
        }

        // A semantic occurrence can be reached through more than one source scanner. Collapse exact
        // duplicates and fail loudly if two planned edits would fight over the same text.
        let edits = Self::normalize_rename_edits(edits)?;

        Ok(Some(RenameResult {
            target: subject.target,
            edits,
        }))
    }

    /// Converts one matched source occurrence into the concrete source edit for its spelling.
    #[rg_std::cancelable("rename edit collection", token = self.analysis)]
    fn rename_edit_for_symbol(
        &self,
        symbol: SourceSymbol,
        new_name: &str,
    ) -> anyhow::Result<RenameEdit> {
        let syntax =
            SyntaxRenderer::new(self.analysis.view_db().crate_edition(symbol.crate_ref())?);
        let new_identifier = syntax.identifier(new_name).to_string();
        let source_name = self.source_text_for_span(&symbol, symbol.span())?;

        // Plain references can replace the selected text directly. Shorthand record syntax carries
        // two names in one span, so those occurrences expand into explicit `field: value` spelling.
        let (span, old_text, new_text) = match symbol.surface().clone() {
            IndexedSourceSurface::FormatCapture => {
                (symbol.span(), source_name, new_name.to_owned())
            }
            IndexedSourceSurface::Plain | IndexedSourceSurface::RecordFieldKeyExplicit => {
                (symbol.span(), source_name, new_identifier)
            }
            IndexedSourceSurface::RecordExprShorthandFieldKey { .. } => (
                symbol.span(),
                source_name.clone(),
                format!("{new_identifier}: {source_name}"),
            ),
            IndexedSourceSurface::RecordExprShorthandValue { key, .. } => (
                symbol.span(),
                source_name,
                format!("{}: {new_identifier}", syntax.field_key(&key)),
            ),
            IndexedSourceSurface::RecordPatShorthandFieldKey {
                field_span,
                pat_span,
            } => {
                let old_text = self.source_text_for_span(&symbol, field_span)?;
                let pat_text = self.source_text_for_span(&symbol, pat_span)?;
                (
                    field_span,
                    old_text,
                    format!("{new_identifier}: {pat_text}"),
                )
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
                    &new_identifier,
                )
                .context("while attempting to rewrite record pattern shorthand binding")?;
                (
                    field_span,
                    old_text,
                    format!("{}: {pat_text}", syntax.field_key(&key)),
                )
            }
        };

        Ok(RenameEdit {
            crate_ref: symbol.crate_ref(),
            file_id: symbol.file_id(),
            span,
            old_text,
            new_text,
        })
    }

    /// Returns whether rename should leave this occurrence untouched because it uses an alias.
    ///
    /// A selected cursor or a declaration-backed occurrence can still refer through an alias.
    /// Rename is stricter: editing `Account` while renaming `User`, or `alias_text!()` while
    /// renaming `make_text`, would break the alias surface instead of renaming the declaration.
    fn source_symbol_uses_different_spelling(
        &self,
        symbol: &SourceSymbol,
        canonical_name: &str,
    ) -> anyhow::Result<bool> {
        let Some(spelling) = self.source_symbol_spelling(symbol)? else {
            return Ok(false);
        };
        Ok(Self::identifier_semantic_spelling(&spelling) != canonical_name)
    }

    fn source_symbol_spelling(&self, symbol: &SourceSymbol) -> anyhow::Result<Option<String>> {
        if let IndexedSourceSurface::RecordExprShorthandValue { key, .. } = symbol.surface() {
            return Ok(Some(key.declaration_label()));
        }

        match symbol.symbol() {
            SymbolAt::TypePath { type_path, .. } => Ok(type_path.path().last_segment_label()),
            SymbolAt::ValuePath { path, .. } | SymbolAt::UsePath { path, .. } => {
                Ok(path.last_segment_label())
            }
            SymbolAt::RecordField { key, .. } => Ok(Some(key.declaration_label())),
            SymbolAt::Expr { expr } => {
                SourceOccurrenceView::new(self.analysis.view_db()).expr_source_label(*expr)
            }
            SymbolAt::Declaration { .. } if symbol.role() == SourceSymbolRole::Reference => {
                self.source_text_for_span(symbol, symbol.span()).map(Some)
            }
            SymbolAt::FunctionBody { .. } | SymbolAt::Declaration { .. } => Ok(None),
        }
    }

    /// Reads the exact source text covered by a rename surface span.
    fn source_text_for_span(&self, symbol: &SourceSymbol, span: Span) -> anyhow::Result<String> {
        self.analysis
            .sources
            .text_for_span(symbol.file_id(), span)
            .with_context(|| "while attempting to read source text for rename edit")?
            .with_context(|| "rename edit span should have source text")
    }

    /// Replaces a child span inside already-loaded parent source text.
    fn replace_text_inside_span(
        parent_span: Span,
        child_span: Span,
        parent_text: &str,
        new_text: &str,
    ) -> anyhow::Result<String> {
        anyhow::ensure!(
            parent_span.start <= child_span.start && child_span.end <= parent_span.end,
            "rename child span is outside parent span"
        );

        let start = usize::try_from(child_span.start - parent_span.start)
            .context("while attempting to compute rename child span start")?;
        let end = usize::try_from(child_span.end - parent_span.start)
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
                edit.crate_ref.package.0,
                edit.crate_ref.crate_id.0,
                edit.file_id,
                edit.span.start,
                edit.span.end,
            )
        });

        let mut normalized: Vec<RenameEdit> = Vec::new();
        for edit in edits {
            let Some(previous) = normalized.last() else {
                normalized.push(edit);
                continue;
            };
            if previous.crate_ref != edit.crate_ref || previous.file_id != edit.file_id {
                normalized.push(edit);
                continue;
            }
            if previous.span == edit.span {
                if previous.old_text == edit.old_text && previous.new_text == edit.new_text {
                    continue;
                }
                anyhow::bail!("rename produced conflicting edits for the same source span");
            }
            if previous.span.start < edit.span.end && edit.span.start < previous.span.end {
                anyhow::bail!("rename produced overlapping source edits");
            }
            normalized.push(edit);
        }

        Ok(normalized)
    }

    /// Check that both the cursor and the declaration can be edited. Prepare-rename can
    /// return the checked target. Full rename can reuse the checked macro inputs during its scan.
    fn rename_subject(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<(RenameSubject, MacroArgumentEdits<'_, 'a, 'db>)>> {
        let Some(symbol) = self
            .analysis
            .source_symbol_at_for_query(crate_ref, file_id, offset)?
        else {
            return Ok(None);
        };
        // Structural candidates exist so references can highlight syntax tied to a symbol, but
        // they are not independently renameable cursor targets.
        if symbol.role() == SourceSymbolRole::Structural {
            return Ok(None);
        }

        // Rename needs one declaration identity. Ambiguous symbols are safe for navigation-style
        // features, but producing edits from them would risk changing unrelated code.
        let Some(declaration_ref) = self.unique_declaration(symbol.symbol().clone())? else {
            return Ok(None);
        };
        let Some(declaration) =
            DeclarationView::new(self.analysis.view_db()).declaration(declaration_ref)?
        else {
            return Ok(None);
        };

        // If the cursor was on a declaration occurrence, require it to be the canonical source
        // declaration. This avoids renaming through generated or alternate declaration-like spans.
        if !Self::declaration_occurrence_matches_canonical(&symbol, &declaration) {
            return Ok(None);
        }
        if declaration.exact_name_location().is_none()
            || matches!(declaration.kind(), SymbolKind::Impl | SymbolKind::Module)
        {
            return Ok(None);
        }
        let Some(name) = declaration.semantic_name().map(|name| name.as_str()) else {
            return Ok(None);
        };
        if name.is_empty()
            || matches!(name, "self" | "Self" | "crate" | "super")
            || self.source_symbol_uses_different_spelling(&symbol, name)?
        {
            return Ok(None);
        }

        let mut arguments = MacroArgumentEdits::new(self, declaration_ref);
        if !arguments.accepts(&symbol)? || !arguments.accepts_declaration(&declaration)? {
            return Ok(None);
        }

        // Prepare-rename describes the selected occurrence, which may be written under a
        // different edition than its declaration. Preserve that exact spelling instead of
        // re-rendering the semantic name with declaration-site syntax rules.
        let placeholder = self.source_text_for_span(&symbol, symbol.span())?;

        let subject = RenameSubject {
            target: RenameTarget {
                file_id: symbol.file_id(),
                span: symbol.span(),
                placeholder,
            },
            identity: declaration_ref,
            declaration,
        };
        Ok(Some((subject, arguments)))
    }

    fn unique_declaration(&self, symbol: SymbolAt) -> anyhow::Result<Option<DeclarationRef>> {
        let declarations =
            SourceSymbolResolver::new(self.analysis.view_db()).declarations_for_symbol(symbol)?;
        let mut unique = ExpectedUnique::new();
        for declaration in declarations {
            rg_std::check_cancel!(self.analysis, "rename edits");
            unique.push(declaration);
        }
        Ok(unique.into_option())
    }

    /// Checks that declaration-like cursor symbols point at the canonical declaration span.
    fn declaration_occurrence_matches_canonical(
        symbol: &SourceSymbol,
        declaration: &Declaration,
    ) -> bool {
        if symbol.role() != SourceSymbolRole::Declaration {
            return true;
        }
        if !matches!(symbol.symbol(), SymbolAt::Declaration { .. }) {
            return true;
        }

        symbol.crate_ref() == declaration.crate_ref()
            && symbol.file_id() == declaration.file_id()
            && Some(symbol.span())
                == declaration
                    .exact_name_location()
                    .map(|location| location.span)
    }

    /// Returns whether a requested replacement can be emitted as a Rust identifier token.
    fn is_supported_new_name(name: &str) -> bool {
        // TODO: Support non-ASCII identifiers once rename edits verify lexer token boundaries.
        let name = Self::identifier_semantic_spelling(name);
        let mut chars = name.chars();
        let Some(first) = chars.next() else {
            return false;
        };
        name != "_"
            && (first == '_' || first.is_ascii_alphabetic())
            && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
            && !matches!(name, "self" | "Self" | "crate" | "super")
    }

    /// Converts source spelling into the identifier identity used by semantic lookup.
    fn identifier_semantic_spelling(name: &str) -> &str {
        name.strip_prefix("r#").unwrap_or(name)
    }
}

type BodyMacroOccurrences = HashMap<(FileId, Span), Vec<SourceSymbol>>;

/// Request-local proof that a text edit can update all uses of an input token.
///
/// If `both!(run)` emits `left.run()` and `right.run()`, a reference search for the left
/// method can return just one of them. Editing `run` in the invocation changes both calls.
///
/// Read all occurrences at the written span before accepting the edit. Compare the number
/// of distinct copy IDs with lowering's total to catch uses such as `stringify!(run)` that
/// have no semantic occurrence to look up.
struct MacroArgumentEdits<'a, 'analysis, 'db> {
    resolver: &'a RenameResolver<'analysis, 'db>,
    declaration: DeclarationRef,
    owners: HashMap<BodyKey, BodyMacroOccurrences>,
    accepted: HashSet<(CrateRef, FileId, Span)>,
}

impl<'a, 'analysis, 'db> MacroArgumentEdits<'a, 'analysis, 'db> {
    fn new(resolver: &'a RenameResolver<'analysis, 'db>, declaration: DeclarationRef) -> Self {
        Self {
            resolver,
            declaration,
            owners: HashMap::new(),
            accepted: HashSet::new(),
        }
    }

    fn accepts_declaration(&mut self, declaration: &Declaration) -> anyhow::Result<bool> {
        let location = declaration
            .exact_name_location()
            .expect("rename subject has an exact name location");
        if location.kind != SourceLocationKind::MacroArgument {
            return Ok(true);
        }
        // A cursor on a direct use still needs to prove the macro input that names its declaration.
        let DefMapRef::Body(body) = self.declaration.origin() else {
            return Ok(false);
        };
        let symbol = self
            .occurrences(body.key())?
            .get(&(declaration.file_id(), location.span))
            .and_then(|symbols| {
                symbols
                    .iter()
                    .find(|symbol| symbol.role() == SourceSymbolRole::Declaration)
            })
            .cloned();
        match symbol {
            Some(symbol) => self.accepts(&symbol),
            None => Ok(false),
        }
    }

    #[rg_std::cancelable("macro argument rename", token = self.resolver.analysis)]
    fn accepts(&mut self, symbol: &SourceSymbol) -> anyhow::Result<bool> {
        let usage = match symbol.provenance() {
            IndexedSourceProvenance::Direct => return Ok(true),
            IndexedSourceProvenance::MacroArgument { usage: Some(usage) } => usage,
            IndexedSourceProvenance::MacroArgument { usage: None } => return Ok(false),
        };
        let Some(format_capture) = Self::format_capture(symbol) else {
            return Ok(false);
        };

        let location = (symbol.crate_ref(), symbol.file_id(), symbol.span());
        if self.accepted.contains(&location) {
            return Ok(true);
        }
        let resolver = self.resolver;
        let declaration = self.declaration;
        let view = SourceOccurrenceView::new(resolver.analysis.view_db());
        // A file included by two modules can give the same bytes two meanings. Check each
        // containing owner in the package's targets, starting from the physical range.
        let mut selected_seen = false;
        for (krate, _) in resolver
            .analysis
            .workspace
            .targets_for_package(usage.owner.krate.package)
        {
            for owner in view.body_owners_at(krate, symbol.file_id(), symbol.span())? {
                let Some(occurrences) = self
                    .occurrences(owner)?
                    .get(&(symbol.file_id(), symbol.span()))
                else {
                    return Ok(false);
                };
                // Several source occurrences can describe one output token. Check every
                // meaning and credit each copy only once toward the total.
                let mut copies = HashSet::new();
                let mut total = ExpectedUnique::new();
                for occurrence in occurrences {
                    rg_std::check_cancel!(resolver.analysis, "macro argument rename");
                    let IndexedSourceProvenance::MacroArgument { usage: Some(other) } =
                        occurrence.provenance()
                    else {
                        return Ok(false);
                    };
                    if other.owner != owner
                        || Self::format_capture(occurrence) != Some(format_capture)
                        || resolver.unique_declaration(occurrence.symbol().clone())?
                            != Some(declaration)
                    {
                        return Ok(false);
                    }
                    copies.insert(other.copy);
                    total.push(other.total_copies);
                }
                if total.into_option().map(|total| total as usize) != Some(copies.len()) {
                    return Ok(false);
                }
                // Coverage in other owners is insufficient if the occurrence that prompted
                // this edit is missing from its own owner's scan.
                selected_seen |= owner == usage.owner && copies.contains(&usage.copy);
            }
        }
        if selected_seen {
            self.accepted.insert(location);
        }
        Ok(selected_seen)
    }

    fn occurrences(&mut self, owner: BodyKey) -> anyhow::Result<&BodyMacroOccurrences> {
        // A rename can check several arguments in the same body. Group the unfiltered
        // occurrences by written range so all those checks share one scan.
        let occurrences = match self.owners.entry(owner) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                let index = SourceSymbolIndex::new(self.resolver.analysis.view_db());
                let mut by_span = BodyMacroOccurrences::new();
                for occurrence in index.symbols_in_body(owner)? {
                    rg_std::check_cancel!(self.resolver.analysis, "macro argument rename");
                    if matches!(
                        occurrence.provenance(),
                        IndexedSourceProvenance::MacroArgument { .. }
                    ) {
                        by_span
                            .entry((occurrence.file_id(), occurrence.span()))
                            .or_default()
                            .push(occurrence);
                    }
                }
                entry.insert(by_span)
            }
        };

        Ok(occurrences)
    }

    /// Shorthand rewrites can change several tokens, so they need a separate proof.
    fn format_capture(symbol: &SourceSymbol) -> Option<bool> {
        match symbol.surface() {
            IndexedSourceSurface::Plain | IndexedSourceSurface::RecordFieldKeyExplicit => {
                Some(false)
            }
            IndexedSourceSurface::FormatCapture => Some(true),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::RenameResolver;

    #[test]
    fn validates_supported_new_names() {
        let cases = [
            ("value", true, "ordinary identifier"),
            ("_value", true, "leading underscore identifier"),
            ("r#type", true, "raw keyword identifier"),
            ("_", false, "wildcard is not a binding name"),
            ("r#_", false, "raw wildcard is not a binding name"),
            ("type", true, "presentation adds raw syntax for keywords"),
            ("self", false, "special path segment is not renameable"),
            ("", false, "empty replacement is not an identifier"),
        ];

        for (input, expected, message) in cases {
            assert_eq!(
                RenameResolver::<'_, '_>::is_supported_new_name(input),
                expected,
                "{message}"
            );
        }
    }
}
