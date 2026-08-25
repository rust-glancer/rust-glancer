//! Request-local source edits for imports.
//!
//! Semantic search supplies a path that is known to resolve. This module owns only source
//! placement for completion and code actions: it reuses compatible direct `use` items when their
//! syntax is simple enough to edit safely, otherwise it inserts a new private import in the
//! innermost module containing the cursor.
//!
//! ```text
//! use std::collections::BTreeMap;
//!
//! fn load() {
//!     let _: HashM$0;
//! }
//!
//! // Accepting `HashMap` can coalesce the existing import into:
//! use std::collections::{BTreeMap, HashMap};
//! ```
//!
//! Attribute-bearing, public, renamed, globbed, multiline, or structurally nested imports are
//! left untouched. In those cases a separate private `use` is safer than rewriting syntax whose
//! formatting or visibility may carry intent.

use rg_ir_model::Path;
use rg_parse::{Span, TextSpan};
use rg_syntax::{
    AstNode as _, SourceFile, TextRange, TextSize, ast, ast::HasModuleItem as _, ast::HasName as _,
};

use crate::{
    model::{CodeActionEdit, CompletionEdit},
    query::completion::CompletionSyntaxContext,
};

/// Selects the syntax/offset mapping used by the shared import planner.
///
/// Completion edits come from a speculative tree and must map back to the request buffer. Code
/// actions parse the request buffer directly, so their syntax ranges are already original ranges.
#[derive(Clone, Copy)]
enum ImportEditSyntax<'syntax, 'source> {
    Completion(&'syntax CompletionSyntaxContext<'source>),
    Direct {
        source: &'source str,
        file: &'syntax SourceFile,
        offset: u32,
    },
}

impl<'syntax, 'source> ImportEditSyntax<'syntax, 'source> {
    fn source(&self) -> &'source str {
        match self {
            Self::Completion(syntax) => syntax.source(),
            Self::Direct { source, .. } => source,
        }
    }

    fn file_tree(&self) -> &SourceFile {
        match self {
            Self::Completion(syntax) => syntax.speculative_file_tree(),
            Self::Direct { file, .. } => file,
        }
    }

    fn syntax_offset(&self) -> u32 {
        match self {
            Self::Completion(syntax) => syntax.speculative_offset(),
            Self::Direct { offset, .. } => *offset,
        }
    }

    /// Map a range in the selected syntax tree back to the editor buffer.
    fn original_span(&self, range: TextRange) -> Option<Span> {
        match self {
            Self::Completion(syntax) => syntax.original_span(range),
            Self::Direct { .. } => Some(Span::from_text_range(range)),
        }
    }

    fn original_offset(&self, offset: u32) -> Option<u32> {
        self.original_span(TextRange::empty(TextSize::from(offset)))
            .map(|span| span.text.start)
    }
}

/// Whether a requested import already exists, needs one edit, or cannot be placed safely.
pub(crate) enum ImportEditPlan {
    /// The exact path already appears in a `use` tree; callers may still apply a non-import edit.
    AlreadyImported,
    /// One source edit adds the requested path without changing another import's meaning.
    Edit(CodeActionEdit),
    /// The name is occupied or the surrounding syntax is not safe to edit.
    Unavailable,
}

/// Plans one conservative module-level import for completion or ordinary current syntax.
///
/// Completion also supplies its primary identifier replacement as a protected span. Import edits
/// may be coalesced or inserted only when they do not overlap that replacement.
pub(crate) struct ImportEditPlanner<'syntax, 'source> {
    syntax: ImportEditSyntax<'syntax, 'source>,
    protected_span: Option<Span>,
}

impl<'syntax, 'source> ImportEditPlanner<'syntax, 'source> {
    /// Plan against completion's speculative tree and map edits back to the editor buffer.
    pub(crate) fn for_completion(
        syntax: &'syntax CompletionSyntaxContext<'source>,
        primary_edit: CompletionEdit,
    ) -> Self {
        Self {
            syntax: ImportEditSyntax::Completion(syntax),
            protected_span: Some(primary_edit.replace),
        }
    }

    /// Plan directly against an ordinary parse of the captured editor buffer.
    pub(crate) fn for_source(source: &'source str, file: &'syntax SourceFile, offset: u32) -> Self {
        Self {
            syntax: ImportEditSyntax::Direct {
                source,
                file,
                offset,
            },
            protected_span: None,
        }
    }

