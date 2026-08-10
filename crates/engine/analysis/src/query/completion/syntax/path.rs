//! Path-shaped completion classifiers shared by specialized and ordinary routing.

use rg_ir_model::Path;
use rg_syntax::{AstNode as _, ast};

use crate::query::completion::site::{
    ConstExpressionCompletionContext, EmptyPathCompletionContext, NameCompletionContext,
    PatternCompletionKind, QualifiedPathCompletionSyntax, RestrictedVisibilityCompletionContext,
};

use super::CompletionSyntaxContext;

impl CompletionSyntaxContext<'_> {
    /// Return the owner of a record whose first field has not been typed yet.
    ///
    /// The saved parser may not lower `User { $0` as a record at all. The speculative marker makes
    /// that intent unambiguous, so completion carries only the owner spelling into semantic scope
    /// lookup and keeps the replacement span empty.
    pub(crate) fn empty_record_owner(&self) -> Option<Path> {
        if !self.prefix.is_empty() {
            return None;
        }

        Self::empty_record_owner_at(&self.marker)
            .or_else(|| Self::empty_record_owner_at(&self.marker_with_suffix(" }")?))
    }

    fn empty_record_owner_at(marker: &rg_syntax::SyntaxToken) -> Option<Path> {
        let ancestors = marker.parent()?.ancestors().collect::<Vec<_>>();
        let path = if let Some(field) = ancestors
            .iter()
            .find_map(|node| ast::RecordExprField::cast(node.clone()))
        {
            if field.colon_token().is_some()
                || field.syntax().text().to_string().trim() != Self::MARKER
            {
                return None;
            }
            ancestors
                .iter()
                .find_map(|node| ast::RecordExpr::cast(node.clone()))?
                .path()?
        } else {
            let field = ancestors
                .iter()
                .find_map(|node| ast::RecordPatField::cast(node.clone()))?;
            if field.colon_token().is_some()
                || field.syntax().text().to_string().trim() != Self::MARKER
            {
                return None;
            }
            ancestors
                .iter()
                .find_map(|node| ast::RecordPat::cast(node.clone()))?
                .path()?
        };

        // DefMap lookup needs only the named owner path. Generic arguments affect the constructed
        // value's type but not which declaration owns its record fields.
        let absolute = path
            .first_segment()
            .is_some_and(|segment| segment.coloncolon_token().is_some());
        let mut names = Vec::new();
        Self::collect_ast_path_names(&path, &mut names)?;
        let mut path_text = names.join("::");
        if absolute {
            path_text.insert_str(0, "::");
        }
        Path::from_macro_path_text(&path_text, None)
    }

    fn collect_ast_path_names(path: &ast::Path, names: &mut Vec<String>) -> Option<()> {
        if let Some(qualifier) = path.qualifier() {
            Self::collect_ast_path_names(&qualifier, names)?;
        }
        names.push(path.segment()?.name_ref()?.text().to_string());
        Some(())
    }

    /// Retain the complete qualifier when a trailing `::` has no saved final segment.
    pub(crate) fn empty_qualified_path(&self) -> Option<QualifiedPathCompletionSyntax> {
        if !self.prefix.is_empty() || self.inside_use_item() || !self.after_colon_colon() {
            return None;
        }

        let path = self
            .marker
            .parent()?
            .ancestors()
            .find_map(ast::Path::cast)?;
        let qualifier = path.qualifier()?;
        let ancestors = path.syntax().ancestors().collect::<Vec<_>>();
        let context = if ancestors
            .iter()
            .any(|node| ast::ConstArg::can_cast(node.kind()))
        {
            NameCompletionContext::Const
        } else if ancestors
            .iter()
            .any(|node| ast::Type::can_cast(node.kind()))
        {
            NameCompletionContext::Type
        } else if ancestors
            .iter()
            .any(|node| ast::RecordPat::can_cast(node.kind()))
        {
            NameCompletionContext::Pattern(PatternCompletionKind::RecordConstructor)
        } else if ancestors
            .iter()
            .any(|node| ast::TupleStructPat::can_cast(node.kind()))
        {
            NameCompletionContext::Pattern(PatternCompletionKind::TupleConstructor)
        } else if ancestors.iter().any(|node| ast::Pat::can_cast(node.kind())) {
            NameCompletionContext::Pattern(PatternCompletionKind::Name)
        } else if ancestors
            .iter()
            .any(|node| ast::RecordExpr::can_cast(node.kind()))
        {
            NameCompletionContext::Type
        } else {
            NameCompletionContext::Value
        };
        Some(QualifiedPathCompletionSyntax::new(
            qualifier.syntax().text().to_string(),
            context,
        ))
    }

    /// Start of the declaration whose Body IR scope should own speculative syntax.
    pub(crate) fn body_owner_start(&self) -> Option<u32> {
        self.marker.parent()?.ancestors().find_map(|node| {
            (ast::Fn::can_cast(node.kind())
                || ast::Const::can_cast(node.kind())
                || ast::Static::can_cast(node.kind()))
            .then(|| u32::from(node.text_range().start()))
        })
    }

    /// Recover a partial `pub(...)` path and whether the `in` keyword is still valid.
    pub(super) fn visibility_completion_context(
        &self,
    ) -> Option<RestrictedVisibilityCompletionContext> {
        let visibility = self
            .marker
            .parent()?
            .ancestors()
            .find_map(ast::Visibility::cast)?;
        let inner = visibility.visibility_inner();
        let qualifier = inner
            .as_ref()
            .and_then(ast::VisibilityInner::path)
            .and_then(|path| Self::marker_path_qualifier(&path.syntax().text().to_string()))
            .flatten();
        let allows_in_keyword = inner
            .as_ref()
            .is_some_and(|inner| inner.in_token().is_none());
        Some(RestrictedVisibilityCompletionContext::new(
            qualifier,
            allows_in_keyword,
        ))
    }

    /// Recognize a const argument and retain any qualifier written before its final segment.
    pub(super) fn const_expression_completion_context(
        &self,
    ) -> Option<ConstExpressionCompletionContext> {
        if !self.accepts_completion_site() {
            return None;
        }
        let ancestors = self.marker.parent()?.ancestors().collect::<Vec<_>>();
        if !ancestors
            .iter()
            .any(|node| ast::ConstArg::can_cast(node.kind()))
        {
            return None;
        }
        let qualifier = ancestors
            .iter()
            .find_map(|node| ast::Path::cast(node.clone()))
            .and_then(|path| Self::marker_path_qualifier(&path.syntax().text().to_string()))
            .flatten();
        Some(ConstExpressionCompletionContext::new(qualifier))
    }

    /// Return the path before the marker while distinguishing an unqualified marker from failure.
    pub(super) fn marker_path_qualifier(path_text: &str) -> Option<Option<Path>> {
        let path = Path::from_macro_path_text(path_text, None)?;
        if path.single_name() == Some(Self::MARKER) {
            return Some(None);
        }
        let (qualifier, name) = path.split_prefix_name()?;
        (name == Self::MARKER).then_some(Some(qualifier))
    }

    /// Extract the path-shaped token run around a marker embedded in larger syntax text.
    pub(super) fn path_around_marker(text: &str) -> Option<Option<Path>> {
        let marker = text.find(Self::MARKER)?;
        let start = text[..marker]
            .char_indices()
            .rev()
            .find(|(_, ch)| !Self::is_identifier_continue(*ch) && *ch != ':')
            .map(|(index, ch)| index + ch.len_utf8())
            .unwrap_or(0);
        let end = marker + Self::MARKER.len();
        Self::marker_path_qualifier(text.get(start..end)?)
    }

    /// Classify only named empty paths; indexed source scans still own semantic scope facts.
    pub(super) fn empty_path_completion_context(&self) -> Option<EmptyPathCompletionContext> {
        if !self.prefix.is_empty() {
            return None;
        }
        if self.inside_use_item() {
            return Some(EmptyPathCompletionContext::Import);
        }

        let ancestors = self.marker.parent()?.ancestors().collect::<Vec<_>>();
        if ancestors
            .iter()
            .any(|node| ast::GenericArgList::can_cast(node.kind()))
        {
            return Some(EmptyPathCompletionContext::GenericArgument);
        }
        if ancestors
            .iter()
            .any(|node| ast::Type::can_cast(node.kind()))
        {
            return Some(EmptyPathCompletionContext::Type);
        }
        if ancestors
            .iter()
            .any(|node| ast::ArgList::can_cast(node.kind()))
        {
            return Some(EmptyPathCompletionContext::Argument);
        }
        ancestors
            .iter()
            .any(|node| ast::StmtList::can_cast(node.kind()))
            .then_some(EmptyPathCompletionContext::Expression)
    }
}
