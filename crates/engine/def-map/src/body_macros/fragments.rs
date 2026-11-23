use rg_ir_model::{BodySource, Span};
use rg_syntax::{Edition, LexedStr, SyntaxNode, TextRange, TextSize};

use super::ExpandedBodyMacro;

/// Ordered runs of copied tokens in the syntax being lowered.
///
/// `forward!(f(/* reason */ x))` can project the entire call. The comment is
/// still in the written range even if the macro parser dropped it. In
/// `($x:expr, $y:expr) => { $x + $y }`, the template's `+` breaks the run.
/// Keep these runs only while the expansion is open so projecting each node
/// needs two endpoint lookups, even in a long chain of nested expressions.
pub struct CopiedArgumentFragments {
    tokens: Vec<FragmentToken>,
}

impl CopiedArgumentFragments {
    pub fn new(
        expansion: &ExpandedBodyMacro<SyntaxNode>,
        syntax: &SyntaxNode,
        call: BodySource,
        written: &str,
    ) -> Self {
        let mut tokens: Vec<FragmentToken> = Vec::new();
        let mut previous = None;
        let mut expanded = syntax
            .descendants_with_tokens()
            .filter_map(|element| element.into_token())
            .filter(|token| !token.kind().is_trivia())
            .peekable();
        while let Some(token) = expanded.next() {
            let output = token.text_range();
            let Some((source, origin)) = expansion.argument_origin(call, output) else {
                previous = None;
                continue;
            };
            let Some(mut remaining) = written
                .get(source.start as usize..source.end as usize)
                .and_then(|text| text.strip_prefix(token.text()))
            else {
                previous = None;
                continue;
            };
            let run = previous
                .filter(|prev: &FragmentToken| {
                    let Some(gap) = written.get(prev.source.end as usize..source.start as usize)
                    else {
                        return false;
                    };
                    if gap.trim().is_empty() {
                        return true;
                    }
                    let lexed = LexedStr::new(Edition::LATEST, gap);
                    (0..lexed.len()).all(|index| lexed.kind(index).is_trivia())
                        && lexed.errors().next().is_none()
                })
                .map_or(output.start(), |prev| prev.run);

            // The parser splits `tuple.0.1`'s float token `0.1` into three syntax tokens.
            // All three retain the same origin. Accept their subranges only if adjacent
            // output tokens reconstruct the entire input token byte for byte.
            let checkpoint = tokens.len();
            let mut part = FragmentToken {
                output,
                source: Span {
                    start: source.start,
                    end: source.start + u32::from(output.len()),
                },
                run,
            };
            tokens.push(part);
            while !remaining.is_empty() {
                let Some(next) = expanded.next_if(|next| {
                    let range = next.text_range();
                    range.start() == part.output.end()
                        && expansion.argument_origin(call, range) == Some((source, origin))
                        && remaining.starts_with(next.text())
                }) else {
                    break;
                };
                let range = next.text_range();
                remaining = &remaining[next.text().len()..];
                part = FragmentToken {
                    output: range,
                    source: Span {
                        start: part.source.end,
                        end: part.source.end + u32::from(range.len()),
                    },
                    run,
                };
                tokens.push(part);
            }
            if remaining.is_empty() {
                previous = Some(part);
            } else {
                tokens.truncate(checkpoint);
                previous = None;
            }
        }
        Self { tokens }
    }

    pub fn span(&self, syntax: &SyntaxNode) -> Option<Span> {
        let node = syntax.text_range();
        let mut first = syntax.first_token()?;
        let mut last = syntax.last_token()?;
        while first.kind().is_trivia() {
            first = first.next_token()?;
        }
        while last.kind().is_trivia() {
            last = last.prev_token()?;
        }
        let first = first.text_range();
        let last = last.text_range();
        if !node.contains_range(first) || !node.contains_range(last) {
            return None;
        }
        let first = &self.tokens[self
            .tokens
            .binary_search_by_key(&first.start(), |token| token.output.start())
            .ok()?];
        let last = &self.tokens[self
            .tokens
            .binary_search_by_key(&last.end(), |token| token.output.end())
            .ok()?];
        (first.run == last.run).then_some(Span {
            start: first.source.start,
            end: last.source.end,
        })
    }
}

#[derive(Clone, Copy)]
struct FragmentToken {
    output: TextRange,
    source: Span,
    run: TextSize,
}
