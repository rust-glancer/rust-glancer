//! Trait-member lookup and source text shared by completion and bulk implementation actions.
//!
//! Both features first select the impl from the editor buffer. An unchanged impl can use its saved
//! identity directly. A new or changed impl gets a request-local semantic header, resolved in the
//! saved module that still contains it. Once missing-member lookup supplies a substituted
//! declaration such as `fn run(&self)` or `type Output`, this module also turns it into a compact
//! completion detail and complete plain Rust source for insertion.

use anyhow::Context as _;
use rg_ir_model::CrateRef;
use rg_ir_view::{
    current::CurrentTraitImplView,
    source::SourceCompletionView,
    trait_impl::{MissingTraitMember, MissingTraitMemberScaffold, TraitImplView},
};
use rg_parse::{FileId, LineIndex, enclosing_inline_module_path};
use rg_syntax::{AstNode as _, ast};

use crate::Analysis;

/// Resolves missing members for the trait impl currently shown by one analysis request.
///
/// Saved lookup remains the cheap path. Only an impl without a usable saved identity pays for the
/// small source-to-semantic lowering step, and the resulting store lives only for this call.
pub(crate) struct TraitImplMemberQuery<'analysis, 'db, 'source> {
    analysis: &'analysis Analysis<'db>,
    crate_ref: CrateRef,
    file_id: FileId,
    source_text: &'source str,
}

impl<'analysis, 'db, 'source> TraitImplMemberQuery<'analysis, 'db, 'source> {
    pub(crate) fn new(
        analysis: &'analysis Analysis<'db>,
        crate_ref: CrateRef,
        file_id: FileId,
        source_text: &'source str,
    ) -> Self {
        Self {
            analysis,
            crate_ref,
            file_id,
            source_text,
        }
    }

    /// Find the current impl beginning at `owner_start`, then resolve its missing members.
    ///
    /// Completion retains only stable offsets from its speculative parse. Looking the impl up in
    /// the ordinary request parse here means semantic lowering never sees the temporary completion
    /// marker used to recover an unfinished member.
    pub(crate) fn missing_members_at(
        &self,
        owner_start: u32,
    ) -> anyhow::Result<Vec<MissingTraitMember>> {
        let edition = self
            .analysis
            .view_db()
            .crate_edition(self.crate_ref)
            .context("read current trait impl edition")?;
        let file = self
            .analysis
            .current_source(self.crate_ref.package, self.file_id)
            .filter(|source| source.text() == self.source_text)
            .and_then(|source| source.parse(edition))
            .map_or_else(
                || rg_parse::parse_source_file(self.source_text, edition).tree(),
                |parse| parse.tree(),
            );
        let Some(impl_) = file
            .syntax()
            .descendants()
            .filter_map(ast::Impl::cast)
            .find(|impl_| u32::from(impl_.syntax().text_range().start()) == owner_start)
        else {
            return Ok(Vec::new());
        };

        self.missing_members(&impl_)
    }

    /// Use saved semantics when the impl still has a saved identity, otherwise lower this header.
    pub(crate) fn missing_members(
        &self,
        impl_: &ast::Impl,
    ) -> anyhow::Result<Vec<MissingTraitMember>> {
        if impl_.trait_().is_none() {
            return Ok(Vec::new());
        }
        let owner_start = u32::from(impl_.syntax().text_range().start());

        // An associated saved declaration already has a complete item store and resolved header.
        // Keep that path on retained semantics instead of building a request-local item store.
        if let Some(saved_owner_start) = self
            .analysis
            .saved_header_offset_for_current(self.crate_ref, self.file_id, owner_start)
            .context("map current trait impl header to saved source")?
            && let Some(site) = SourceCompletionView::new(self.analysis.view_db())
                .trait_impl_site_at(self.crate_ref, self.file_id, saved_owner_start)
                .context("resolve saved trait impl owner")?
        {
            return TraitImplView::new(self.analysis.view_db())
                .missing_members(site.impl_ref(), site.trait_ref())
                .context("collect saved trait impl members");
        }

        // A new impl has no semantic owner, but its enclosing module normally still does. Lower
        // only this impl and resolve its header as though it were declared in that saved module.
        let inline_module_path = enclosing_inline_module_path(impl_.syntax())
            .into_iter()
            .map(|name| name.to_string())
            .collect::<Vec<_>>();
        let Some(module) = SourceCompletionView::new(self.analysis.view_db())
            .module_syntax_source_site(self.crate_ref, self.file_id, &inline_module_path)
            .context("resolve current trait impl module")?
            .map(|site| site.module())
        else {
            return Ok(Vec::new());
        };

        let fallback_line_index;
        let line_index = match self
            .analysis
            .current_source(self.crate_ref.package, self.file_id)
            .filter(|source| source.text() == self.source_text)
        {
            Some(source) => source.line_index(),
            None => {
                fallback_line_index = LineIndex::new(self.source_text);
                &fallback_line_index
            }
        };
        let Some(current) = CurrentTraitImplView::new(
            self.analysis.view_db(),
            self.crate_ref,
            self.file_id,
            module,
            line_index,
            impl_,
        )
        .context("resolve current trait impl")?
        else {
            return Ok(Vec::new());
        };
        current
            .missing_members()
            .context("collect current trait impl members")
    }
}

/// The two text forms shared by completion and the bulk implementation action.
pub(crate) struct RenderedTraitMember {
    /// Compact declaration text used in completion detail.
    pub(crate) signature: String,
    /// Complete, non-snippet source suitable for direct insertion.
    pub(crate) plain: String,
}

impl RenderedTraitMember {
    /// Add the body, value, or semicolon that turns one declaration scaffold into insertable Rust.
    pub(crate) fn new(scaffold: &MissingTraitMemberScaffold) -> Self {
        match scaffold {
            MissingTraitMemberScaffold::Function { signature } => Self {
                signature: signature.clone(),
                plain: format!("{signature} {{\n    todo!()\n}}"),
            },
            MissingTraitMemberScaffold::TypeAlias {
                signature_prefix,
                suggested_value,
            } => Self {
                signature: format!("{signature_prefix} = {suggested_value}"),
                plain: format!("{signature_prefix} = {suggested_value};"),
            },
            MissingTraitMemberScaffold::Const { signature } => Self {
                signature: signature.clone(),
                plain: format!("{signature} = todo!();"),
            },
        }
    }
}
