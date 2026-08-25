//! Source text shared by trait-member completion and bulk implementation actions.
//!
//! Missing-member lookup supplies a substituted declaration such as `fn run(&self)` or
//! `type Output` with the suggested value `()`. This module turns that declaration into two forms:
//! a compact line for completion detail and complete plain Rust source for insertion. Completion
//! adds snippet placeholders separately; a bulk code action can insert only ordinary source text.

use rg_ir_view::trait_impl::MissingTraitMemberScaffold;

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
