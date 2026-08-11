//! Lifetime and loop-label syntax classification.

use rg_std::UniqueVec;
use rg_syntax::{AstNode as _, SyntaxKind, ast};

use crate::query::completion::site::{
    LabelCompletionContext, LifetimeCompletionContext, SpecializedCompletionContext,
};

use super::CompletionSyntaxContext;

impl CompletionSyntaxContext<'_> {
    /// Distinguish loop labels from lifetime uses and declarations before scope lookup.
    ///
    /// Higher-ranked `for<'a>` binders live only in request syntax, so their lifetimes are carried
    /// in the returned context instead of being reconstructed by the semantic resolver.
    pub(super) fn apostrophe_completion_context(&self) -> Option<SpecializedCompletionContext> {
        if self.marker.kind() != SyntaxKind::LIFETIME_IDENT
            || !self.marker.text().contains(Self::MARKER)
        {
            return None;
        }
        let ancestors = self.marker.parent()?.ancestors().collect::<Vec<_>>();
        if ancestors.iter().any(|node| {
            ast::BreakExpr::can_cast(node.kind()) || ast::ContinueExpr::can_cast(node.kind())
        }) {
            return Some(SpecializedCompletionContext::Label(
                LabelCompletionContext::new(false),
            ));
        }
        if ancestors
            .iter()
            .any(|node| ast::Label::can_cast(node.kind()))
        {
            return Some(SpecializedCompletionContext::Label(
                LabelCompletionContext::new(true),
            ));
        }

        let declaration = ancestors
            .iter()
            .any(|node| ast::LifetimeParam::can_cast(node.kind()));
        let mut binder_lifetimes = UniqueVec::new();
        for binder in ancestors
            .iter()
            .filter_map(|node| ast::ForBinder::cast(node.clone()))
        {
            for param in binder
                .generic_param_list()
                .into_iter()
                .flat_map(|params| params.generic_params())
            {
                let ast::GenericParam::LifetimeParam(param) = param else {
                    continue;
                };
                if let Some(lifetime) = param.lifetime() {
                    let label = lifetime.syntax().text().to_string();
                    if !label.contains(Self::MARKER) {
                        binder_lifetimes.push(label);
                    }
                }
            }
        }
        Some(SpecializedCompletionContext::Lifetime(
            LifetimeCompletionContext::new(declaration, binder_lifetimes.into_vec()),
        ))
    }
}
