//! Typed query vectors for each comparison fixture.

mod model;
mod parser;

use std::sync::LazyLock;

pub(crate) use self::model::{QueryCase, QueryKind, QueryTarget, SourcePosition};
use self::parser::parse_query_cases;
use crate::compare_lsp::config::DIRTY_EDITOR_PREFIX_LINE_COUNT;

pub(crate) fn rust_analyzer_cases() -> &'static [QueryCase] {
    RUST_ANALYZER_CASES.as_slice()
}

pub(crate) fn rust_analyzer_dirty_cases() -> &'static [QueryCase] {
    RUST_ANALYZER_DIRTY_CASES.as_slice()
}

static RUST_ANALYZER_CASES: LazyLock<Vec<QueryCase>> =
    LazyLock::new(|| parse_query_cases(RUST_ANALYZER_CASES_TEXT));

static RUST_ANALYZER_DIRTY_CASES: LazyLock<Vec<QueryCase>> = LazyLock::new(|| {
    parse_query_cases(RUST_ANALYZER_DIRTY_CASES_TEXT)
        .into_iter()
        .map(|query| query.shifted_lines(DIRTY_EDITOR_PREFIX_LINE_COUNT))
        .collect()
});

// Format:
//
// [<lsp-method>(<optional-params>)]
// <fixture-relative-path>:<zero-based-line>:<zero-based-character> # <label>  (position methods)
// <fixture-relative-path> # <label>                                        (file methods)
// <workspace-symbol-query> # <label>                                       (workspace methods)
//
// Keep cases fixture-root-local so location normalization remains meaningful. The line/character
// coordinates are LSP coordinates, not byte offsets.
//
// Sample language constructs and library usage without selecting for either server's results:
// IDE operations, std collections and adapters, closure and pattern inference, macro inputs and
// generated items, and builtin derives. Hover and inlay queries cover std APIs because navigation
// into the sysroot cannot be normalized to a fixture-relative location. Local navigation also
// follows values inferred through those APIs, such as a VfsPath obtained from a map and cloned.
const RUST_ANALYZER_CASES_TEXT: &str = r#"
[textDocument/references(includeDeclaration=true)]
crates/ide/src/child_modules.rs:18:14 # references/function: child_modules
crates/ide/src/call_hierarchy.rs:25:11 # references/type: CallHierarchyConfig
crates/ide/src/call_hierarchy.rs:20:8 # references/field: CallItem.target
crates/ide/src/call_hierarchy.rs:21:8 # references/field: CallItem.ranges
crates/ide/src/call_hierarchy.rs:43:14 # references/function: incoming_calls
crates/ide/src/call_hierarchy.rs:52:12 # references/local: calls
crates/ide/src/hover.rs:35:11 # references/type: HoverConfig
crates/ide/src/navigation_target.rs:30:11 # references/type: NavigationTarget
crates/ide/src/navigation_target.rs:116:10 # references/trait: TryToNav
crates/ide/src/references.rs:47:11 # references/type: ReferenceSearchResult
crates/ide/src/references.rs:63:11 # references/type: Declaration
crates/ide/src/references.rs:51:8 # references/field: ReferenceSearchResult.declaration
crates/ide/src/references.rs:58:8 # references/field: ReferenceSearchResult.references
crates/ide/src/references.rs:91:11 # references/config: FindAllRefsConfig
crates/ide/src/references.rs:123:14 # references/function: find_all_refs
crates/ide/src/references.rs:148:16 # references/helper-call: retain_adt_literal_usages
crates/ide/src/references.rs:190:23 # references/helper-call: handle_control_flow_keywords
crates/ide/src/references.rs:230:14 # references/function: find_defs
crates/stdx/src/non_empty_vec.rs:3:11 # references/generic-type: NonEmptyVec
crates/vfs/src/file_set.rs:14:11 # references/derived-type: FileSet
crates/stdx/src/macros.rs:20:13 # references/declarative-macro: format_to_acc
crates/syntax/src/ast/expr_ext.rs:60:27 # references/macro-binding: match_ast block
crates/syntax/src/ast/make.rs:344:51 # references/closure-local: formatting accumulator
crates/stdx/src/lib.rs:108:12 # references/captured-local: snake-case words
crates/vfs/src/file_set.rs:180:20 # references/destructured-local: enumerated paths
crates/stdx/src/panic_context.rs:43:15 # references/macro-generated-static: CTX

