//! Trait-member lookup and source text shared by completion and bulk implementation actions.
//!
//! Both features select the impl from the editor buffer and read the declaration context prepared
//! for it. The missing-member projection supplies substituted declarations such as `fn run(&self)`
//! or `type Output`; this module turns those into completion details and insertable Rust source.

use anyhow::Context as _;
use rg_ir_model::{CrateRef, FileId, Span};
use rg_ir_view::{
    source::SourceCompletionView,
    trait_impl::{MissingTraitMember, MissingTraitMemberScaffold, TraitImplView},
};
use rg_syntax::{AstNode as _, ast};

use crate::Analysis;

/// Resolves missing members for the trait impl currently shown by one analysis request.
///
/// Saved-only queries use indexed source sites. Current-source queries use the impl identity chosen
/// during preparation, including its decision to reuse saved semantics where possible.
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

    /// Read the complete impl selected for this source, without constructing semantic state here.
    pub(crate) fn missing_members(
        &self,
        impl_: &ast::Impl,
    ) -> anyhow::Result<Vec<MissingTraitMember>> {
        if impl_.trait_().is_none() {
            return Ok(Vec::new());
        }
        let db = self.analysis.view_db();
        let span = Span::from_text_range(impl_.syntax().text_range());
        if let Some(source) = self
            .analysis
            .current_source(self.crate_ref.package, self.file_id)
        {
            // Source coordinates and selected declarations belong to the same captured document.
            if source.text() != self.source_text {
                return Ok(Vec::new());
            }
            let Some(impl_ref) = db.selected_current_impl(self.crate_ref, self.file_id, span)
            else {
                return Ok(Vec::new());
            };
            return TraitImplView::new(db)
                .missing_members_for_prepared_impl(impl_ref)
                .context("collect prepared trait impl members");
        }

        // An exact saved document needs no current-source preparation. Its indexed source site
        // already carries the resolved trait identity used by the ordinary member projection.
        let owner_start = u32::from(impl_.syntax().text_range().start());
        let Some(site) = SourceCompletionView::new(db)
            .trait_impl_site_at(self.crate_ref, self.file_id, owner_start)
            .context("resolve saved trait impl owner")?
        else {
            return Ok(Vec::new());
        };
        TraitImplView::new(db)
            .missing_members(site.impl_ref(), site.trait_ref())
            .context("collect saved trait impl members")
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