    /// Decide whether the requested path needs an edit and where that edit can go.
    ///
    /// For `std::collections::HashMap`, this method checks, in order:
    ///
    /// 1. Is that exact path already imported? Return `AlreadyImported`.
    /// 2. Does another import already introduce the name `HashMap`? Return `Unavailable` rather
    ///    than silently changing which item the name means.
    /// 3. Can a simple import with the same prefix become a group such as
    ///    `std::collections::{BTreeMap, HashMap}`? Return that edit.
    /// 4. Otherwise, can a new private `use std::collections::HashMap;` be inserted in this module?
    pub(crate) fn plan(&self, path: &Path, rendered_path: &str) -> ImportEditPlan {
        let items = self.containing_items();
        let use_items: Vec<ast::Use> = items
            .iter()
            .filter_map(|item| match item {
                ast::Item::Use(use_item) => Some(use_item.clone()),
                _ => None,
            })
            .collect();

        // 1. An exact duplicate needs no source edit. This is different from a conflict because a
        // qualified-path action may still remove its qualifier and reuse the existing import.
        if use_items
            .iter()
            .any(|use_item| self.already_imports(use_item, path))
        {
            return ImportEditPlan::AlreadyImported;
        }

        // 2. A different import with the same final name would make the new import ambiguous or
        // invalid. Renames count by the name they introduce at this module.
        let Some(imported_name) = path.segments().last().map(|name| name.as_str()) else {
            return ImportEditPlan::Unavailable;
        };
        if use_items.iter().any(|use_item| {
            use_item
                .use_tree()
                .is_some_and(|tree| Self::use_tree_imports_name(tree, "", imported_name))
        }) {
            return ImportEditPlan::Unavailable;
        }

        // 3. Prefer extending a simple, one-line import with the same prefix.
        for use_item in &use_items {
            let Some(edit) = self.coalesced_edit(use_item, rendered_path) else {
                continue;
            };
            return if self.does_not_overlap_protected(edit.replace) {
                ImportEditPlan::Edit(edit)
            } else {
                ImportEditPlan::Unavailable
            };
        }

        // 4. If no existing import is suitable, insert a separate private `use`.
        let Some(edit) = self.insertion_edit(&items, &use_items, rendered_path) else {
            return ImportEditPlan::Unavailable;
        };
        if self.does_not_overlap_protected(edit.replace) {
            ImportEditPlan::Edit(edit)
        } else {
            ImportEditPlan::Unavailable
        }
    }

    /// Return the item list that should receive the new import.
    ///
    /// A name used inside `mod nested { ... }` needs a `use` inside `nested`, not at the file root.
    /// Walking all inline module bodies and choosing the smallest one containing the cursor finds
    /// that innermost module. If none contains it, the file's direct items are used.
    fn containing_items(&self) -> Vec<ast::Item> {
        let cursor = TextSize::from(self.syntax.syntax_offset());
        let item_list = self
            .syntax
            .file_tree()
            .syntax()
            .descendants()
            .filter_map(ast::Module::cast)
            .filter_map(|module| module.item_list())
            .filter(|list| {
                let range = list.syntax().text_range();
                range.start() <= cursor && cursor <= range.end()
            })
            .min_by_key(|list| list.syntax().text_range().len());

        item_list.map_or_else(
            || self.syntax.file_tree().items().collect(),
            |list| list.items().collect(),
        )
    }

    /// Check whether one `use` tree already imports the complete requested path.
    ///
    /// The recursive walk recognizes `use std::{collections::HashMap}` as the path
    /// `std::collections::HashMap`, even though the planner deliberately will not rewrite every
    /// nested tree shape.
    fn already_imports(&self, use_item: &ast::Use, path: &Path) -> bool {
        let Some(use_tree) = use_item.use_tree() else {
            return false;
        };
        Self::use_tree_imports_path(use_tree, "", &Self::compact(&path.to_string()))
    }

