//! String-owned and macro-token completion classifiers.

use rg_parse::{Span, TextSpan};
use rg_syntax::{
    AstNode as _, AstToken as _, SyntaxKind,
    ast::{self, IsString as _},
};

use crate::query::completion::site::SpecializedStringCompletionContext;

use super::{CompletionPrefix, CompletionSyntaxContext};

impl<'source> CompletionSyntaxContext<'source> {
    /// Scan a string-owned word without changing the prefix rules for Rust identifiers.
    pub(super) fn string_word_prefix_impl(
        &self,
        allows_hyphen: bool,
    ) -> Option<CompletionPrefix<'source>> {
        let content = self.string_content_span_impl()?;
        let cursor = usize::try_from(self.offset).ok()?;
        let content_start = usize::try_from(content.text.start).ok()?;
        if cursor < content_start || content.text.end < self.offset {
            return None;
        }
        let before_cursor = self.source.get(content_start..cursor)?;
        let relative_start = before_cursor
            .char_indices()
            .rev()
            .find(|(_, ch)| !Self::is_identifier_continue(*ch) && !(allows_hyphen && *ch == '-'))
            .map(|(index, ch)| index + ch.len_utf8())
            .unwrap_or(0);
        let start = content_start + relative_start;
        Some(CompletionPrefix {
            text: self.source.get(start..cursor)?,
            span: Span {
                text: TextSpan {
                    start: u32::try_from(start).ok()?,
                    end: self.offset,
                },
            },
        })
    }

    /// Translate speculative string offsets back to the original request source.
    pub(super) fn string_content_span_impl(&self) -> Option<Span> {
        let literal = ast::String::cast(self.string_marker.as_ref()?.clone())?;
        let speculative = literal.text_range_between_quotes()?;

        // The speculative parse replaces only the ordinary identifier prefix with MARKER. Text
        // before the cursor keeps its offsets; text after it must be shifted back by that delta.
        let delta = i64::try_from(Self::MARKER.len()).ok()?
            - i64::try_from(self.prefix.text().len()).ok()?;
        let end = i64::from(u32::from(speculative.end())) - delta;
        let end = u32::try_from(end).ok()?;
        let start = u32::from(speculative.start());
        (start <= self.offset && self.offset <= end).then_some(Span {
            text: TextSpan { start, end },
        })
    }

    /// Select the expression immediately before a field-access completion marker.
    pub(super) fn postfix_receiver_span_impl(&self) -> Option<Span> {
        let field = self
            .marker
            .parent()?
            .ancestors()
            .find_map(ast::FieldExpr::cast)?;
        if field
            .name_ref()
            .is_none_or(|name| !name.syntax().text().to_string().contains(Self::MARKER))
        {
            return None;
        }
        let receiver = field.expr()?;
        let dot = field.dot_token()?;
        let receiver_range = receiver.syntax().text_range();
        if receiver_range.end() > dot.text_range().start() {
            return None;
        }
        Some(Span::from_text_range(receiver_range))
    }

    /// Recognize ABI, environment, and format-string positions without widening ordinary strings.
    ///
    /// Format completion requires the recognized macro's format argument and an unmatched single
    /// `{`; doubled braces and later string arguments remain plain string contents.
    pub(super) fn string_completion_context(&self) -> Option<SpecializedStringCompletionContext> {
        let marker = self.string_marker.as_ref()?;
        if marker.kind() != SyntaxKind::STRING || !marker.text().contains(Self::MARKER) {
            return None;
        }
        if marker
            .parent()?
            .ancestors()
            .any(|node| ast::Abi::can_cast(node.kind()))
        {
            return Some(SpecializedStringCompletionContext::Abi);
        }

        let call = marker
            .parent()?
            .ancestors()
            .find_map(ast::MacroCall::cast)?;
        let macro_name = call
            .path()?
            .syntax()
            .text()
            .to_string()
            .rsplit("::")
            .next()?
            .to_string();
        if matches!(macro_name.as_str(), "env" | "option_env") {
            return (Self::macro_argument_index(&call, marker)? == 0)
                .then_some(SpecializedStringCompletionContext::Environment);
        }
        if !matches!(
            macro_name.as_str(),
            "format"
                | "format_args"
                | "print"
                | "println"
                | "eprint"
                | "eprintln"
                | "panic"
                | "write"
                | "writeln"
        ) {
            return None;
        }

        // `write!` and `writeln!` receive their destination first. The other recognized format
        // macros receive the format literal first. A string in any later argument is an ordinary
        // expression and must not acquire format-capture completion.
        let expected_argument = usize::from(matches!(macro_name.as_str(), "write" | "writeln"));
        if Self::macro_argument_index(&call, marker)? != expected_argument {
            return None;
        }

        // Format capture completion is useful only inside an unmatched single `{...}` field.
        // Doubled braces are literal text and must not activate semantic candidates.
        let literal = marker.text().to_string();
        let marker = literal.find(Self::MARKER)?;
        let before = literal.get(..marker)?;
        let mut open = false;
        let bytes = before.as_bytes();
        let mut index = 0;
        while index < bytes.len() {
            match bytes[index] {
                b'{' if bytes.get(index + 1) == Some(&b'{') => index += 2,
                b'}' if bytes.get(index + 1) == Some(&b'}') => index += 2,
                b'{' => {
                    open = true;
                    index += 1;
                }
                b'}' => {
                    open = false;
                    index += 1;
                }
                _ => index += 1,
            }
        }
        if !open {
            return None;
        }

        let args = call.token_tree()?.syntax().text().to_string();
        let named_arguments = Self::comma_list_entries(&args)
            .into_iter()
            .filter_map(|entry| {
                entry
                    .split_once('=')
                    .map(|(name, _)| name.trim().to_string())
            })
            .filter(|name| name.chars().all(Self::is_identifier_continue))
            .collect();
        Some(SpecializedStringCompletionContext::Format { named_arguments })
    }

    /// Return the top-level macro argument containing the speculative string token.
    fn macro_argument_index(
        call: &ast::MacroCall,
        marker: &rg_syntax::SyntaxToken,
    ) -> Option<usize> {
        let token_tree = call.token_tree()?;
        if marker.parent().as_ref() != Some(token_tree.syntax()) {
            return None;
        }

        let mut argument = 0;
        for element in token_tree.syntax().children_with_tokens() {
            let Some(token) = element.into_token() else {
                continue;
            };
            if token == marker.clone() {
                return Some(argument);
            }
            if token.kind() == SyntaxKind::COMMA {
                argument += 1;
            }
        }
        None
    }

    /// Recognize the fragment name after `$binding:` inside a macro definition.
    pub(super) fn is_macro_fragment_site(&self) -> bool {
        if !self.accepts_completion_site()
            || self
                .previous_non_trivia_token()
                .is_none_or(|token| token.kind() != SyntaxKind::COLON)
        {
            return false;
        }
        self.marker.parent().is_some_and(|parent| {
            parent.ancestors().any(|node| {
                ast::MacroRules::can_cast(node.kind()) || ast::MacroDef::can_cast(node.kind())
            })
        })
    }
}
