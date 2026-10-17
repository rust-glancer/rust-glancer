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
//! Keep explicit query lists when a marker needs several queries, a line has more than three
//! markers, or an unterminated string would swallow the query comment.
//!
//! Instructions and their separator whitespace are removed before analysis. Keep the source
//! lines and cursor positions intact, including spaces before a cursor at EOF.

use test_fixture::{
    FixtureFile, FixtureMarkers,
    testonly::inline::{InlineArgument, InlineCall, InlineFixture},
};

use super::{AnalysisQuery, ReferenceQuery};

pub(super) struct InlineAnalysisFixture {
    pub(super) files: Vec<FixtureFile>,
    pub(super) markers: FixtureMarkers,
    pub(super) queries: Vec<AnalysisQuery>,
}

impl InlineAnalysisFixture {
    pub(super) fn parse(fixture: &str) -> Self {
        let fixture = InlineFixture::parse(
            fixture,
            &[
                "symbol",
                "resolve",
                "goto",
                "goto_type",
                "goto_impl",
                "ty",
                "complete",
                "complete_verbose",
                "complete_with_source",
                "complete_verbose_with_source",
                "complete_keywords_with_source",
                "complete_keywords_verbose_with_source",
                "hover",
                "code_actions",
                "automatic_code_actions",
                "prepare_rename",
                "references",
                "rename",
            ],
        );
        let queries = fixture
            .queries
            .iter()
            .map(|query| Self::query(&query.call, &query.marker))
            .collect::<Vec<_>>();
        assert!(
            !queries.is_empty(),
            "fixture should contain inline analysis queries"
        );
        for (name, marker) in fixture.markers.iter() {
            assert!(
                queries.iter().any(|query| query.marker == name),
                "{}: marker `{name}` needs an inline query; use an explicit query list for unterminated strings",
                marker.path
            );
        }
        Self {
            files: fixture.files,
            markers: fixture.markers,
            queries,
        }
    }

    // Simple constructors share a title argument; rename and references have extra arguments.
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

    fn query(call: &InlineCall, marker: &str) -> AnalysisQuery {
        use InlineArgument::{Call, String};
        let mut query = match (call.name.as_str(), call.arguments.as_slice()) {
            ("rename", [String(title), String(new_name)]) => {
                AnalysisQuery::rename(title, marker, new_name)
            }
            ("references", [String(title), Call(scope)]) => {
                AnalysisQuery::references(title, marker, Self::references(scope))
            }
            ("rename" | "references", _) => {
                panic!("expected rename(\"title\", \"new name\") or references(\"title\", scope)")
            }
            (name, [String(title)]) => Self::constructor(name)
                .unwrap_or_else(|| panic!("unknown inline query `{name}`"))(
                title, marker
            ),
            (name, _) => panic!("inline query `{name}` takes one title string"),
        };
        // Apply options in source order, so the annotation reads like its explicit query.
        for modifier in &call.modifiers {
            query = match (modifier.name.as_str(), modifier.arguments.as_slice()) {
                ("in_lib", [String(package)]) => query.in_lib(package),
                ("in_bin", [String(package)]) => query.in_bin(package),
                ("matching", [String(prefix)]) => query.matching(prefix),
                _ => panic!(
                    "expected .in_lib(\"package\"), .in_bin(\"package\") or .matching(\"prefix\")"
                ),
            };
        }
        query
    }

    fn references(call: &InlineCall) -> ReferenceQuery {
        use InlineArgument::Strings;
        let mut scope = match (call.name.as_str(), call.arguments.as_slice()) {
            ("ReferenceQuery::all", []) => ReferenceQuery::all(),
            ("ReferenceQuery::current_target", []) => ReferenceQuery::current_target(),
            ("ReferenceQuery::current_file", []) => ReferenceQuery::current_file(),
            (name @ ("ReferenceQuery::files" | "ReferenceQuery::libs"), [Strings(values)]) => {
                let values = values.iter().map(String::as_str).collect::<Vec<_>>();
                if name == "ReferenceQuery::files" {
                    ReferenceQuery::files(&values)
                } else {
                    ReferenceQuery::libs(&values)
                }
            }
            _ => panic!(
                "expected ReferenceQuery::all(), current_target(), current_file(), files(&[\"path\"]) or libs(&[\"package\"])"
            ),
        };
        for modifier in &call.modifiers {
            match (modifier.name.as_str(), modifier.arguments.as_slice()) {
                ("without_declaration", []) => scope = scope.without_declaration(),
                _ => panic!("reference queries support only `.without_declaration()`"),
            }
        }
        scope
    }
}

#[test]
fn strips_directives_without_changing_source_or_marker_positions() {
    use test_fixture::FixtureSpec;

    let fixture = InlineAnalysisFixture::parse(
        r##"
//- /src/lib.rs crate:inline_queries edition:2015
let literal = "\$0 and \$1 and \$11$ and \$named$";
let raw = r#"// ty("inside a string")"#;
/* // hover("inside a block comment") */
// example("ordinary fixture comment")
let pair$binding$ = (1$0, 2$1); // ty("binding, \"quoted\""), ty("left"), ty(r#"right"#)
let digits = 1$0$2; // ty("digits")
name.$11 // complete_with_source("members").matching("na").in_lib("inline_queries")
//- /src/main.rs crate:inline_queries edition:2015 kind:bin
use $0 // complete_keywords_with_source("import at EOF").in_bin("inline_queries")
"##,
    );
    let expected = FixtureSpec::parse(
        r##"
//- /src/lib.rs crate:inline_queries edition:2015
let literal = "\$0 and \$1 and \$11$ and \$named$";
let raw = r#"// ty("inside a string")"#;
/* // hover("inside a block comment") */
// example("ordinary fixture comment")
let pair$binding$ = (1$left$, 2$right$);
let digits = 1$digits$2;
name.$member$
//- /src/main.rs crate:inline_queries edition:2015 kind:bin
use $0
"##,
    );
    assert_eq!(fixture.files, expected.files());
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