[textDocument/references(includeDeclaration=false)]
crates/ide/src/child_modules.rs:20:8 # references/local-no-decl: source_file
crates/ide/src/call_hierarchy.rs:52:12 # references/local-no-decl: calls
crates/ide/src/lib.rs:112:17 # references/config-no-decl: FindAllRefsConfig
crates/ide/src/navigation_target.rs:136:11 # references/method-no-decl: focus_or_full_range
crates/ide/src/references.rs:92:8 # references/field-no-decl: search_scope
crates/syntax/src/ast/expr_ext.rs:61:24 # references/macro-binding-no-decl: match_ast if
crates/vfs/src/file_set.rs:30:16 # references/inferred-local-no-decl: cloned path
crates/stdx/src/lib.rs:214:19 # references/while-let-no-decl: string offset
crates/stdx/src/panic_context.rs:45:14 # references/closure-param-no-decl: RefCell

[textDocument/definition]
crates/ide/src/call_hierarchy.rs:36:4 # definition/qualified-call: goto_definition
crates/ide/src/call_hierarchy.rs:39:9 # definition/config-constructor: GotoDefinitionConfig
crates/ide/src/call_hierarchy.rs:78:20 # definition/trait-method-call: try_to_nav
crates/ide/src/child_modules.rs:22:17 # definition/helper-call: find_node_at_offset
crates/ide/src/child_modules.rs:30:48 # definition/associated-function: from_module_to_decl
crates/ide/src/child_modules.rs:54:68 # definition/method-call: focus_or_full_range
crates/ide/src/goto_implementation.rs:83:28 # definition/helper-call: impls_for_trait_item
crates/ide/src/goto_definition.rs:147:16 # definition/helper-call: try_lookup_include_path
crates/ide/src/call_hierarchy.rs:107:8 # definition/enum-variant-import: SyntaxKind::IDENT
crates/hir-def/src/item_scope.rs:769:12 # definition/body-macro: format_to
crates/hir-def/src/item_tree/pretty.rs:64:8 # definition/body-macro: wln
crates/ide/src/lib.rs:91:21 # definition/reexport: GotoDefinitionConfig
crates/ide/src/lib.rs:95:21 # definition/reexport: HoverConfig
crates/ide/src/lib.rs:111:24 # definition/reexport: NavigationTarget
crates/ide/src/lib.rs:112:17 # definition/reexport: FindAllRefsConfig
crates/ide/src/references.rs:148:16 # definition/helper-call: retain_adt_literal_usages
crates/ide/src/references.rs:178:41 # definition/method-call: focus_or_full_range-extra-ref
crates/ide/src/references.rs:190:23 # definition/helper-call: handle_control_flow_keywords
crates/ide/src/references.rs:220:17 # definition/helper-call: find_defs
crates/ide/src/references.rs:245:12 # definition/glob-enum-variant: SyntaxKind::IDENT
crates/vfs/src/file_set.rs:31:13 # definition/inferred-receiver: cloned VfsPath
crates/vfs/src/file_set.rs:115:22 # definition/indexed-receiver: FileSet
crates/vfs/src/file_set.rs:183:22 # definition/iterator-item: VfsPath
crates/vfs/src/file_set.rs:32:29 # definition/shadowed-param: anchored path
crates/syntax/src/ast/expr_ext.rs:60:51 # definition/macro-local-use: match_ast block
crates/syntax/src/ast/expr_ext.rs:60:17 # definition/macro-type-argument: BlockExpr
crates/tt/src/lib.rs:126:20 # definition/macro-type-argument: Punct
crates/syntax/src/ast/make.rs:344:90 # definition/macro-expression-argument: attr
crates/stdx/src/panic_context.rs:45:4 # definition/macro-generated-static: CTX
crates/stdx/src/variance.rs:141:44 # definition/macro-generated-type: PhantomCovariant
crates/hir-ty/src/diagnostics/match_check.rs:429:42 # definition/macro-generated-method: variance marker
crates/tt/src/iter.rs:77:19 # definition/try-call: TtIter.expect_leaf
crates/stdx/src/lib.rs:135:23 # definition/generic-closure-call: change_case

