//! Bulk implementation of required members for one resolved trait impl.
//!
//! Given `trait Service { fn run(&self); }` and `impl Service for Worker {}`, the action inserts a
//! plain `run` scaffold with a `todo!()` body. Saved semantics provide substituted member
//! signatures, while the current syntax tree subtracts members typed since the last save. This
//! keeps the generated block useful without duplicating dirty editor text.
//!
//! Nominal types in those signatures are rendered by their declared short name. This provider
//! does not add imports for them: if a generated method mentions `Request`, that name must already
//! be visible from the impl module.

use anyhow::Context as _;
use rg_ir_view::{
    source::SourceCompletionView,
    trait_impl::{MissingTraitMember, MissingTraitMemberRef, TraitImplView},
};
use rg_parse::{Span, TextSpan};
use rg_syntax::{
    AstNode as _,
    ast::{self, HasName as _},
};

use crate::{
    Analysis, CodeAction, CodeActionEdit, CodeActionKind, CodeActionQuery,
    query::trait_member::RenderedTraitMember,
};

use super::syntax::CodeActionSyntax;

/// The part of a current-source impl member needed to compare it with saved trait information.
///
/// Kind is part of the key so temporarily typing `const run` cannot suppress a required `fn run`
/// merely because the dirty source contains the same label.
#[derive(Debug, Clone, PartialEq, Eq)]
enum CurrentMemberKey {
    Function(String),
    TypeAlias(String),
    Const(String),
}

impl CurrentMemberKey {
    /// Reduce one parsed impl item to the kind and name used by the missing-member list.
    fn from_assoc_item(item: ast::AssocItem) -> Option<Self> {
        match item {
            ast::AssocItem::Fn(function) => {
                Some(Self::Function(Self::semantic_name(function.name()?.text())))
            }
            ast::AssocItem::TypeAlias(alias) => {
                Some(Self::TypeAlias(Self::semantic_name(alias.name()?.text())))
            }
            ast::AssocItem::Const(konst) => {
                Some(Self::Const(Self::semantic_name(konst.name()?.text())))
            }
            ast::AssocItem::MacroCall(_) => None,
        }
    }

    /// Remove the source-only `r#` prefix so `r#type` compares as the semantic name `type`.
    fn semantic_name(name: impl AsRef<str>) -> String {
        let name = name.as_ref();
        name.strip_prefix("r#").unwrap_or(name).to_string()
    }

    /// Check whether this current-source item already implements the saved missing member.
    fn matches(&self, member: &MissingTraitMember) -> bool {
        match (self, member.member()) {
            (Self::Function(current), MissingTraitMemberRef::Function(_))
            | (Self::TypeAlias(current), MissingTraitMemberRef::TypeAlias(_))
            | (Self::Const(current), MissingTraitMemberRef::Const(_)) => current == member.label(),
            _ => false,
        }
    }
}

/// Builds one edit containing every required member still absent from the current impl body.
pub(super) struct TraitImplCodeActionProvider<'analysis, 'db, 'source> {
    analysis: &'analysis Analysis<'db>,
    query: CodeActionQuery<'source>,
}

impl<'analysis, 'db, 'source> TraitImplCodeActionProvider<'analysis, 'db, 'source> {
    pub(super) fn new(analysis: &'analysis Analysis<'db>, query: CodeActionQuery<'source>) -> Self {
        Self { analysis, query }
    }

