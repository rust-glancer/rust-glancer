//! Inline query comments share marker and whitespace rules across test suites. Each suite
//! supplies its query names and interprets the calls; this module only handles source positions
//! and the small Rust call syntax used by annotations.

use rg_syntax::{Edition, LexedStr, SyntaxKind};
use syn::parse::Parser as _;

use super::MarkedText;
use crate::{FixtureMarker, FixtureMarkers, FixtureSpec};

// More annotations make a source line hard to follow; crowded lines use explicit query lists.
const MAX_MARKERS_PER_LINE: usize = 3;

pub struct InlineFixture {
    pub files: Vec<(String, String)>,
    pub markers: FixtureMarkers,
    pub queries: Vec<InlineQuery>,
}

impl InlineFixture {
    /// Recognize only the query names supplied by the test suite. Other comments remain source.
    pub fn parse(fixture: &str, names: &[&str]) -> Self {
        let fixture = FixtureSpec::parse(fixture);
        let mut files = Vec::new();
        let mut cleaned_markers = Vec::new();
        let mut queries = Vec::new();

        for file in fixture.files() {
            let markers = fixture
                .markers()
                .iter()
                .filter(|(_, marker)| marker.path == file.relative_path())
                .collect::<Vec<_>>();
            let cleaned = Self::clean(file.relative_path(), file.contents(), markers, names);
            cleaned_markers.extend(
                cleaned
                    .text
                    .markers
                    .iter()
                    .map(|(name, marker)| (name.to_string(), marker.clone())),
            );
            queries.extend(cleaned.queries);
            files.push((file.relative_path().to_string(), cleaned.text.text));
        }

        Self {
            files,
            markers: cleaned_markers.into_iter().collect(),
            queries,
        }
    }

    fn clean(
        path: &str,
        source: &str,
        mut markers: Vec<(&str, &FixtureMarker)>,
        names: &[&str],
    ) -> InlineSource {
        let mut queries = Vec::new();
        let mut cleaned_markers = Vec::new();
        markers.sort_by_key(|(_, marker)| marker.offset);

        // First find query comments. Lexing each Rust file separately keeps string contents,
        // block comments, and non-Rust fixture files out of the instruction syntax.
        let mut query_comments = Vec::new();
        if path.ends_with(".rs") {
            let lexed = LexedStr::new(Edition::CURRENT, source);
            for token in 0..lexed.len() {
                if lexed.kind(token) != SyntaxKind::COMMENT {
                    continue;
                }
                let range = lexed.text_range(token);
                let Some(comment) = source[range.clone()].strip_prefix("//") else {
                    continue;
                };
                let comment = comment.trim_start();
                let Some((name, _)) = comment.split_once('(') else {
                    continue;
                };
                if !names.contains(&name.trim()) {
                    continue;
                }
                query_comments.push((range, comment));
            }
        }

        // Bind each comment to that line's markers. Check the readability rules before
        // interpreting calls, so a crowded or mismatched line gets a direct explanation.
        let mut removals = Vec::new();
        for (range, comment) in query_comments {
            let line_start = source[..range.start].rfind('\n').map_or(0, |at| at + 1);
            let line_number = source[..line_start].lines().count() + 1;
            let location = format!("{}:{line_number}", path);
            let line_markers = markers
                .iter()
                .filter(|(_, marker)| {
                    let offset = marker.offset as usize;
                    (line_start..=range.start).contains(&offset)
                })
                .collect::<Vec<_>>();
            assert!(
                !line_markers.is_empty(),
                "{location}: an inline query comment needs a marker on the same line"
            );
            assert!(
                line_markers.len() <= MAX_MARKERS_PER_LINE,
                "{location}: at most {MAX_MARKERS_PER_LINE} markers fit on an inline query line; use an explicit query list otherwise"
            );

            let expressions =
                syn::punctuated::Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated
                    .parse_str(comment)
                    .unwrap_or_else(|error| panic!("{location}: invalid inline queries: {error}"));
            assert_eq!(
                expressions.len(),
                line_markers.len(),
                "{location}: inline queries must match the markers from left to right"
            );
            for (expression, (name, marker)) in expressions.iter().zip(&line_markers) {
                let name = Self::marker_name(name, marker, source);
                queries.push(InlineQuery {
                    marker: name,
                    call: InlineCall::parse(expression),
                });
            }

            // Remove the separator whitespace too, so `use crate::$cursor$` stays at
            // EOF. Whitespace before the final marker still belongs to the fixture.
            let last_marker = line_markers
                .last()
                .expect("inline query has a marker")
                .1
                .offset as usize;
            let trimmed = source[line_start..range.start].trim_end_matches([' ', '\t']);
            let start = (line_start + trimmed.len()).max(last_marker);
            removals.push(start..range.end);
        }

        // Remove instructions from the end, leaving earlier source coordinates valid.
        let mut cleaned = source.to_string();
        for range in removals.iter().rev() {
            cleaned.replace_range(range.clone(), "");
        }

        // Shift later markers by the removed comments. Materialize these parsed files
        // directly so escaped literal dollars are not interpreted as markers again.
        for (name, marker) in markers {
            let offset = marker.offset as usize;
            if removals
                .iter()
                .any(|range| range.start < offset && offset < range.end)
            {
                continue;
            }
            let scoped_name = Self::marker_name(name, marker, source);
            let query_marker = if queries.iter().any(|query| query.marker == scoped_name) {
                scoped_name
            } else {
                name.to_string()
            };
            let removed = removals
                .iter()
                .filter(|range| range.end <= offset)
                .map(|range| range.len())
                .sum::<usize>();
            let mut marker = marker.clone();
            marker.offset = u32::try_from(offset - removed).expect("cleaned offset fits u32");
            cleaned_markers.push((query_marker, marker));
        }
        InlineSource {
            text: MarkedText {
                text: cleaned,
                markers: cleaned_markers.into_iter().collect(),
            },
            queries,
        }
    }

