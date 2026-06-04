use rg_ir_model::items::{
    ExternCrateItem, ImportAlias, MacroUseAttr, UseImport, UseImportKind, UseItem, UsePath,
    UsePathSegment, UsePathSegmentKind,
};
use rg_parse::{Span, TextSpan};
use rg_syntax::{
    AstNode as _, SyntaxKind, algo,
    ast::{self, HasName},
};
use rg_text::NameInterner;

use super::{FromAst, MaybeFromAst as _};

impl FromAst for ExternCrateItem {
    type AstNode = ast::ExternCrate;
    type Context<'a> = &'a mut NameInterner;

    fn from_ast(item: &Self::AstNode, interner: Self::Context<'_>) -> Self {
        Self {
            name: item
                .name_ref()
                .map(|name_ref| interner.intern(name_ref.text())),
            alias: import_alias_from_rename(item.rename(), interner),
            macro_use: MacroUseAttr::maybe_from_ast(item, interner),
        }
    }
}

impl FromAst for UseItem {
    type AstNode = ast::Use;
    type Context<'a> = &'a mut NameInterner;

    fn from_ast(item: &Self::AstNode, interner: Self::Context<'_>) -> Self {
        let mut imports = Vec::new();

        if let Some(use_tree) = item.use_tree() {
            lower_use_tree(&mut imports, &UsePath::empty(), use_tree, interner);
        }

        Self { imports }
    }
}

fn lower_use_tree(
    imports: &mut Vec<UseImport>,
    prefix: &UsePath,
    use_tree: ast::UseTree,
    interner: &mut NameInterner,
) {
    let path = match use_tree.path() {
        Some(path) => {
            let Some(path) = use_path_from_ast(&path, &use_tree, interner) else {
                return;
            };
            prefix.joined(&path)
        }
        None => prefix.clone(),
    };

    if let Some(use_tree_list) = use_tree.use_tree_list() {
        for child_use_tree in use_tree_list.use_trees() {
            lower_use_tree(imports, &path, child_use_tree, interner);
        }
        return;
    }

    let (kind, path) = if use_tree.star_token().is_some() {
        (UseImportKind::Glob, path)
    } else if path.ends_with_self() {
        (UseImportKind::SelfImport, path.without_trailing_self())
    } else {
        (UseImportKind::Named, path)
    };

    imports.push(UseImport {
        kind,
        path,
        alias: import_alias_from_rename(use_tree.rename(), interner),
    });
}

fn import_alias_from_rename(
    rename: Option<ast::Rename>,
    interner: &mut NameInterner,
) -> ImportAlias {
    let Some(rename) = rename else {
        return ImportAlias::Inferred;
    };

    if rename.underscore_token().is_some() {
        return ImportAlias::Hidden;
    }

    rename
        .name()
        .map(|name| ImportAlias::Explicit {
            span: Span::from_text_range(name.syntax().text_range()),
            name: interner.intern(name.text()),
        })
        .unwrap_or(ImportAlias::Inferred)
}

fn use_path_from_ast(
    path: &ast::Path,
    use_tree: &ast::UseTree,
    interner: &mut NameInterner,
) -> Option<UsePath> {
    let mut segments = Vec::new();

    for segment in path.segments() {
        let span = Span::from_text_range(segment.syntax().text_range());
        let Some(kind) = segment.kind() else {
            // A live edit such as `use crate::module::` produces an empty trailing segment. Keep
            // the completed prefix so completion can fill that final segment.
            if span.is_empty() {
                continue;
            }

            // Syntax recovery may attach the next item's attribute marker as a bogus segment after
            // the trailing `::`. The valid prefix is still useful for completion, so stop before
            // the recovered token instead of dropping the whole use path.
            if !segments.is_empty() {
                break;
            }
            return None;
        };
        let lowered_segment = match kind {
            ast::PathSegmentKind::Name(name_ref) => UsePathSegment {
                kind: UsePathSegmentKind::Name(interner.intern(name_ref.text())),
                span: Span::from_text_range(name_ref.syntax().text_range()),
            },
            ast::PathSegmentKind::SelfKw => UsePathSegment {
                kind: UsePathSegmentKind::SelfKw,
                span,
            },
            ast::PathSegmentKind::SuperKw => UsePathSegment {
                kind: UsePathSegmentKind::SuperKw,
                span,
            },
            ast::PathSegmentKind::CrateKw => UsePathSegment {
                kind: UsePathSegmentKind::CrateKw,
                span,
            },
            ast::PathSegmentKind::SelfTypeKw | ast::PathSegmentKind::Type { .. } => {
                return None;
            }
        };
        segments.push(lowered_segment);
    }
    if segments.is_empty() {
        return None;
    }

    Some(UsePath {
        source_span: Some(use_path_source_span(path, use_tree)),
        absolute: path
            .first_segment()
            .is_some_and(|segment| segment.coloncolon_token().is_some()),
        segments,
    })
}

fn use_path_source_span(path: &ast::Path, use_tree: &ast::UseTree) -> Span {
    let mut span = Span::from_text_range(path.syntax().text_range());
    let Some(next_token) = algo::next_non_trivia_token(path.syntax().clone()) else {
        return span;
    };
    if next_token.kind() != SyntaxKind::COLON2 {
        return span;
    }

    // Some use-tree forms keep a trailing separator next to the path rather than in a named segment
    // span. Keep that token in the source span so completion can recognize an empty final segment.
    let use_tree_range = use_tree.syntax().text_range();
    let colon_range = next_token.text_range();
    if use_tree_range.start() <= colon_range.start() && colon_range.end() <= use_tree_range.end() {
        span.text = TextSpan {
            start: span.text.start,
            end: u32::from(colon_range.end()),
        };
    }
    span
}