[textDocument/typeDefinition]
crates/ide/src/call_hierarchy.rs:52:12 # type_definition/local: calls
crates/ide/src/child_modules.rs:20:8 # type_definition/local: source_file
crates/ide/src/call_hierarchy.rs:20:17 # type_definition/field: NavigationTarget
crates/ide/src/call_hierarchy.rs:21:21 # type_definition/field: FileRange
crates/ide/src/goto_implementation.rs:36:16 # type_definition/local: original_token
crates/ide/src/hover.rs:120:17 # type_definition/field: Markup
crates/ide/src/hover.rs:121:22 # type_definition/field: HoverAction
crates/ide/src/navigation_target.rs:48:15 # type_definition/field: Symbol
crates/ide/src/references.rs:129:8 # type_definition/local: syntax
crates/ide/src/references.rs:163:16 # type_definition/local: declaration
crates/ide/src/references.rs:125:15 # type_definition/param: FilePosition
crates/vfs/src/file_set.rs:30:16 # type_definition/try-and-clone: VfsPath
crates/vfs/src/file_set.rs:32:12 # type_definition/try-result: VfsPath
crates/vfs/src/file_set.rs:181:20 # type_definition/nested-iterator-item: VfsPath
crates/vfs/src/file_set.rs:113:13 # type_definition/opaque-iterator-item: FileId
crates/vfs/src/file_set.rs:113:22 # type_definition/opaque-iterator-reference: VfsPath
crates/syntax/src/ast/expr_ext.rs:60:27 # type_definition/macro-binding: BlockExpr
crates/syntax/src/ast/make.rs:344:56 # type_definition/closure-param: Attr
crates/vfs/src/vfs_path.rs:105:16 # type_definition/derived-clone: VfsPath
crates/stdx/src/process.rs:66:12 # type_definition/deref-wrapper: JodChild
crates/tt/src/iter.rs:57:45 # type_definition/match-binding: TtIter
crates/tt/src/iter.rs:41:45 # type_definition/nested-pattern: Punct

[textDocument/implementation]
crates/ide/src/call_hierarchy.rs:160:7 # implementation/helper-type: CallLocations
crates/ide/src/hover.rs:79:9 # implementation/enum: HoverAction
crates/ide/src/navigation_target.rs:30:11 # implementation/type: NavigationTarget
crates/ide/src/navigation_target.rs:112:17 # implementation/trait: ToNav
crates/ide/src/navigation_target.rs:116:10 # implementation/trait: TryToNav
crates/ide/src/navigation_target.rs:117:7 # implementation/trait-method: try_to_nav
crates/ide/src/navigation_target.rs:135:9 # implementation/inherent-impl: NavigationTarget
crates/ide/src/navigation_target.rs:298:7 # implementation/trait-impl: TryToNav
crates/vfs/src/file_set.rs:14:11 # implementation/derived-traits: FileSet
crates/vfs/src/vfs_path.rs:12:11 # implementation/derived-traits: VfsPath
crates/tt/src/iter.rs:221:9 # implementation/derived-enum: TtElement
crates/stdx/src/variance.rs:228:10 # implementation/macro-generated-impls: Variance
crates/syntax/src/ast.rs:88:11 # implementation/generic-derived-type: AstChildren
crates/stdx/src/anymap.rs:32:11 # implementation/default-and-hasher: TypeIdHasher

