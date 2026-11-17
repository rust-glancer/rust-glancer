//! Trait-member lookup and source text shared by completion and bulk implementation actions.
//!
//! Both features select the impl from the editor buffer and read the declaration context prepared
//! for it. The missing-member projection supplies substituted declarations such as `fn run(&self)`
//! or `type Output`; this module turns those into completion details and insertable Rust source.
//! The details preview the code we will insert, so they retain its path qualification even when
//! an ordinary hover or inlay label would use a shorter name.

use anyhow::Context as _;
use rg_ir_model::{CrateRef, FileId};
use rg_ir_view::{
    source::SourceCompletionView,
    trait_impl::{MissingTraitMember, MissingTraitMemberScaffold, TraitImplView},
};
use rg_syntax::{AstNode as _, ast};

use crate::Analysis;

/// Resolve missing trait members and render signatures suitable for insertion into the impl.
pub(crate) struct TraitImplMemberQuery<'analysis, 'db> {
    analysis: &'analysis Analysis<'db>,
    crate_ref: CrateRef,
    file_id: FileId,
}

impl<'analysis, 'db> TraitImplMemberQuery<'analysis, 'db> {
    pub(crate) fn new(
        analysis: &'analysis Analysis<'db>,
        crate_ref: CrateRef,
        file_id: FileId,
    ) -> Self {
        Self {
            analysis,
            crate_ref,
            file_id,
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
        let file = self.analysis.source_syntax(self.crate_ref, self.file_id)?;
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
        let owner_start = u32::from(impl_.syntax().text_range().start());
        let Some(site) = SourceCompletionView::new(db)
            .trait_impl_site_at(self.crate_ref, self.file_id, owner_start)
            .context("resolve trait impl owner")?
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
    /// Declaration text for completion detail, with the same paths as the member we insert.
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