    /// Insert every required trait member not already present in the editor buffer.
    ///
    /// The indexed project knows the trait and its substituted signatures, but it may lag behind
    /// unsaved typing. This method therefore uses saved information to discover requirements and
    /// the parsed editor buffer to avoid duplicating members the user just added.
    pub(super) fn code_action(
        &self,
        syntax: &CodeActionSyntax<'_>,
    ) -> anyhow::Result<Option<CodeAction>> {
        // 1. Select a trait impl with a complete body from the exact text shown by the editor.
        let Some(impl_) = syntax.node_at_start::<ast::Impl>() else {
            return Ok(None);
        };
        if !syntax.request_starts_on(&impl_) || impl_.trait_().is_none() {
            return Ok(None);
        }
        let Some(item_list) = impl_.assoc_item_list() else {
            return Ok(None);
        };
        if item_list.r_curly_token().is_none() {
            return Ok(None);
        }

        // 2. Pair the current impl header to its saved semantic declaration. If editing changed
        // the header identity, declining the action is safer than borrowing another impl's trait.
        let current_owner_start = u32::from(impl_.syntax().text_range().start());
        let Some(saved_owner_start) = self
            .analysis
            .saved_header_offset_for_current(
                self.query.crate_ref,
                self.query.file_id,
                current_owner_start,
            )
            .context("map current trait impl header to saved source")?
        else {
            return Ok(None);
        };
        let Some(site) = SourceCompletionView::new(self.analysis.view_db())
            .trait_impl_site_at(self.query.crate_ref, self.query.file_id, saved_owner_start)
            .context("resolve trait impl action owner")?
        else {
            return Ok(None);
        };

        // 3. Saved semantics suppress saved members. Repeat the same `(kind, name)` comparison over
        // current syntax so a newly typed member cannot be inserted twice before the next save.
        let current_members = item_list
            .assoc_items()
            .filter_map(CurrentMemberKey::from_assoc_item)
            .collect::<Vec<_>>();
        let members = TraitImplView::new(self.analysis.view_db())
            .missing_members(site.impl_ref(), site.trait_ref())
            .context("collect required trait members for code action")?
            .into_iter()
            .filter(|member| member.is_required())
            .filter(|member| {
                !current_members
                    .iter()
                    .any(|current| current.matches(member))
            })
            .collect::<Vec<_>>();
        if members.is_empty() {
            return Ok(None);
        }

        // TODO: If the trait signature refers to `protocol::Request`, the shared renderer inserts
        // only `Request`. Before this action can always produce compiling source, either render a
        // path visible from the impl module or add the required imports.
        //
        // 4. Render the remaining members together and replace only whitespace before the closing
        // brace. Everything else in the possibly-unsaved impl stays untouched.
        let edit = Self::member_insertion(syntax.source(), &impl_, &item_list, &members)
            .context("build required trait member insertion")?;
        Ok(Some(CodeAction {
            title: "Implement missing trait members".to_string(),
            kind: CodeActionKind::QuickFix,
            is_preferred: true,
            edits: vec![edit],
        }))
    }

    /// Replace only trailing whitespace before `}` so comments and incomplete syntax survive.
    ///
    /// Existing member indentation wins when available. An empty impl gets one leading newline;
    /// an impl with inner content gets a blank line before the generated block.
    fn member_insertion(
        source: &str,
        impl_: &ast::Impl,
        item_list: &ast::AssocItemList,
        members: &[MissingTraitMember],
    ) -> Option<CodeActionEdit> {
        // Find the whitespace immediately before `}`. This is the only existing text replaced by
        // the action, so comments and incomplete members earlier in the body are preserved.
        let left_curly_end = usize::from(item_list.l_curly_token()?.text_range().end());
        let right_curly_start = usize::from(item_list.r_curly_token()?.text_range().start());
        let trailing_start =
            Self::trailing_whitespace_start(source, left_curly_end, right_curly_start)?;
        let closing_indent =
            Self::line_indent(source, usize::from(impl_.syntax().text_range().start()))
                .unwrap_or_default();
        // Follow an existing member's indentation when possible. For an empty impl, indent one
        // level beyond the line containing `impl`.
        let member_indent = item_list
            .assoc_items()
            .next()
            .and_then(|item| {
                Self::line_indent(source, usize::from(item.syntax().text_range().start()))
            })
            .filter(|indent| indent.len() > closing_indent.len())
            .unwrap_or_else(|| format!("{closing_indent}    "));
        // The shared renderer produces unindented member text. Indent every line here, then keep a
        // blank line between generated members so functions, types, and consts form one block.
        let block = members
            .iter()
            .map(|member| {
                let rendered = RenderedTraitMember::new(member.scaffold());
                rendered
                    .plain
                    .lines()
                    .map(|line| format!("{member_indent}{line}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        let has_inner_content = !source
            .get(left_curly_end..trailing_start)?
            .trim()
            .is_empty();
        let separator = if has_inner_content { "\n\n" } else { "\n" };
        let new_text = format!("{separator}{block}\n{closing_indent}");
        Some(CodeActionEdit {
            replace: Span {
                text: TextSpan {
                    start: u32::try_from(trailing_start).ok()?,
                    end: u32::try_from(right_curly_start).ok()?,
                },
            },
            new_text,
        })
    }

    /// Walk backward from `}` to leave every non-whitespace byte in the impl body untouched.
    fn trailing_whitespace_start(source: &str, lower: usize, end: usize) -> Option<usize> {
        let text = source.get(lower..end)?;
        let mut start = end;
        for (relative, character) in text.char_indices().rev() {
            if !character.is_whitespace() {
                break;
            }
            start = lower + relative;
        }
        Some(start)
    }

    /// Return the whitespace before an item only when it is the first token on its line.
    fn line_indent(source: &str, offset: usize) -> Option<String> {
        let before = source.get(..offset)?;
        let line_start = before.rfind('\n').map_or(0, |newline| newline + 1);
        let indent = source.get(line_start..offset)?;
        indent
            .chars()
            .all(char::is_whitespace)
            .then(|| indent.to_string())
    }
}