[textDocument/prepareRename]
crates/ide/src/call_hierarchy.rs:25:11 # prepare_rename/type: CallHierarchyConfig
crates/ide/src/call_hierarchy.rs:20:8 # prepare_rename/field: target
crates/ide/src/call_hierarchy.rs:43:14 # prepare_rename/function: incoming_calls
crates/ide/src/call_hierarchy.rs:52:12 # prepare_rename/local: calls
crates/ide/src/goto_implementation.rs:121:3 # prepare_rename/function: impls_for_trait_item
crates/ide/src/hover.rs:35:11 # prepare_rename/type: HoverConfig
crates/ide/src/navigation_target.rs:30:11 # prepare_rename/type: NavigationTarget
crates/ide/src/navigation_target.rs:116:10 # prepare_rename/trait: TryToNav
crates/ide/src/references.rs:92:8 # prepare_rename/field: search_scope
crates/ide/src/references.rs:123:14 # prepare_rename/function: find_all_refs
crates/syntax/src/ast/expr_ext.rs:60:27 # prepare_rename/macro-binding: match_ast block
crates/syntax/src/ast/make.rs:344:51 # prepare_rename/closure-local: formatting accumulator
crates/vfs/src/file_set.rs:180:20 # prepare_rename/destructured-local: enumerated paths
crates/stdx/src/lib.rs:108:12 # prepare_rename/captured-local: snake-case words

[textDocument/rename]
crates/ide/src/call_hierarchy.rs:25:11 -> RenamedCallHierarchyConfig # rename/type: CallHierarchyConfig
crates/ide/src/call_hierarchy.rs:20:8 -> renamed_target # rename/field: target
crates/ide/src/call_hierarchy.rs:43:14 -> renamed_incoming_calls # rename/function: incoming_calls
crates/ide/src/call_hierarchy.rs:52:12 -> renamed_calls # rename/local: calls
crates/ide/src/goto_implementation.rs:121:3 -> renamed_impls_for_trait_item # rename/function: impls_for_trait_item
crates/ide/src/hover.rs:35:11 -> RenamedHoverConfig # rename/type: HoverConfig
crates/ide/src/navigation_target.rs:30:11 -> RenamedNavigationTarget # rename/type: NavigationTarget
crates/ide/src/navigation_target.rs:116:10 -> RenamedTryToNav # rename/trait: TryToNav
crates/ide/src/references.rs:92:8 -> renamed_search_scope # rename/field: search_scope
crates/ide/src/references.rs:123:14 -> renamed_find_all_refs # rename/function: find_all_refs
crates/syntax/src/ast/expr_ext.rs:60:27 -> block_node # rename/macro-binding: match_ast block
crates/syntax/src/ast/make.rs:344:51 -> formatted_attrs # rename/closure-local: formatting accumulator
crates/vfs/src/file_set.rs:180:20 -> root_paths # rename/destructured-local: enumerated paths
crates/stdx/src/lib.rs:108:12 -> snake_words # rename/captured-local: snake-case words

[textDocument/documentHighlight]
crates/ide/src/call_hierarchy.rs:43:14 # document_highlight/function: incoming_calls
crates/ide/src/call_hierarchy.rs:87:16 # document_highlight/local: calls
crates/ide/src/child_modules.rs:20:8 # document_highlight/local: source_file
crates/ide/src/goto_implementation.rs:32:8 # document_highlight/local: original_token
crates/ide/src/hover.rs:130:14 # document_highlight/function: hover
crates/ide/src/navigation_target.rs:116:10 # document_highlight/trait: TryToNav
crates/ide/src/navigation_target.rs:136:11 # document_highlight/method: focus_or_full_range
crates/ide/src/references.rs:194:25 # document_highlight/local: syntax
crates/ide/src/references.rs:123:14 # document_highlight/function: find_all_refs
crates/ide/src/references.rs:230:14 # document_highlight/function: find_defs
crates/syntax/src/ast/expr_ext.rs:60:27 # document_highlight/macro-binding: match_ast block
crates/stdx/src/panic_context.rs:45:4 # document_highlight/macro-generated-static: CTX
crates/stdx/src/lib.rs:108:12 # document_highlight/captured-local: snake-case words
crates/vfs/src/file_set.rs:180:20 # document_highlight/destructured-local: enumerated paths
crates/vfs/src/file_set.rs:32:12 # document_highlight/shadowed-local: joined path
crates/stdx/src/process.rs:34:20 # document_highlight/branch-inferred-local: output buffer