    // Inline numeric IDs belong to a source line. Include that line's identity in the lookup
    // name so the shared fixture-wide lookup can still find one cursor for each query.
    fn marker_name(name: &str, marker: &FixtureMarker, source: &str) -> String {
        if name.bytes().all(|byte| byte.is_ascii_digit()) {
            let line = source[..marker.offset as usize]
                .bytes()
                .filter(|byte| *byte == b'\n')
                .count()
                + 1;
            format!("{}:{line}:{name}", marker.path)
        } else {
            name.to_string()
        }
    }
}

pub struct InlineSource {
    pub text: MarkedText,
    pub queries: Vec<InlineQuery>,
}

impl InlineSource {
    /// Editor snippets keep their exact whitespace, including a cursor at EOF.
    pub fn parse(path: &str, source: &str, names: &[&str]) -> Self {
        let marked = MarkedText::parse(source);
        InlineFixture::clean(path, marked.text(), marked.markers.iter().collect(), names)
    }
}

pub struct InlineQuery {
    pub marker: String,
    pub call: InlineCall,
}

/// A constructor and its chained options, in their written order.
pub struct InlineCall {
    pub name: String,
    pub arguments: Vec<InlineArgument>,
    pub modifiers: Vec<InlineCall>,
}

impl InlineCall {
    fn parse(expression: &syn::Expr) -> Self {
        if let syn::Expr::MethodCall(method) = expression {
            assert!(
                method.turbofish.is_none(),
                "inline query modifiers do not take type arguments"
            );
            let mut call = Self::parse(&method.receiver);
            call.modifiers.push(Self {
                name: method.method.to_string(),
                arguments: method.args.iter().map(InlineArgument::parse).collect(),
                modifiers: Vec::new(),
            });
            return call;
        }
        let syn::Expr::Call(call) = expression else {
            panic!("inline query should be a function call");
        };
        let syn::Expr::Path(function) = call.func.as_ref() else {
            panic!("inline query should name a query constructor");
        };
        assert!(
            function.qself.is_none()
                && function
                    .path
                    .segments
                    .iter()
                    .all(|segment| segment.arguments.is_empty()),
            "inline query constructors do not take type arguments"
        );
        Self {
            name: function
                .path
                .segments
                .iter()
                .map(|segment| segment.ident.to_string())
                .collect::<Vec<_>>()
                .join("::"),
            arguments: call.args.iter().map(InlineArgument::parse).collect(),
            modifiers: Vec::new(),
        }
    }
}

pub enum InlineArgument {
    String(String),
    Bool(bool),
    Strings(Vec<String>),
    Call(InlineCall),
}

impl InlineArgument {
    fn parse(expression: &syn::Expr) -> Self {
        match expression {
            syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(value),
                ..
            }) => Self::String(value.value()),
            syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Bool(value),
                ..
            }) => Self::Bool(value.value),
            syn::Expr::Reference(reference) => {
                let syn::Expr::Array(array) = reference.expr.as_ref() else {
                    panic!("inline query slice should contain string literals");
                };
                Self::Strings(
                    array
                        .elems
                        .iter()
                        .map(|expression| {
                            let Self::String(value) = Self::parse(expression) else {
                                panic!("inline query slice should contain string literals");
                            };
                            value
                        })
                        .collect(),
                )
            }
            syn::Expr::Call(_) | syn::Expr::MethodCall(_) => {
                Self::Call(InlineCall::parse(expression))
            }
            _ => {
                panic!("inline query arguments should be strings, booleans, string slices or calls")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::InlineSource;
    use crate::testonly::MarkedText;

    #[test]
    fn cleans_editor_queries_without_moving_cursors_or_explicit_markers() {
        let source = InlineSource::parse(
            "src/lib.rs",
            r#"let literal = "\$0";
let value$anchor$ = 1;
user.i$0d(); // hover("member")
user.$11 // complete("members")
    $0 // references("uses", true)"#,
            &["hover", "complete", "references"],
        );
        let expected = MarkedText::parse(
            r#"let literal = "\$0";
let value$anchor$ = 1;
user.i$member$d();
user.$members$
    $uses$"#,
        );
        assert_eq!(source.text.text(), expected.text());
        assert_eq!(source.text.offset("anchor"), expected.offset("anchor"));
        for (query, marker) in source.queries.iter().zip(["member", "members", "uses"]) {
            assert_eq!(source.text.offset(&query.marker), expected.offset(marker));
        }
        assert_eq!(source.queries.len(), 3);
    }
}