    /// Try to extend a simple import that has the same path prefix.
    ///
    /// `use std::collections::BTreeMap` can become
    /// `use std::collections::{BTreeMap, HashMap}`. Attribute-bearing, public, renamed, globbed,
    /// multiline, or nested imports are left unchanged because rewriting them could alter
    /// formatting or source intent; the caller may still insert a separate private `use`.
    fn coalesced_edit(&self, use_item: &ast::Use, rendered_path: &str) -> Option<CodeActionEdit> {
        let item_text = use_item.syntax().text().to_string();
        let trimmed_item = item_text.trim_start();
        if !(trimmed_item.starts_with("use ")
            || trimmed_item.starts_with("use\n")
            || trimmed_item.starts_with("use\r\n"))
        {
            return None;
        }
        let use_tree = use_item.use_tree()?;
        let tree_text = use_tree.syntax().text().to_string();
        if tree_text.contains('\n') {
            return None;
        }

        let (prefix, new_leaf) = rendered_path.rsplit_once("::")?;
        let compact_tree: String = tree_text
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect();
        let compact_prefix: String = prefix
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect();
        let grouped_prefix = format!("{compact_prefix}::{{");

        if compact_tree.starts_with(&grouped_prefix) && compact_tree.ends_with('}') {
            let inner = &compact_tree[grouped_prefix.len()..compact_tree.len() - 1];
            if inner.contains('{') || inner.contains('}') {
                return None;
            }
            let leaves: Vec<&str> = inner.split(',').filter(|leaf| !leaf.is_empty()).collect();
            if leaves.is_empty() || leaves.iter().any(|leaf| !Self::is_simple_use_leaf(leaf)) {
                return None;
            }

            let closing_brace = tree_text.rfind('}')?;
            let insertion = u32::from(use_tree.syntax().text_range().start())
                .checked_add(u32::try_from(closing_brace).ok()?)?;
            let insertion = self.syntax.original_offset(insertion)?;
            let new_text = if inner.ends_with(',') {
                format!(" {new_leaf},")
            } else {
                format!(", {new_leaf}")
            };
            return Some(CodeActionEdit {
                replace: Span {
                    text: TextSpan {
                        start: insertion,
                        end: insertion,
                    },
                },
                new_text,
            });
        }

        let existing_leaf = compact_tree.strip_prefix(&format!("{compact_prefix}::"))?;
        if !Self::is_simple_use_leaf(existing_leaf) {
            return None;
        }
        Some(CodeActionEdit {
            replace: self.syntax.original_span(use_tree.syntax().text_range())?,
            new_text: format!("{prefix}::{{{existing_leaf}, {new_leaf}}}"),
        })
    }

    /// Insert a separate `use` without moving or reformatting existing items.
    ///
    /// When imports already form clean lines, append after the last one with the same indentation.
    /// Otherwise insert before the first item in the selected module and leave one blank line.
    fn insertion_edit(
        &self,
        items: &[ast::Item],
        use_items: &[ast::Use],
        rendered_path: &str,
    ) -> Option<CodeActionEdit> {
        if let Some(last_use) = use_items.last() {
            let end = self
                .syntax
                .original_offset(u32::from(last_use.syntax().text_range().end()))?;
            let line_suffix = self
                .syntax
                .source()
                .get(usize::try_from(end).ok()?..)?
                .split_once('\n')
                .map_or_else(
                    || self.syntax.source().get(usize::try_from(end).ok()?..),
                    |(suffix, _)| Some(suffix),
                )?;
            if line_suffix.trim().is_empty() {
                let indent = Self::line_indent(
                    self.syntax.source(),
                    self.syntax
                        .original_offset(u32::from(last_use.syntax().text_range().start()))?,
                )?;
                return Some(CodeActionEdit {
                    replace: Span {
                        text: TextSpan { start: end, end },
                    },
                    new_text: format!("\n{indent}use {rendered_path};"),
                });
            }
        }

        let first_item = items.first()?;
        let start = self
            .syntax
            .original_offset(u32::from(first_item.syntax().text_range().start()))?;
        let indent = Self::line_indent(self.syntax.source(), start)?;
        Some(CodeActionEdit {
            replace: Span {
                text: TextSpan { start, end: start },
            },
            new_text: format!("use {rendered_path};\n\n{indent}"),
        })
    }

    fn does_not_overlap_protected(&self, additional: Span) -> bool {
        let Some(protected) = self.protected_span else {
            return true;
        };
        let primary = protected.text;
        let additional = additional.text;
        primary.end <= additional.start || additional.end <= primary.start
    }

    fn is_simple_use_leaf(leaf: &str) -> bool {
        let leaf = leaf.strip_prefix("r#").unwrap_or(leaf);
        !leaf.is_empty()
            && leaf
                .chars()
                .all(|character| character == '_' || character.is_ascii_alphanumeric())
    }

    /// Reconstruct complete leaves while walking a possibly-grouped `use` tree.
    ///
    /// A rename or glob does not count as an exact path import: it either introduces another name
    /// or does not say which concrete leaf is present.
    fn use_tree_imports_path(use_tree: ast::UseTree, prefix: &str, desired: &str) -> bool {
        let path = use_tree
            .path()
            .map(|path| Self::compact(&path.syntax().text().to_string()))
            .unwrap_or_default();
        let joined = match (prefix.is_empty(), path.is_empty()) {
            (true, _) => path,
            (_, true) => prefix.to_string(),
            (false, false) => format!("{prefix}::{path}"),
        };

        if let Some(list) = use_tree.use_tree_list() {
            return list
                .use_trees()
                .any(|child| Self::use_tree_imports_path(child, &joined, desired));
        }
        if use_tree.star_token().is_some() || use_tree.rename().is_some() {
            return false;
        }

        joined.strip_suffix("::self").unwrap_or(&joined) == desired
    }