[textDocument/documentSymbol]
crates/ide/src/call_hierarchy.rs # document_symbol/file: call_hierarchy
crates/ide/src/child_modules.rs # document_symbol/file: child_modules
crates/ide/src/goto_definition.rs # document_symbol/file: goto_definition
crates/ide/src/goto_implementation.rs # document_symbol/file: goto_implementation
crates/ide/src/hover.rs # document_symbol/file: hover
crates/ide/src/lib.rs # document_symbol/file: lib
crates/ide/src/navigation_target.rs # document_symbol/file: navigation_target
crates/ide/src/references.rs # document_symbol/file: references
crates/stdx/src/variance.rs # document_symbol/file: macro-generated-variance
crates/stdx/src/panic_context.rs # document_symbol/file: thread-local-and-nested-functions
crates/stdx/src/non_empty_vec.rs # document_symbol/file: generic-non-empty-vec
crates/vfs/src/file_set.rs # document_symbol/file: derived-file-sets
crates/syntax/src/ast.rs # document_symbol/file: generic-ast-traits

[workspace/symbol]
child_modules # workspace_symbol/function: child_modules
CallItem # workspace_symbol/type: CallItem
CallHierarchyConfig # workspace_symbol/type: CallHierarchyConfig
FilePosition # workspace_symbol/type: FilePosition
find_all_refs # workspace_symbol/function: find_all_refs
GotoDefinitionConfig # workspace_symbol/type: GotoDefinitionConfig
GotoImplementationConfig # workspace_symbol/type: GotoImplementationConfig
HoverConfig # workspace_symbol/type: HoverConfig
NavigationTarget # workspace_symbol/type: NavigationTarget
ReferenceSearchResult # workspace_symbol/type: ReferenceSearchResult
SearchScope # workspace_symbol/type: SearchScope
TryToNav # workspace_symbol/trait: TryToNav
NonEmptyVec # workspace_symbol/generic-type: NonEmptyVec
FileSet # workspace_symbol/derived-type: FileSet
PhantomCovariant # workspace_symbol/macro-generated-type: PhantomCovariant
format_to_acc # workspace_symbol/declarative-macro: format_to_acc
TypeIdHasher # workspace_symbol/derived-default: TypeIdHasher

[textDocument/inlayHint]
crates/ide/src/call_hierarchy.rs # inlay_hint/file: call_hierarchy
crates/ide/src/child_modules.rs # inlay_hint/file: child_modules
crates/ide/src/goto_definition.rs # inlay_hint/file: goto_definition
crates/ide/src/goto_implementation.rs # inlay_hint/file: goto_implementation
crates/ide/src/hover.rs # inlay_hint/file: hover
crates/ide/src/lib.rs # inlay_hint/file: lib
crates/ide/src/navigation_target.rs # inlay_hint/file: navigation_target
crates/ide/src/references.rs # inlay_hint/file: references
crates/stdx/src/lib.rs # inlay_hint/file: std-iterators-and-closures
crates/stdx/src/non_empty_vec.rs # inlay_hint/file: generic-vec-and-option
crates/stdx/src/panic_context.rs # inlay_hint/file: thread-local-and-closures
crates/stdx/src/process.rs # inlay_hint/file: io-and-deref
crates/vfs/src/file_set.rs # inlay_hint/file: collections-and-derives
crates/tt/src/iter.rs # inlay_hint/file: token-patterns-and-iterators

