//! Split format placeholders into names and formatting syntax.
//!
//! Body lowering uses the names as variable references; highlighting uses both kinds of parts.
//! Both need ranges in the written literal, including its quotes and escapes.

use std::ops::Range;

use rustc_parse_format::{Count, ParseMode, Parser, Piece, Position};

use super::{AstToken as _, IsString as _, String};
use crate::{TextRange, TextSize};

/// One part of a placeholder, with byte offsets from the start of the string token.
/// The offsets include opening quotes and raw-string prefixes such as `r##"`.
pub struct FormatStringPart<'a> {
    pub range: TextRange,
    pub kind: FormatStringPartKind<'a>,
}

pub enum FormatStringPartKind<'a> {
    Name(&'a str),
    Specifier,
}

impl String {
    /// Visit the names and formatting syntax in each placeholder, in source order.
    ///
    /// For `"{user:>width$}"`, names are `user` and `width`; other parts are `{`, `:>`, and `$}`.
    /// Ordinary string text is skipped so syntax highlighting can keep its escape colors.
    /// Names are unescaped (`caf\u{e9}` becomes `café`), but ranges cover the written text.
    pub fn format_parts(&self, mut callback: impl FnMut(FormatStringPart<'_>)) {
        let Ok(value) = self.value() else {
            return;
        };
        let style = self.is_raw().then(|| {
            self.text()
                .bytes()
                .skip(1)
                .take_while(|&byte| byte == b'#')
                .count()
        });
        // Give rustc both the decoded value and the original token. The value tells it
        // which characters form placeholders; the token lets it return source offsets
        // for spellings such as `\u{7b}user\u{7d}`.
        let mut parser = Parser::new(
            &value,
            style,
            Some(self.text().to_owned()),
            false,
            ParseMode::Format,
        );
        let mut emit = |range: Range<usize>, kind| {
            if range.start < range.end {
                callback(FormatStringPart {
                    range: TextRange::new(
                        TextSize::try_from(range.start)
                            .expect("literal offset fits syntax storage"),
                        TextSize::try_from(range.end).expect("literal offset fits syntax storage"),
                    ),
                    kind,
                });
            }
        };
        loop {
            // Read the opening brace's source range before advancing the parser.
            // A brace written as `\u{7b}` must keep the range of that whole escape.
            let start = parser.peek().map(|(range, _, _)| range.start);
            let place = parser.arg_places.len();
            let Some(piece) = parser.next() else {
                break;
            };
            let Piece::NextArgument(argument) = piece else {
                continue;
            };
            let mut cursor = start.expect("parsed argument has an opening brace");
            // An unfinished `{user` may have no closing-brace range. In that case,
            // stop at the literal's closing quote, or at the end if the quote is missing too.
            let end = parser.arg_places.get(place).map_or_else(
                || {
                    self.text().len()
                        - self
                            .close_quote_text_range()
                            .map_or(0, |range| usize::from(range.len()))
                },
                |range| range.end,
            );
            // In `{value:width$.precision$}`, the names always appear in this order.
            // Treat the gaps around them as formatting syntax, including braces and `$`.
            let names = [
                match argument.position {
                    Position::ArgumentNamed(name) => Some((name, argument.position_span)),
                    _ => None,
                },
                match argument.format.width {
                    Count::CountIsName(name, range) => Some((name, range)),
                    _ => None,
                },
                match argument.format.precision {
                    Count::CountIsName(name, range) => Some((name, range)),
                    _ => None,
                },
            ];
            for (name, range) in names.into_iter().flatten() {
                let range = self.format_name_range(range);
                emit(cursor..range.start, FormatStringPartKind::Specifier);
                emit(range.clone(), FormatStringPartKind::Name(name));
                cursor = range.end;
            }
            emit(cursor..end, FormatStringPartKind::Specifier);
        }
    }

    fn format_name_range(&self, range: Range<usize>) -> Range<usize> {
        // The parser includes trailing whitespace in `{name  }`. Find the last non-whitespace
        // character after unescaping, then keep its whole written spelling: `caf\u{e9}`
        // must end after the closing brace of `\u{e9}`.
        let snippet = &self.text()[range.clone()];
        let mut end = range.start;
        if self.is_raw() {
            end += snippet.trim_end().len();
        } else {
            self.unescape(snippet, |character_range, character| {
                if character.is_ok_and(|character| !character.is_whitespace()) {
                    end = range.start + character_range.end;
                }
            });
        }
        range.start..end
    }
}
