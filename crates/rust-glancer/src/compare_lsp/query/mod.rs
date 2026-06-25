//! Typed query vectors for each comparison fixture.

mod model;
mod parser;

use std::sync::LazyLock;

pub(crate) use self::model::{QueryCase, QueryKind, SourcePosition};

use self::parser::parse_query_cases;

pub(crate) fn rust_analyzer_cases() -> &'static [QueryCase] {
    RUST_ANALYZER_CASES.as_slice()
}

static RUST_ANALYZER_CASES: LazyLock<Vec<QueryCase>> =
    LazyLock::new(|| parse_query_cases(RUST_ANALYZER_CASES_TEXT));

// Format:
//
// [<lsp-method>(<optional-params>)]
// <fixture-relative-path>:<zero-based-line>:<zero-based-character> # <label>
//
// Keep cases fixture-root-local so location normalization remains meaningful. The line/character
// coordinates are LSP coordinates, not byte offsets.
const RUST_ANALYZER_CASES_TEXT: &str = r#"
[textDocument/references(includeDeclaration=true)]
crates/ide/src/call_hierarchy.rs:25:11 # references/type: CallHierarchyConfig
crates/ide/src/call_hierarchy.rs:43:14 # references/function: incoming_calls
crates/ide/src/hover.rs:35:11 # references/type: HoverConfig
crates/ide/src/navigation_target.rs:31:11 # references/type: NavigationTarget
crates/ide/src/navigation_target.rs:120:10 # references/trait: TryToNav
crates/ide/src/references.rs:46:11 # references/type: ReferenceSearchResult
crates/ide/src/references.rs:62:11 # references/type: Declaration
crates/ide/src/references.rs:90:11 # references/config: FindAllRefsConfig
crates/ide/src/references.rs:120:14 # references/function: find_all_refs
crates/ide/src/references.rs:132:16 # references/helper-call: retain_adt_literal_usages
crates/ide/src/references.rs:174:23 # references/helper-call: handle_control_flow_keywords
crates/ide/src/references.rs:209:14 # references/function: find_defs

[textDocument/references(includeDeclaration=false)]
crates/ide/src/lib.rs:110:17 # references/config-no-decl: FindAllRefsConfig
crates/ide/src/navigation_target.rs:140:11 # references/method-no-decl: focus_or_full_range

[textDocument/definition]
crates/ide/src/call_hierarchy.rs:36:4 # definition/qualified-call: goto_definition
crates/ide/src/call_hierarchy.rs:39:9 # definition/config-constructor: GotoDefinitionConfig
crates/ide/src/child_modules.rs:30:48 # definition/associated-function: from_module_to_decl
crates/ide/src/child_modules.rs:54:68 # definition/method-call: focus_or_full_range
crates/ide/src/goto_definition.rs:125:16 # definition/helper-call: try_lookup_include_path
crates/ide/src/lib.rs:89:21 # definition/reexport: GotoDefinitionConfig
crates/ide/src/lib.rs:93:21 # definition/reexport: HoverConfig
crates/ide/src/lib.rs:109:24 # definition/reexport: NavigationTarget
crates/ide/src/lib.rs:110:17 # definition/reexport: FindAllRefsConfig
crates/ide/src/references.rs:132:16 # definition/helper-call: retain_adt_literal_usages
crates/ide/src/references.rs:174:23 # definition/helper-call: handle_control_flow_keywords
crates/ide/src/references.rs:204:17 # definition/helper-call: find_defs

[textDocument/typeDefinition]
crates/ide/src/call_hierarchy.rs:20:17 # type_definition/field: NavigationTarget
crates/ide/src/call_hierarchy.rs:21:21 # type_definition/field: FileRange
crates/ide/src/hover.rs:120:17 # type_definition/field: Markup
crates/ide/src/hover.rs:121:22 # type_definition/field: HoverAction
crates/ide/src/navigation_target.rs:49:15 # type_definition/field: Symbol
crates/ide/src/references.rs:122:15 # type_definition/param: FilePosition

[textDocument/implementation]
crates/ide/src/hover.rs:79:9 # implementation/enum: HoverAction
crates/ide/src/navigation_target.rs:31:11 # implementation/type: NavigationTarget
crates/ide/src/navigation_target.rs:120:10 # implementation/trait: TryToNav
crates/ide/src/navigation_target.rs:121:7 # implementation/trait-method: try_to_nav
crates/ide/src/navigation_target.rs:139:9 # implementation/inherent-impl: NavigationTarget
crates/ide/src/navigation_target.rs:305:7 # implementation/trait-impl: TryToNav

[textDocument/documentHighlight]
crates/ide/src/call_hierarchy.rs:43:14 # document_highlight/function: incoming_calls
crates/ide/src/hover.rs:130:14 # document_highlight/function: hover
crates/ide/src/navigation_target.rs:120:10 # document_highlight/trait: TryToNav
crates/ide/src/navigation_target.rs:140:11 # document_highlight/method: focus_or_full_range
crates/ide/src/references.rs:120:14 # document_highlight/function: find_all_refs
crates/ide/src/references.rs:209:14 # document_highlight/function: find_defs

[textDocument/hover]
crates/ide/src/call_hierarchy.rs:19:11 # hover/type: CallItem
crates/ide/src/call_hierarchy.rs:98:14 # hover/function: outgoing_calls
crates/ide/src/hover.rs:35:11 # hover/config: HoverConfig
crates/ide/src/hover.rs:79:9 # hover/enum: HoverAction
crates/ide/src/hover.rs:119:11 # hover/type: HoverResult
crates/ide/src/hover.rs:130:14 # hover/function: hover
crates/ide/src/hover.rs:159:3 # hover/helper: hover_offset
crates/ide/src/lib.rs:109:24 # hover/reexport: NavigationTarget
crates/ide/src/navigation_target.rs:31:11 # hover/type: NavigationTarget
crates/ide/src/navigation_target.rs:120:10 # hover/trait: TryToNav
"#;