[textDocument/hover]
crates/ide/src/call_hierarchy.rs:19:11 # hover/type: CallItem
crates/ide/src/call_hierarchy.rs:20:8 # hover/field: target
crates/ide/src/call_hierarchy.rs:52:12 # hover/local: calls
crates/ide/src/call_hierarchy.rs:98:14 # hover/function: outgoing_calls
crates/ide/src/child_modules.rs:18:14 # hover/function: child_modules
crates/ide/src/goto_implementation.rs:121:3 # hover/helper: impls_for_trait_item
crates/ide/src/hover.rs:35:11 # hover/config: HoverConfig
crates/ide/src/hover.rs:79:9 # hover/enum: HoverAction
crates/ide/src/hover.rs:81:4 # hover/enum-variant: Implementation
crates/ide/src/hover.rs:119:11 # hover/type: HoverResult
crates/ide/src/hover.rs:130:14 # hover/function: hover
crates/ide/src/hover.rs:392:38 # hover/body-macro: format
crates/ide/src/hover.rs:158:3 # hover/helper: hover_offset
crates/ide/src/inlay_hints.rs:816:27 # hover/enum-variant: Err
crates/ide/src/lib.rs:111:24 # hover/reexport: NavigationTarget
crates/ide/src/navigation_target.rs:30:11 # hover/type: NavigationTarget
crates/ide/src/navigation_target.rs:116:10 # hover/trait: TryToNav
crates/ide/src/navigation_target.rs:83:8 # hover/enum-variant: Ok
crates/ide/src/references.rs:171:20 # hover/enum-variant: Some
crates/ide/src/references.rs:172:20 # hover/enum-variant: None
crates/ide/src/references.rs:92:8 # hover/field: search_scope
crates/stdx/src/lib.rs:108:12 # hover/inferred-collection: Vec<String>
crates/stdx/src/lib.rs:111:10 # hover/std-str: trim_start_matches
crates/stdx/src/lib.rs:120:8 # hover/inferred-iterator-item: split
crates/stdx/src/lib.rs:128:12 # hover/inferred-iterator-item: chars
crates/stdx/src/lib.rs:135:16 # hover/std-extend: generic iterator
crates/stdx/src/lib.rs:135:23 # hover/closure-bound: Fn(char) -> I
crates/stdx/src/lib.rs:176:43 # hover/closure-destructure: fold accumulator
crates/stdx/src/lib.rs:176:76 # hover/inferred-closure-param: mapped String
crates/stdx/src/lib.rs:181:24 # hover/try-in-closure: char
crates/stdx/src/lib.rs:205:45 # hover/reference-pattern: filter closure
crates/stdx/src/lib.rs:215:12 # hover/std-string: replace_range
crates/stdx/src/lib.rs:229:19 # hover/inferred-closure-param: split_inclusive
crates/stdx/src/lib.rs:264:21 # hover/std-cow: Borrowed
crates/stdx/src/lib.rs:266:46 # hover/std-conversion: String into Cow
crates/stdx/src/lib.rs:276:39 # hover/generic-closure-param: slice item
crates/stdx/src/lib.rs:276:22 # hover/std-slice: partition_point
crates/stdx/src/non_empty_vec.rs:16:18 # hover/std-autoderef: Vec to slice
crates/stdx/src/non_empty_vec.rs:16:29 # hover/std-option: mutable generic reference
crates/stdx/src/non_empty_vec.rs:36:18 # hover/std-vec: generic pop
crates/stdx/src/process.rs:34:20 # hover/branch-inference: mutable Vec
crates/stdx/src/process.rs:36:20 # hover/std-iterator: Drain
crates/stdx/src/process.rs:40:32 # hover/std-string: from_utf8_lossy
crates/stdx/src/process.rs:40:16 # hover/std-cow-deref: lines item
crates/stdx/src/process.rs:74:23 # hover/std-deref-method: Child wait
crates/stdx/src/process.rs:68:14 # hover/std-deref-field: Child stdout
crates/stdx/src/panic_context.rs:20:12 # hover/std-trait-object: panic hook
crates/stdx/src/panic_context.rs:35:13 # hover/std-sync: Once call_once
crates/stdx/src/panic_context.rs:45:30 # hover/std-cell: RefCell borrow_mut
crates/stdx/src/anymap.rs:125:23 # hover/std-hash-map: entry through alias
crates/stdx/src/anymap.rs:44:38 # hover/expected-type: array conversion
crates/vfs/src/file_set.rs:180:20 # hover/iterator-destructure: nested Vec
crates/vfs/src/file_set.rs:188:31 # hover/mutable-tuple-pattern: dedup_by
crates/syntax/src/ast/expr_ext.rs:60:27 # hover/macro-binding: match_ast block
crates/syntax/src/ast/make.rs:344:77 # hover/macro-expression-argument: accumulator
crates/stdx/src/panic_context.rs:45:4 # hover/macro-generated-static: CTX
crates/stdx/src/variance.rs:141:44 # hover/macro-generated-type: PhantomCovariant
crates/hir-ty/src/diagnostics/match_check.rs:429:42 # hover/macro-generated-method: variance marker
crates/vfs/src/vfs_path.rs:105:30 # hover/derived-clone: VfsPath
crates/vfs/src/vfs_path.rs:105:16 # hover/derived-clone-result: VfsPath
crates/vfs/src/file_set.rs:112:36 # hover/derived-default: FileSet
crates/vfs/src/file_set.rs:112:16 # hover/derived-clone-repeat: Vec<FileSet>
crates/vfs/src/file_set.rs:104:30 # hover/derived-default: FileSetConfigBuilder
crates/tt/src/lib.rs:735:45 # hover/derived-enum-clone: TtElement
crates/tt/src/lib.rs:738:19 # hover/derived-equality-operand: Spacing
crates/vfs/src/lib.rs:64:61 # hover/builtin-derive: FileId Hash
crates/syntax/src/ast.rs:102:31 # hover/associated-function-item: generic AstNode
"#;