    /// Check whether one `use` tree already introduces the proposed unqualified name.
    ///
    /// Both `use alpha::User` and `use alpha::Account as User` occupy `User`. A glob is ignored
    /// because its individual names are not written in the source tree.
    fn use_tree_imports_name(use_tree: ast::UseTree, prefix: &str, desired: &str) -> bool {
        let path = use_tree
            .path()
            .map(|path| Self::compact(&path.syntax().text().to_string()))
            .unwrap_or_default();
        let joined = match (prefix.is_empty(), path.is_empty()) {
            (true, _) => path,
            (_, true) => prefix.to_string(),
            (false, false) => format!("{prefix}::{path}"),
        };

        if let Some(list) = use_tree.use_tree_list() {
            return list
                .use_trees()
                .any(|child| Self::use_tree_imports_name(child, &joined, desired));
        }
        if use_tree.star_token().is_some() {
            return false;
        }
        if let Some(rename) = use_tree.rename() {
            return rename
                .name()
                .is_some_and(|name| Self::same_identifier(&name.text(), desired));
        }

        let imported = joined.strip_suffix("::self").map_or_else(
            || joined.rsplit("::").next().unwrap_or_default(),
            |parent| parent.rsplit("::").next().unwrap_or_default(),
        );
        Self::same_identifier(imported, desired)
    }

    fn same_identifier(left: &str, right: &str) -> bool {
        left.strip_prefix("r#").unwrap_or(left) == right.strip_prefix("r#").unwrap_or(right)
    }

    fn compact(text: &str) -> String {
        text.chars()
            .filter(|character| !character.is_whitespace())
            .collect()
    }

    /// Return the spaces or tabs before an item only when it starts its source line.
    fn line_indent(source: &str, offset: u32) -> Option<&str> {
        let offset = usize::try_from(offset).ok()?;
        let before = source.get(..offset)?;
        let line_start = before.rfind('\n').map_or(0, |newline| newline + 1);
        let indent = before.get(line_start..)?;
        indent
            .chars()
            .all(|character| matches!(character, ' ' | '\t'))
            .then_some(indent)
    }
}

#[cfg(test)]
mod tests {
    use rg_ir_model::Path;
    use rg_parse::{Span, TextSpan};

    use super::{CompletionSyntaxContext, ImportEditPlan, ImportEditPlanner};
    use crate::model::CompletionEdit;

    #[test]
    fn coalesces_plain_and_braced_use_trees() {
        let cases = [
            (
                "use std::collections::BTreeMap;\nfn main() { let _: HashM; }",
                "std::collections::{BTreeMap, HashMap}",
            ),
            (
                "use std::collections::{BTreeMap};\nfn main() { let _: HashM; }",
                ", HashMap",
            ),
            (
                "use\r\nstd::collections::BTreeMap;\r\nfn main() { let _: HashM; }",
                "std::collections::{BTreeMap, HashMap}",
            ),
        ];
        for (source, expected_edit) in cases {
            let offset = u32::try_from(source.find("HashM").expect("fixture should have cursor"))
                .expect("fixture offset should fit");
            let syntax = CompletionSyntaxContext::at(Some(source), offset + 5)
                .expect("completion syntax should parse");
            let planner = ImportEditPlanner::for_completion(&syntax, primary(offset));
            let ImportEditPlan::Edit(edit) =
                planner.plan(&hash_map_path(), "std::collections::HashMap")
            else {
                panic!("compatible import should produce an edit");
            };
            assert_eq!(edit.new_text, expected_edit);
        }
    }

    #[test]
    fn recognizes_nested_duplicate_use_tree() {
        let source = "use std::{collections::HashMap};\nfn main() { let _: HashM; }";
        let offset = u32::try_from(source.rfind("HashM").expect("fixture should have cursor"))
            .expect("fixture offset should fit");
        let syntax = CompletionSyntaxContext::at(Some(source), offset + 5)
            .expect("completion syntax should parse");
        let planner = ImportEditPlanner::for_completion(&syntax, primary(offset));
        assert!(matches!(
            planner.plan(&hash_map_path(), "std::collections::HashMap"),
            ImportEditPlan::AlreadyImported
        ));
    }

