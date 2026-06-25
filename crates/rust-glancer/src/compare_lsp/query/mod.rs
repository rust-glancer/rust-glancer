//! Typed query vectors for each comparison fixture.

mod model;
mod parser;

use std::sync::LazyLock;

pub(crate) use self::model::{QueryCase, QueryKind, SourcePosition};

use self::parser::parse_query_cases;

pub(crate) fn rust_analyzer_cases() -> &'static [QueryCase] {
    RUST_ANALYZER_CASES.as_slice()
}

static RUST_ANALYZER_CASES: LazyLock<Vec<QueryCase>> = LazyLock::new(|| {
    parse_query_cases(RUST_ANALYZER_CASES_TEXT)
        .expect("hardcoded rust-analyzer LSP comparison query cases should parse")
});

// Format:
//
// <lsp-method>(<optional-params>) <fixture-relative-path>:<zero-based-line>:<zero-based-character> # <label>
//
// Keep cases fixture-root-local so location normalization remains meaningful. The line/character
// coordinates are LSP coordinates, not byte offsets.
const RUST_ANALYZER_CASES_TEXT: &str = r#"
textDocument/references(includeDeclaration=true) crates/ide/src/call_hierarchy.rs:25:11 # references/type: CallHierarchyConfig
textDocument/references(includeDeclaration=true) crates/ide/src/call_hierarchy.rs:43:14 # references/function: incoming_calls
textDocument/references(includeDeclaration=true) crates/ide/src/hover.rs:35:11 # references/type: HoverConfig
textDocument/references(includeDeclaration=false) crates/ide/src/lib.rs:110:17 # references/config-no-decl: FindAllRefsConfig
textDocument/references(includeDeclaration=true) crates/ide/src/navigation_target.rs:31:11 # references/type: NavigationTarget
textDocument/references(includeDeclaration=true) crates/ide/src/navigation_target.rs:120:10 # references/trait: TryToNav
textDocument/references(includeDeclaration=false) crates/ide/src/navigation_target.rs:140:11 # references/method-no-decl: focus_or_full_range
textDocument/references(includeDeclaration=true) crates/ide/src/references.rs:46:11 # references/type: ReferenceSearchResult
textDocument/references(includeDeclaration=true) crates/ide/src/references.rs:62:11 # references/type: Declaration
textDocument/references(includeDeclaration=true) crates/ide/src/references.rs:90:11 # references/config: FindAllRefsConfig
textDocument/references(includeDeclaration=true) crates/ide/src/references.rs:120:14 # references/function: find_all_refs
textDocument/references(includeDeclaration=true) crates/ide/src/references.rs:132:16 # references/helper-call: retain_adt_literal_usages
textDocument/references(includeDeclaration=true) crates/ide/src/references.rs:174:23 # references/helper-call: handle_control_flow_keywords
textDocument/references(includeDeclaration=true) crates/ide/src/references.rs:209:14 # references/function: find_defs
textDocument/definition crates/ide/src/call_hierarchy.rs:36:4 # definition/qualified-call: goto_definition
textDocument/definition crates/ide/src/call_hierarchy.rs:39:9 # definition/config-constructor: GotoDefinitionConfig
textDocument/definition crates/ide/src/child_modules.rs:30:48 # definition/associated-function: from_module_to_decl
textDocument/definition crates/ide/src/child_modules.rs:54:68 # definition/method-call: focus_or_full_range
textDocument/definition crates/ide/src/goto_definition.rs:125:16 # definition/helper-call: try_lookup_include_path
textDocument/definition crates/ide/src/lib.rs:89:21 # definition/reexport: GotoDefinitionConfig
textDocument/definition crates/ide/src/lib.rs:93:21 # definition/reexport: HoverConfig
textDocument/definition crates/ide/src/lib.rs:109:24 # definition/reexport: NavigationTarget
textDocument/definition crates/ide/src/lib.rs:110:17 # definition/reexport: FindAllRefsConfig
textDocument/definition crates/ide/src/references.rs:132:16 # definition/helper-call: retain_adt_literal_usages
textDocument/definition crates/ide/src/references.rs:174:23 # definition/helper-call: handle_control_flow_keywords
textDocument/definition crates/ide/src/references.rs:204:17 # definition/helper-call: find_defs
textDocument/hover crates/ide/src/call_hierarchy.rs:19:11 # hover/type: CallItem
textDocument/hover crates/ide/src/call_hierarchy.rs:98:14 # hover/function: outgoing_calls
textDocument/hover crates/ide/src/hover.rs:35:11 # hover/config: HoverConfig
textDocument/hover crates/ide/src/hover.rs:79:9 # hover/enum: HoverAction
textDocument/hover crates/ide/src/hover.rs:119:11 # hover/type: HoverResult
textDocument/hover crates/ide/src/hover.rs:130:14 # hover/function: hover
textDocument/hover crates/ide/src/hover.rs:159:3 # hover/helper: hover_offset
textDocument/hover crates/ide/src/lib.rs:109:24 # hover/reexport: NavigationTarget
textDocument/hover crates/ide/src/navigation_target.rs:31:11 # hover/type: NavigationTarget
textDocument/hover crates/ide/src/navigation_target.rs:120:10 # hover/trait: TryToNav
"#;
