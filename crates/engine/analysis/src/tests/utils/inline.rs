//! Read inline queries as instructions attached to a source line:
//!
//! ```text
//! let pair$0 = (1$1, 2$2); // ty("binding"), ty("left"), ty("right")
//! value.$0; // complete("members").matching("name").in_lib("app")
//! ```
//!
//! Each marker has one query. Pair them from left to right, and run the lines in source order.
//! Numeric IDs are local to each line: reuse `$0` on the next line or in another file.
//! Any numeric ID works, including `$11`; use `$0$` when source digits follow the marker.
//! Named markers such as `$method$` keep their fixture-wide names.
//! Only trailing Rust line comments starting with a known query name are instructions; ordinary
//! comments and text inside literals remain source text.
//!
//! Keep explicit query lists when a marker needs several queries, a line exceeds the limit
//! below, or an unterminated string would swallow the query comment.
//!
//! Instructions and their separator whitespace are removed before analysis. Keep the source
//! lines and cursor positions intact, including spaces before a cursor at EOF.

use rg_syntax::{Edition, LexedStr, SyntaxKind};
use syn::parse::Parser as _;
use test_fixture::{FixtureMarker, FixtureMarkers, FixtureSpec};

use super::{AnalysisQuery, ReferenceQuery};

// More annotations make a source line hard to follow; crowded lines use explicit query lists.
const MAX_MARKERS_PER_LINE: usize = 3;

pub(super) struct InlineAnalysisFixture {
    pub(super) files: Vec<(String, String)>,
    pub(super) markers: FixtureMarkers,
    pub(super) queries: Vec<AnalysisQuery>,
}

