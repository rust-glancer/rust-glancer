//! Selects function, const, and static bodies from editor syntax.
//!
//! This module only decides which syntax body belongs to a cursor or overlaps a requested range.
//! It also keeps parser-recovery rules for unfinished code in one place. It does not decide whether
//! the selected body still belongs to a saved declaration; the parent builder does that after
//! syntax selection is complete.

use rg_parse::{Span, enclosing_inline_module_path};
use rg_syntax::{AstNode as _, SourceFile, SyntaxNode, ast};
use rg_text::Name;

use super::CurrentSourceSelection;
use super::declaration::CurrentDeclarationBuilder;

/// A function, const, or static that has a body in the editor's syntax tree.
///
/// All values are created through [`Self::cast_with_body`], which skips declarations such as trait
/// methods and unfinished items. Later code can therefore rely on the body being present.
#[derive(Clone)]
pub(super) enum SyntaxBodyOwner {
    Function(ast::Fn),
    Const(ast::Const),
    Static(ast::Static),
}

impl SyntaxBodyOwner {
    /// Apply the requested cursor or range policy to the parsed editor text.
    pub(super) fn select(
        file: &SourceFile,
        source: &str,
        errors: &[rg_syntax::SyntaxError],
        selection: CurrentSourceSelection,
        cancellation: &rg_std::CancellationToken,
    ) -> anyhow::Result<Vec<Self>> {
        let mut owners = Vec::new();
        for (index, node) in file.syntax().descendants().enumerate() {
            if index % 64 == 0 {
                rg_std::check_cancel!(cancellation, "current body syntax selection");
            }
            if let Some(owner) = Self::cast_with_body(node) {
                owners.push(owner);
            }
        }
        Ok(match selection {
            CurrentSourceSelection::AtOffset(offset) => {
                Self::at_cursor(&owners, source, offset, errors)
                    .into_iter()
                    .collect()
            }
            CurrentSourceSelection::IntersectingRange(range) => owners
                .into_iter()
                .filter(|owner| {
                    let body = owner.body_span();
                    range.start < body.text.end && body.text.start < range.end
                })
                .collect(),
        })
    }

    /// Return the outer declaration that can provide all request-local context for this body.
    ///
    /// A nested function inside a new method needs the method's `Self` and parameter context. If
    /// the method has no saved owner, starting at the nested function would cut those facts off.
    pub(super) fn outermost_body_owner(&self) -> Self {
        self.syntax()
            .ancestors()
            .filter_map(Self::cast_with_body)
            .last()
            .unwrap_or_else(|| self.clone())
    }

    /// Describe the inline modules that contain this declaration in editor syntax.
    pub(super) fn inline_module_path(&self) -> Vec<Name> {
        enclosing_inline_module_path(self.syntax())
    }

    /// Return whether this declaration is an associated item of an impl block.
    pub(super) fn belongs_to_impl(&self) -> bool {
        CurrentDeclarationBuilder::associated_owner(self.syntax())
            .is_some_and(|owner| ast::Impl::can_cast(owner.kind()))
    }

    /// Find the declaration that owns the cursor, including its header and an unfinished body at
    /// the end of the buffer.
    ///
    /// Header selection lets a new `fn load<T>(_: T$0)` use the same request-local signature as
    /// its body. Rust parser recovery can also end a block immediately before an incomplete
    /// construct. For example, the body parsed from `let _ = User { $0` ends at `{`, while the
    /// cursor follows its trailing space. When the remaining gap is only whitespace and the parser
    /// reports an error there, the nearest declaration is still the only owner the cursor can
    /// belong to.
    fn at_cursor(
        owners: &[Self],
        source: &str,
        offset: u32,
        errors: &[rg_syntax::SyntaxError],
    ) -> Option<Self> {
        let candidates = owners
            .iter()
            .filter(|owner| Self::span(owner.syntax()).touches(offset));
        if let Some(owner) = candidates.min_by_key(|owner| Self::syntax_len(owner)) {
            return Some(owner.clone());
        }

        let offset = usize::try_from(offset).ok()?;
        if offset > source.len()
            || source[offset..]
                .chars()
                .any(|character| !character.is_whitespace())
        {
            return None;
        }

        let mut recovered = owners
            .iter()
            .filter(|owner| {
                let body_end = usize::from(owner.body_end());
                body_end <= offset
                    && source[body_end..offset].chars().all(char::is_whitespace)
                    && errors.iter().any(|error| {
                        let error_start = usize::from(error.range().start());
                        body_end <= error_start && error_start <= offset
                    })
            })
            .collect::<Vec<_>>();
        recovered.sort_by_key(|owner| std::cmp::Reverse(owner.body_start()));
        recovered.into_iter().next().cloned()
    }

    pub(super) fn cast_with_body(node: SyntaxNode) -> Option<Self> {
        // A declaration without a body is syntax that surrounds an analysis range, not a body we
        // can lower. This includes valid trait methods and half-typed items such as `fn name`.
        if ast::Fn::can_cast(node.kind()) {
            let function = ast::Fn::cast(node).expect("syntax kind should cast to a function");
            return function
                .body()
                .is_some()
                .then_some(Self::Function(function));
        }
        if ast::Const::can_cast(node.kind()) {
            let konst = ast::Const::cast(node).expect("syntax kind should cast to a const");
            return konst.body().is_some().then_some(Self::Const(konst));
        }
        if ast::Static::can_cast(node.kind()) {
            let static_ = ast::Static::cast(node).expect("syntax kind should cast to a static");
            return static_.body().is_some().then_some(Self::Static(static_));
        }
        None
    }

    fn syntax_len(&self) -> u32 {
        Self::span(self.syntax()).len()
    }

    fn body_span(&self) -> Span {
        match self {
            Self::Function(function) => function.body().map(|body| Self::span(body.syntax())),
            Self::Const(konst) => konst.body().map(|body| Self::span(body.syntax())),
            Self::Static(static_) => static_.body().map(|body| Self::span(body.syntax())),
        }
        .expect("selected syntax owner should have a body")
    }

    pub(super) fn syntax(&self) -> &SyntaxNode {
        match self {
            Self::Function(function) => function.syntax(),
            Self::Const(konst) => konst.syntax(),
            Self::Static(static_) => static_.syntax(),
        }
    }

    fn body_start(&self) -> rg_syntax::TextSize {
        match self {
            Self::Function(function) => function
                .body()
                .map(|body| body.syntax().text_range().start()),
            Self::Const(konst) => konst.body().map(|body| body.syntax().text_range().start()),
            Self::Static(static_) => static_
                .body()
                .map(|body| body.syntax().text_range().start()),
        }
        .expect("selected syntax owner should have a body")
    }

    fn body_end(&self) -> rg_syntax::TextSize {
        match self {
            Self::Function(function) => {
                function.body().map(|body| body.syntax().text_range().end())
            }
            Self::Const(konst) => konst.body().map(|body| body.syntax().text_range().end()),
            Self::Static(static_) => static_.body().map(|body| body.syntax().text_range().end()),
        }
        .expect("selected syntax owner should have a body")
    }

    fn span(node: &SyntaxNode) -> Span {
        Span::from_text_range(node.text_range())
    }
}