// This is intentionally a small preservation corpus rather than a second copy of the clean
// benchmark. Coordinates below refer to the pinned saved checkout. The query model shifts them
// past the harmless comment inserted by the dirty fixture.
//
// The selected cases cover current bodies, unchanged declaration headers, use items, and
// document-wide reads. New module-level semantics and saved-only global operations are outside the
// dirty-buffer contract and therefore do not contribute noise to this report.
const RUST_ANALYZER_DIRTY_CASES_TEXT: &str = r#"
[textDocument/definition]
crates/ide/src/call_hierarchy.rs:36:4 # dirty/definition/body-qualified-call
crates/ide/src/child_modules.rs:30:48 # dirty/definition/body-associated-function
crates/ide/src/goto_definition.rs:147:16 # dirty/definition/body-helper-call
crates/ide/src/goto_implementation.rs:83:28 # dirty/definition/body-implementation-helper
crates/ide/src/lib.rs:91:21 # dirty/definition/unchanged-reexport
crates/ide/src/references.rs:148:16 # dirty/definition/body-reference-helper
crates/vfs/src/file_set.rs:31:13 # dirty/definition/cloned-path-method
crates/syntax/src/ast/expr_ext.rs:60:51 # dirty/definition/macro-local-use
crates/stdx/src/panic_context.rs:45:4 # dirty/definition/thread-local-static

