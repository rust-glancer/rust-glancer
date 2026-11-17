use rg_syntax::{
    AstNode as _, SyntaxKind, TextRange, algo,
    ast::{self, HasName},
};
use rg_text::Name;

use super::{FromAst, MaybeFromAst as _};
use crate::{
    RecordSource,
    item::{
        ExternCrateItem, ImportAlias, MacroUseAttr, UseImport, UseImportKind, UseItem, UsePath,
        UsePathSegment, UsePathSegmentKind,
    },
};

impl FromAst for ExternCrateItem {
    type AstNode = ast::ExternCrate;
    type Context<'a> = &'a mut dyn RecordSource;

    fn from_ast(item: &Self::AstNode, source: Self::Context<'_>) -> Self {
        Self {
            name: item.name_ref().map(|name_ref| Name::new(name_ref.text())),
            alias: import_alias_from_rename(item.rename(), source),
            macro_use: MacroUseAttr::maybe_from_ast(item, ()),
        }
    }
}

impl FromAst for UseItem {
    type AstNode = ast::Use;
    type Context<'a> = &'a mut dyn RecordSource;

    fn from_ast(item: &Self::AstNode, source: Self::Context<'_>) -> Self {
        let mut imports = Vec::new();

        if let Some(use_tree) = item.use_tree() {
            UsePathBuilder::default().collect_imports(&mut imports, use_tree, source);
        }

        Self { imports }
    }
}

// A nested import joins several written path pieces. Record each completed leaf only after
// collecting its syntax, so the shared prefix keeps the same location in every leaf.
#[derive(Clone, Default)]
struct UsePathBuilder {
    source_span: Option<TextRange>,
    absolute: bool,
    segments: Vec<(UsePathSegmentKind, TextRange)>,
}

impl UsePathBuilder {
    fn collect_imports(
        &self,
        imports: &mut Vec<UseImport>,
        use_tree: ast::UseTree,
        source: &mut dyn RecordSource,
    ) {
        let mut path = self.clone();
        if let Some(syntax) = use_tree.path() {
            let Some(suffix) = Self::from_ast(&syntax, &use_tree) else {
                return;
            };
            path.segments.extend(suffix.segments);
            path.source_span = match (path.source_span, suffix.source_span) {
                (Some(left), Some(right)) => Some(left.cover(right)),
                (Some(span), None) | (None, Some(span)) => Some(span),
                (None, None) => None,
            };
            path.absolute |= suffix.absolute;
        }

        if let Some(use_tree_list) = use_tree.use_tree_list() {
            for child_use_tree in use_tree_list.use_trees() {
                path.collect_imports(imports, child_use_tree, source);
            }
            return;
        }

        // Publish source occurrences alongside each completed import.
        let path = UsePath {
            location: path.source_span.map(|range| source.record(range)),
            absolute: path.absolute,
            segments: path
                .segments
                .into_iter()
                .map(|(kind, span)| UsePathSegment {
                    kind,
                    location: source.record(span),
                })
                .collect(),
        };
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
            alias: import_alias_from_rename(use_tree.rename(), source),
        });
    }

    fn from_ast(path: &ast::Path, use_tree: &ast::UseTree) -> Option<Self> {
        let mut segments = Vec::new();

        for segment in path.segments() {
            let span = segment.syntax().text_range();
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
            let (kind, span) = match kind {
                ast::PathSegmentKind::Name(name_ref) => (
                    UsePathSegmentKind::Name(Name::new(name_ref.text())),
                    name_ref.syntax().text_range(),
                ),
                ast::PathSegmentKind::SelfKw => (UsePathSegmentKind::SelfKw, span),
                ast::PathSegmentKind::SuperKw => (UsePathSegmentKind::SuperKw, span),
                ast::PathSegmentKind::CrateKw => (UsePathSegmentKind::CrateKw, span),
                ast::PathSegmentKind::SelfTypeKw | ast::PathSegmentKind::Type { .. } => {
                    return None;
                }
            };
            segments.push((kind, span));
        }
        if segments.is_empty() {
            return None;
        }

        Some(Self {
            source_span: Some(Self::source_span(path, use_tree)),
            absolute: path
                .first_segment()
                .is_some_and(|segment| segment.coloncolon_token().is_some()),
            segments,
        })
    }

    fn source_span(path: &ast::Path, use_tree: &ast::UseTree) -> TextRange {
        let mut span = path.syntax().text_range();
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
        if use_tree_range.start() <= colon_range.start()
            && colon_range.end() <= use_tree_range.end()
        {
            span = TextRange::new(span.start(), colon_range.end());
        }
        span
    }
}

fn import_alias_from_rename(
    rename: Option<ast::Rename>,
    source: &mut dyn RecordSource,
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
            location: source.record(name.syntax().text_range()),
            name: Name::new(name.text()),
        })
        .unwrap_or(ImportAlias::Inferred)
}
