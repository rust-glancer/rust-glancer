//! Request-local source edits for auto-import completion.
//!
//! The semantic search supplies a path that is known to resolve. This module owns only source
//! placement: it reuses compatible direct `use` items when their syntax is simple enough to edit
//! safely, otherwise it inserts a new private import in the innermost module containing the cursor.
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
use rg_syntax::{AstNode as _, Edition, SourceFile, TextSize, ast, ast::HasModuleItem as _};

use crate::model::{CompletionAdditionalEdit, CompletionEdit};

/// Plans the source-side half of accepting one auto-import completion.
///
/// The planner reparses only the request buffer, chooses the innermost inline module, and verifies
/// that its additional edit does not overlap the completion's primary identifier replacement.
pub(super) struct AutoImportEditPlanner<'a> {
    source: &'a str,
    file: SourceFile,
    offset: u32,
    primary_edit: CompletionEdit,
}

impl<'a> AutoImportEditPlanner<'a> {
    pub(super) fn new(source: &'a str, offset: u32, primary_edit: CompletionEdit) -> Self {
        Self {
            source,
            file: SourceFile::parse(source, Edition::CURRENT).tree(),
            offset,
            primary_edit,
        }
    }

    /// Plan one additional edit, or decline when an existing import or ambiguous syntax makes a
    /// source change unnecessary or unsafe.
    pub(super) fn plan(
        &self,
        path: &Path,
        rendered_path: &str,
    ) -> Option<CompletionAdditionalEdit> {
        let items = self.containing_items();
        let use_items: Vec<ast::Use> = items
            .iter()
            .filter_map(|item| match item {
                ast::Item::Use(use_item) => Some(use_item.clone()),
                _ => None,
            })
            .collect();

        if use_items
            .iter()
            .any(|use_item| self.already_imports(use_item, path))
        {
            return None;
        }

        for use_item in &use_items {
            let Some(edit) = self.coalesced_edit(use_item, rendered_path) else {
                continue;
            };
            return self.does_not_overlap_primary(edit.replace).then_some(edit);
        }

        let edit = self.insertion_edit(&items, &use_items, rendered_path)?;
        self.does_not_overlap_primary(edit.replace).then_some(edit)
    }

    /// Select direct items from the smallest inline module whose body contains the cursor.
    fn containing_items(&self) -> Vec<ast::Item> {
        let cursor = TextSize::from(self.offset);
        let item_list = self
            .file
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
            || self.file.items().collect(),
            |list| list.items().collect(),
        )
    }

    /// Walk the existing tree recursively so nested groups are recognized as duplicates even when
    /// this planner would not attempt to rewrite them.
    fn already_imports(&self, use_item: &ast::Use, path: &Path) -> bool {
        let Some(use_tree) = use_item.use_tree() else {
            return false;
        };
        Self::use_tree_imports_path(use_tree, "", &Self::compact(&path.to_string()))
    }

    /// Coalesce only plain, one-line trees. Attribute-bearing/public imports and nested trees keep
    /// their own source semantics and receive a separate private `use` instead.
    fn coalesced_edit(
        &self,
        use_item: &ast::Use,
        rendered_path: &str,
    ) -> Option<CompletionAdditionalEdit> {
        let item_text = use_item.syntax().text().to_string();
        let trimmed_item = item_text.trim_start();
        if !(trimmed_item.starts_with("use ") || trimmed_item.starts_with("use\n")) {
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
            let new_text = if inner.ends_with(',') {
                format!(" {new_leaf},")
            } else {
                format!(", {new_leaf}")
            };
            return Some(CompletionAdditionalEdit {
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
        Some(CompletionAdditionalEdit {
            replace: Span::from_text_range(use_tree.syntax().text_range()),
            new_text: format!("{prefix}::{{{existing_leaf}, {new_leaf}}}"),
        })
    }

    fn insertion_edit(
        &self,
        items: &[ast::Item],
        use_items: &[ast::Use],
        rendered_path: &str,
    ) -> Option<CompletionAdditionalEdit> {
        if let Some(last_use) = use_items.last() {
            let end = u32::from(last_use.syntax().text_range().end());
            let line_suffix = self
                .source
                .get(usize::try_from(end).ok()?..)?
                .split_once('\n')
                .map_or_else(
                    || self.source.get(usize::try_from(end).ok()?..),
                    |(suffix, _)| Some(suffix),
                )?;
            if line_suffix.trim().is_empty() {
                let indent = Self::line_indent(
                    self.source,
                    u32::from(last_use.syntax().text_range().start()),
                )?;
                return Some(CompletionAdditionalEdit {
                    replace: Span {
                        text: TextSpan { start: end, end },
                    },
                    new_text: format!("\n{indent}use {rendered_path};"),
                });
            }
        }

        let first_item = items.first()?;
        let start = u32::from(first_item.syntax().text_range().start());
        let indent = Self::line_indent(self.source, start)?;
        Some(CompletionAdditionalEdit {
            replace: Span {
                text: TextSpan { start, end: start },
            },
            new_text: format!("use {rendered_path};\n\n{indent}"),
        })
    }

    fn does_not_overlap_primary(&self, additional: Span) -> bool {
        let primary = self.primary_edit.replace.text;
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

    fn compact(text: &str) -> String {
        text.chars()
            .filter(|character| !character.is_whitespace())
            .collect()
    }

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

    use super::AutoImportEditPlanner;
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
        ];
        for (source, expected_edit) in cases {
            let offset = u32::try_from(source.find("HashM").expect("fixture should have cursor"))
                .expect("fixture offset should fit");
            let planner = AutoImportEditPlanner::new(source, offset, primary(offset));
            let edit = planner
                .plan(&hash_map_path(), "std::collections::HashMap")
                .expect("compatible import should produce an edit");
            assert_eq!(edit.new_text, expected_edit);
        }
    }

    #[test]
    fn recognizes_nested_duplicate_use_tree() {
        let source = "use std::{collections::HashMap};\nfn main() { let _: HashM; }";
        let offset = u32::try_from(source.find("HashM").expect("fixture should have cursor"))
            .expect("fixture offset should fit");
        let planner = AutoImportEditPlanner::new(source, offset, primary(offset));
        assert!(
            planner
                .plan(&hash_map_path(), "std::collections::HashMap")
                .is_none()
        );
    }

    #[test]
    fn inserts_into_the_innermost_module_item_list() {
        let source = "mod nested {\n    fn main() { let _: HashM; }\n}\n";
        let offset = u32::try_from(source.find("HashM").expect("fixture should have cursor"))
            .expect("fixture offset should fit");
        let planner = AutoImportEditPlanner::new(source, offset, primary(offset));
        let edit = planner
            .plan(&hash_map_path(), "std::collections::HashMap")
            .expect("module should accept a new import");
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