[textDocument/typeDefinition]
crates/ide/src/call_hierarchy.rs:52:12 # dirty/type-definition/current-local
crates/ide/src/call_hierarchy.rs:20:17 # dirty/type-definition/unchanged-field-header
crates/ide/src/navigation_target.rs:48:15 # dirty/type-definition/unchanged-struct-header
crates/ide/src/references.rs:125:15 # dirty/type-definition/unchanged-parameter-header
crates/vfs/src/file_set.rs:30:16 # dirty/type-definition/try-and-clone
crates/syntax/src/ast/expr_ext.rs:60:27 # dirty/type-definition/macro-binding

[textDocument/documentHighlight]
crates/ide/src/call_hierarchy.rs:87:16 # dirty/document-highlight/current-local
crates/ide/src/call_hierarchy.rs:43:14 # dirty/document-highlight/unchanged-function-header
crates/ide/src/navigation_target.rs:136:11 # dirty/document-highlight/unchanged-method-header
crates/ide/src/references.rs:194:25 # dirty/document-highlight/current-body-symbol
crates/syntax/src/ast/expr_ext.rs:60:27 # dirty/document-highlight/macro-binding

[textDocument/documentSymbol]
crates/ide/src/call_hierarchy.rs # dirty/document-symbol/call-hierarchy
crates/ide/src/lib.rs # dirty/document-symbol/lib

[textDocument/inlayHint]
crates/ide/src/child_modules.rs # dirty/inlay-hint/child-modules
crates/ide/src/hover.rs # dirty/inlay-hint/hover
crates/stdx/src/panic_context.rs # dirty/inlay-hint/thread-local-and-closures
crates/vfs/src/file_set.rs # dirty/inlay-hint/collections-and-derives

[textDocument/hover]
crates/ide/src/call_hierarchy.rs:52:12 # dirty/hover/current-local
crates/ide/src/call_hierarchy.rs:19:11 # dirty/hover/unchanged-type-header
crates/ide/src/call_hierarchy.rs:20:8 # dirty/hover/unchanged-field-header
crates/ide/src/hover.rs:130:14 # dirty/hover/unchanged-function-header
crates/ide/src/lib.rs:111:24 # dirty/hover/unchanged-reexport
crates/stdx/src/lib.rs:108:12 # dirty/hover/inferred-collection
crates/stdx/src/process.rs:74:23 # dirty/hover/deref-method
crates/stdx/src/panic_context.rs:45:30 # dirty/hover/refcell-method
crates/syntax/src/ast/expr_ext.rs:60:27 # dirty/hover/macro-binding
crates/vfs/src/vfs_path.rs:105:30 # dirty/hover/derived-clone
crates/vfs/src/file_set.rs:112:36 # dirty/hover/derived-default
"#;

#[cfg(test)]
mod tests {
    use super::{QueryKind, QueryTarget, rust_analyzer_dirty_cases};
    use crate::compare_lsp::config::{DIRTY_EDITOR_PREFIX, DIRTY_EDITOR_PREFIX_LINE_COUNT};

    #[test]
    fn dirty_queries_are_shifted_past_the_unsaved_prefix() {
        assert_eq!(
            u32::try_from(DIRTY_EDITOR_PREFIX.lines().count())
                .expect("dirty editor prefix line count should fit u32"),
            DIRTY_EDITOR_PREFIX_LINE_COUNT,
        );
        let first = rust_analyzer_dirty_cases()
            .first()
            .expect("dirty query corpus should not be empty");
        let QueryTarget::Position { position, .. } = first.target() else {
            panic!("first dirty query should target a position");
        };

        assert_eq!(position.line(), 37);
        assert_eq!(position.character(), 4);
    }

    #[test]
    fn dirty_corpus_contains_only_current_document_reads() {
        assert!(rust_analyzer_dirty_cases().iter().all(|query| matches!(
            query.kind(),
            QueryKind::GotoDefinition
                | QueryKind::TypeDefinition
                | QueryKind::DocumentHighlight
                | QueryKind::DocumentSymbol
                | QueryKind::InlayHint
                | QueryKind::Hover
        )));
    }
}