    #[test]
    fn rejects_an_import_that_would_reuse_an_occupied_name() {
        let source = "use crate::alpha::User;\nfn main() { let _: User; }";
        let offset = u32::try_from(source.rfind("User").expect("fixture should have cursor"))
            .expect("fixture offset should fit");
        let syntax = CompletionSyntaxContext::at(Some(source), offset + 4)
            .expect("fixture source should parse");
        let desired = Path::from_macro_path_text("crate::beta::User", None)
            .expect("fixture path should parse");
        let planner = ImportEditPlanner::for_source(source, syntax.speculative_file_tree(), offset);

        assert!(matches!(
            planner.plan(&desired, "crate::beta::User"),
            ImportEditPlan::Unavailable
        ));
    }

    #[test]
    fn keeps_attribute_bearing_imports_untouched() {
        let source = "#[cfg(any())]\nuse std::collections::BTreeMap;\nfn main() { let _: HashM; }";
        let offset = u32::try_from(source.find("HashM").expect("fixture should have cursor"))
            .expect("fixture offset should fit");
        let syntax = CompletionSyntaxContext::at(Some(source), offset + 5)
            .expect("fixture source should parse");
        let planner = ImportEditPlanner::for_source(source, syntax.speculative_file_tree(), offset);

        let ImportEditPlan::Edit(edit) =
            planner.plan(&hash_map_path(), "std::collections::HashMap")
        else {
            panic!("a separate import should remain safe");
        };
        assert_eq!(
            edit.replace.text.start,
            u32::try_from(
                source
                    .find(';')
                    .expect("fixture should contain an existing import")
                    + 1
            )
            .expect("fixture offset should fit")
        );
        assert_eq!(edit.new_text, "\nuse std::collections::HashMap;");
    }

    #[test]
    fn inserts_into_the_innermost_module_item_list() {
        let source = "mod nested {\n    fn main() { let _: HashM; }\n}\n";
        let offset = u32::try_from(source.find("HashM").expect("fixture should have cursor"))
            .expect("fixture offset should fit");
        let syntax = CompletionSyntaxContext::at(Some(source), offset + 5)
            .expect("completion syntax should parse");
        let planner = ImportEditPlanner::for_completion(&syntax, primary(offset));
        let ImportEditPlan::Edit(edit) =
            planner.plan(&hash_map_path(), "std::collections::HashMap")
        else {
            panic!("module should accept a new import");
        };
        assert_eq!(edit.new_text, "use std::collections::HashMap;\n\n    ");
        assert_eq!(
            edit.replace.text.start,
            u32::try_from(
                source
                    .find("fn main")
                    .expect("fixture should have function")
            )
            .expect("fixture offset should fit")
        );
    }

    #[test]
    fn maps_use_edits_after_short_and_long_completion_prefixes() {
        for prefix in ["H", "AnExtremelyLongCompletionPrefix"] {
            let source =
                format!("fn main() {{ let _: {prefix}; }}\nuse std::collections::BTreeMap;\n");
            let start = u32::try_from(source.find(prefix).expect("fixture should have cursor"))
                .expect("fixture offset should fit");
            let end = start + u32::try_from(prefix.len()).expect("fixture prefix should fit");
            let syntax = CompletionSyntaxContext::at(Some(&source), end)
                .expect("completion syntax should parse");
            let planner = ImportEditPlanner::for_completion(
                &syntax,
                CompletionEdit {
                    replace: Span {
                        text: TextSpan { start, end },
                    },
                },
            );
            let ImportEditPlan::Edit(edit) =
                planner.plan(&hash_map_path(), "std::collections::HashMap")
            else {
                panic!("following use item should accept a coalesced edit");
            };
            let tree_start = u32::try_from(
                source
                    .find("std::collections::BTreeMap")
                    .expect("fixture should contain use tree"),
            )
            .expect("fixture offset should fit");

            assert_eq!(edit.replace.text.start, tree_start, "{prefix}");
            assert_eq!(
                edit.replace.text.end,
                tree_start + u32::try_from("std::collections::BTreeMap".len()).expect("text fits"),
                "{prefix}"
            );
            assert_eq!(
                edit.new_text, "std::collections::{BTreeMap, HashMap}",
                "{prefix}"
            );
        }
    }

    fn primary(start: u32) -> CompletionEdit {
        CompletionEdit {
            replace: Span {
                text: TextSpan {
                    start,
                    end: start + 5,
                },
            },
        }
    }

    fn hash_map_path() -> Path {
        Path::from_macro_path_text("std::collections::HashMap", None)
            .expect("fixture path should parse")
    }
}