impl InlineAnalysisFixture {
    pub(super) fn parse(fixture: &str) -> Self {
        let fixture = FixtureSpec::parse(fixture);
        let mut files = Vec::new();
        let mut cleaned_markers = Vec::new();
        let mut queries = Vec::new();

        for file in fixture.files() {
            let source = file.contents();
            let mut markers = fixture
                .markers()
                .iter()
                .filter(|(_, marker)| marker.path == file.relative_path())
                .collect::<Vec<_>>();
            markers.sort_by_key(|(_, marker)| marker.offset);

            // First find query comments. Lexing each Rust file separately keeps string contents,
            // block comments, and non-Rust fixture files out of the instruction syntax.
            let mut query_comments = Vec::new();
            if file.relative_path().ends_with(".rs") {
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
                    if !matches!(name.trim(), "references" | "rename")
                        && Self::constructor(name.trim()).is_none()
                    {
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
                let location = format!("{}:{line_number}", file.relative_path());
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
                        .unwrap_or_else(|error| {
                            panic!("{location}: invalid inline queries: {error}")
                        });
                assert_eq!(
                    expressions.len(),
                    line_markers.len(),
                    "{location}: inline queries must match the markers from left to right"
                );
                for (expression, (name, marker)) in expressions.iter().zip(&line_markers) {
                    let name = Self::marker_name(name, marker, source);
                    queries.push(Self::query(expression, &name));
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
                let query_marker = Self::marker_name(name, marker, source);
                assert!(
                    queries.iter().any(|query| query.marker == query_marker),
                    "{}: marker `{name}` needs an inline query; use an explicit query list for unterminated strings",
                    file.relative_path()
                );
                let removed = removals
                    .iter()
                    .filter(|range| range.end <= offset)
                    .map(|range| range.len())
                    .sum::<usize>();
                let mut marker = marker.clone();
                marker.offset = u32::try_from(offset - removed).expect("cleaned offset fits u32");
                cleaned_markers.push((query_marker, marker));
            }
            files.push((file.relative_path().to_string(), cleaned));
        }

        assert!(
            !queries.is_empty(),
            "fixture should contain inline analysis queries"
        );
        Self {
            files,
            markers: cleaned_markers.into_iter().collect(),
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

    // These constructors all have the same `query("title")` syntax. Rename additionally
    // needs a new name, and references needs a scope.
    fn constructor(name: &str) -> Option<fn(&str, &str) -> AnalysisQuery> {
        Some(match name {
            "symbol" => AnalysisQuery::symbol,
            "resolve" => AnalysisQuery::resolve,
            "goto" => AnalysisQuery::goto,
            "goto_type" => AnalysisQuery::goto_type,
            "goto_impl" => AnalysisQuery::goto_impl,
            "ty" => AnalysisQuery::ty,
            "complete" => AnalysisQuery::complete,
            "complete_verbose" => AnalysisQuery::complete_verbose,
            "complete_with_source" => AnalysisQuery::complete_with_source,
            "complete_verbose_with_source" => AnalysisQuery::complete_verbose_with_source,
            "complete_keywords_with_source" => AnalysisQuery::complete_keywords_with_source,
            "complete_keywords_verbose_with_source" => {
                AnalysisQuery::complete_keywords_verbose_with_source
            }
            "hover" => AnalysisQuery::hover,
            "code_actions" => AnalysisQuery::code_actions,
            "automatic_code_actions" => AnalysisQuery::automatic_code_actions,
            "prepare_rename" => AnalysisQuery::prepare_rename,
            _ => return None,
        })
    }

    // Each signature pattern states the accepted query name and arguments together.
    fn query(expression: &syn::Expr, marker: &str) -> AnalysisQuery {
        if let syn::Expr::MethodCall(method) = expression {
            assert!(
                method.turbofish.is_none(),
                "inline query modifiers do not take type arguments"
            );
            // Build the receiver first so chained options apply in their written order.
            let query = Self::query(&method.receiver, marker);
            let name = method.method.to_string();
            let arguments = method.args.iter().collect::<Vec<_>>();
            return match (name.as_str(), arguments.as_slice()) {
                ("in_lib", [package]) => query.in_lib(&Self::string(package)),
                ("in_bin", [package]) => query.in_bin(&Self::string(package)),
                ("matching", [prefix]) => query.matching(&Self::string(prefix)),
                _ => panic!(
                    "expected .in_lib(\"package\"), .in_bin(\"package\") or .matching(\"prefix\")"
                ),
            };
        }

        let (function, arguments) = Self::call(expression);
        let name = function
            .get_ident()
            .expect("inline query names are unqualified")
            .to_string();
        match (name.as_str(), arguments.as_slice()) {
            ("rename", [title, new_name]) => {
                AnalysisQuery::rename(&Self::string(title), marker, &Self::string(new_name))
            }
            ("references", [title, scope]) => {
                AnalysisQuery::references(&Self::string(title), marker, Self::references(scope))
            }
            ("rename" | "references", _) => {
                panic!("expected rename(\"title\", \"new name\") or references(\"title\", scope)")
            }
            (_, [title]) => {
                let constructor = Self::constructor(&name)
                    .unwrap_or_else(|| panic!("unknown inline query `{name}`"));
                constructor(&Self::string(title), marker)
            }
            _ => panic!("inline query `{name}` takes one title string"),
        }
    }

    // Reference queries accept a scope constructor, optionally followed by
    // `.without_declaration()`. Keep the accepted scope signatures together here.
    fn references(expression: &syn::Expr) -> ReferenceQuery {
        if let syn::Expr::MethodCall(method) = expression {
            assert!(
                method.method == "without_declaration"
                    && method.args.is_empty()
                    && method.turbofish.is_none(),
                "reference queries support only `.without_declaration()`"
            );
            return Self::references(&method.receiver).without_declaration();
        }
        let (function, arguments) = Self::call(expression);
        let segments = &function.segments;
        assert!(
            segments.len() == 2 && segments[0].ident == "ReferenceQuery",
            "reference scope should use ReferenceQuery"
        );
        let name = segments[1].ident.to_string();
        match (name.as_str(), arguments.as_slice()) {
            ("all", []) => ReferenceQuery::all(),
            ("current_target", []) => ReferenceQuery::current_target(),
            ("current_file", []) => ReferenceQuery::current_file(),
            ("files" | "libs", [paths_or_packages]) => {
                let syn::Expr::Reference(reference) = paths_or_packages else {
                    panic!("reference scope `{name}` needs a string slice");
                };
                let syn::Expr::Array(array) = reference.expr.as_ref() else {
                    panic!("reference scope `{name}` needs a string slice");
                };
                let values = array.elems.iter().map(Self::string).collect::<Vec<_>>();
                let values = values.iter().map(String::as_str).collect::<Vec<_>>();
                if name == "files" {
                    ReferenceQuery::files(&values)
                } else {
                    ReferenceQuery::libs(&values)
                }
            }
            _ => panic!(
                "expected ReferenceQuery::all(), current_target(), current_file(), files(&[\"path\"]) or libs(&[\"package\"])"
            ),
        }
    }

    // Analysis queries and reference scopes use the same Rust call shape.
    fn call(expression: &syn::Expr) -> (&syn::Path, Vec<&syn::Expr>) {
        let syn::Expr::Call(call) = expression else {
            panic!("inline query should be a function call");
        };
        let syn::Expr::Path(function) = call.func.as_ref() else {
            panic!("inline query should name a query constructor");
        };
        (&function.path, call.args.iter().collect())
    }

    // Let syn decode quoted and raw string literals, including embedded commas and escapes.
    fn string(expression: &syn::Expr) -> String {
        let syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(value),
            ..
        }) = expression
        else {
            panic!("inline query argument should be a string literal");
        };
        value.value()
    }
}

#[test]
fn strips_directives_without_changing_source_or_marker_positions() {
    let fixture = InlineAnalysisFixture::parse(
        r##"
//- /Cargo.toml
[package]
name = "inline_queries"
version = "0.1.0"

//- /src/lib.rs
let literal = "\$0 and \$1 and \$11$ and \$named$";
let raw = r#"// ty("inside a string")"#;
/* // hover("inside a block comment") */
// example("ordinary fixture comment")
let pair$binding$ = (1$0, 2$1); // ty("binding, \"quoted\""), ty("left"), ty(r#"right"#)
let digits = 1$0$2; // ty("digits")
name.$11 // complete_with_source("members").matching("na").in_lib("inline_queries")
//- /src/main.rs
use $0 // complete_keywords_with_source("import at EOF").in_bin("inline_queries")
"##,
    );
    let expected = FixtureSpec::parse(
        r##"
//- /Cargo.toml
[package]
name = "inline_queries"
version = "0.1.0"

//- /src/lib.rs
let literal = "\$0 and \$1 and \$11$ and \$named$";
let raw = r#"// ty("inside a string")"#;
/* // hover("inside a block comment") */
// example("ordinary fixture comment")
let pair$binding$ = (1$left$, 2$right$);
let digits = 1$digits$2;
name.$member$
//- /src/main.rs
use $0
"##,
    );
    let expected_files = expected
        .files()
        .iter()
        .map(|file| {
            (
                file.relative_path().to_string(),
                file.contents().to_string(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(fixture.files, expected_files);
    let expected_markers = ["binding", "left", "right", "digits", "member", "0"];
    assert_eq!(fixture.queries.len(), expected_markers.len());
    for (query, expected_marker) in fixture.queries.iter().zip(expected_markers) {
        assert_eq!(
            fixture.markers.position(&query.marker),
            expected.markers().position(expected_marker),
            "{}",
            query.title,
        );
    }
    assert_eq!(
        fixture
            .queries
            .iter()
            .map(|query| query.title.as_str())
            .collect::<Vec<_>>(),
        [
            "binding, \"quoted\"",
            "left",
            "right",
            "digits",
            "members",
            "import at EOF"
        ]
    );
}
