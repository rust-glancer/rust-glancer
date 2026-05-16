use std::fmt;

use ra_syntax::{
    AstNode as _, SyntaxKind, algo,
    ast::{self, HasName},
};

use rg_parse::{Span, TextSpan};
use rg_text::{Name, NameInterner};

/// Syntactic `extern crate` facts attached to `ItemKind::ExternCrate`.
#[derive(Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub struct ExternCrateItem {
    pub name: Option<Name>,
    pub alias: ImportAlias,
}

impl ExternCrateItem {
    pub fn from_ast(item: &ast::ExternCrate, interner: &mut NameInterner) -> Self {
        Self {
            name: item
                .name_ref()
                .map(|name_ref| interner.intern(name_ref.text())),
            alias: ImportAlias::from_rename(item.rename(), interner),
        }
    }
}

/// Syntactic `use` facts attached to `ItemKind::Use`.
#[derive(Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub struct UseItem {
    pub imports: Vec<UseImport>,
}

impl UseItem {
    pub fn from_ast(item: &ast::Use, interner: &mut NameInterner) -> Self {
        let mut imports = Vec::new();

        if let Some(use_tree) = item.use_tree() {
            Self::lower_use_tree(&mut imports, &UsePath::empty(), use_tree, interner);
        }

        Self { imports }
    }

    fn lower_use_tree(
        imports: &mut Vec<UseImport>,
        prefix: &UsePath,
        use_tree: ast::UseTree,
        interner: &mut NameInterner,
    ) {
        let path = match use_tree.path() {
            Some(path) => {
                let Some(path) = UsePath::from_ast(&path, &use_tree, interner) else {
                    return;
                };
                prefix.joined(&path)
            }
            None => prefix.clone(),
        };

        if let Some(use_tree_list) = use_tree.use_tree_list() {
            for child_use_tree in use_tree_list.use_trees() {
                Self::lower_use_tree(imports, &path, child_use_tree, interner);
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
            alias: ImportAlias::from_rename(use_tree.rename(), interner),
        });
    }
}

/// One leaf import produced by a potentially nested use tree.
#[derive(Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub struct UseImport {
    pub kind: UseImportKind,
    pub path: UsePath,
    pub alias: ImportAlias,
}

/// Import form before name resolution.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    derive_more::Display,
    wincode::SchemaRead,
    wincode::SchemaWrite,
)]
pub enum UseImportKind {
    #[display("named")]
    Named,
    #[display("self")]
    SelfImport,
    #[display("glob")]
    Glob,
}

/// Explicit import alias, including `as _`.
#[derive(Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub enum ImportAlias {
    Inferred,
    Explicit { name: Name, span: Span },
    Hidden,
}

impl ImportAlias {
    pub fn from_rename(rename: Option<ast::Rename>, interner: &mut NameInterner) -> Self {
        let Some(rename) = rename else {
            return Self::Inferred;
        };

        if rename.underscore_token().is_some() {
            return Self::Hidden;
        }

        rename
            .name()
            .map(|name| Self::Explicit {
                span: Span::from_text_range(name.syntax().text_range()),
                name: interner.intern(name.text()),
            })
            .unwrap_or(Self::Inferred)
    }
}

impl fmt::Display for ImportAlias {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Inferred => Ok(()),
            Self::Explicit { name, .. } => write!(f, " as {name}"),
            Self::Hidden => write!(f, " as _"),
        }
    }
}

/// Structured path used before semantic resolution.
#[derive(Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub struct UsePath {
    pub source_span: Option<Span>,
    pub absolute: bool,
    pub segments: Vec<UsePathSegment>,
}

impl UsePath {
    fn empty() -> Self {
        Self {
            source_span: None,
            absolute: false,
            segments: Vec::new(),
        }
    }

    fn from_ast(
        path: &ast::Path,
        use_tree: &ast::UseTree,
        interner: &mut NameInterner,
    ) -> Option<Self> {
        let mut segments = Vec::new();

        for segment in path.segments() {
            let span = Span::from_text_range(segment.syntax().text_range());
            let Some(kind) = segment.kind() else {
                // A live edit such as `use crate::module::` produces an empty trailing segment.
                // Keep the completed prefix so completion can fill that final segment.
                if span.is_empty() {
                    continue;
                }

                // Syntax recovery may attach the next item's attribute marker as a bogus segment
                // after the trailing `::`. The valid prefix is still useful for completion, so stop
                // before the recovered token instead of dropping the whole use path.
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

        Some(Self {
            source_span: Some(Self::source_span(path, use_tree)),
            absolute: path
                .first_segment()
                .is_some_and(|segment| segment.coloncolon_token().is_some()),
            segments,
        })
    }

    fn source_span(path: &ast::Path, use_tree: &ast::UseTree) -> Span {
        let mut span = Span::from_text_range(path.syntax().text_range());
        let Some(next_token) = algo::next_non_trivia_token(path.syntax().clone()) else {
            return span;
        };
        if next_token.kind() != SyntaxKind::COLON2 {
            return span;
        }

        // Some use-tree forms keep a trailing separator next to the path rather than in a named
        // segment span. Keep that token in the source span so completion can recognize an empty
        // final segment.
        let use_tree_range = use_tree.syntax().text_range();
        let colon_range = next_token.text_range();
        if use_tree_range.start() <= colon_range.start()
            && colon_range.end() <= use_tree_range.end()
        {
            span.text.end = u32::from(colon_range.end());
        }
        span
    }

    fn joined(&self, suffix: &Self) -> Self {
        let mut segments = self.segments.clone();
        segments.extend(suffix.segments.clone());
        let source_span = match (self.source_span, suffix.source_span) {
            (Some(left), Some(right)) => Some(Span {
                text: TextSpan {
                    start: left.text.start.min(right.text.start),
                    end: left.text.end.max(right.text.end),
                },
            }),
            (Some(span), None) | (None, Some(span)) => Some(span),
            (None, None) => None,
        };

        Self {
            source_span,
            absolute: self.absolute || suffix.absolute,
            segments,
        }
    }

    fn without_trailing_self(&self) -> Self {
        let mut segments = self.segments.clone();
        if matches!(
            segments.last().map(|segment| &segment.kind),
            Some(UsePathSegmentKind::SelfKw)
        ) {
            segments.pop();
        }
        Self {
            source_span: self.source_span,
            absolute: self.absolute,
            segments,
        }
    }

    fn ends_with_self(&self) -> bool {
        matches!(
            self.segments.last().map(|segment| &segment.kind),
            Some(UsePathSegmentKind::SelfKw)
        )
    }
}

impl fmt::Display for UsePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.absolute {
            write!(f, "::")?;
        }

        for (idx, segment) in self.segments.iter().enumerate() {
            if idx > 0 {
                write!(f, "::")?;
            }
            write!(f, "{segment}")?;
        }

        Ok(())
    }
}

/// One structured path segment.
#[derive(Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub struct UsePathSegment {
    pub kind: UsePathSegmentKind,
    pub span: Span,
}

impl fmt::Display for UsePathSegment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.kind.fmt(f)
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, derive_more::Display, wincode::SchemaRead, wincode::SchemaWrite,
)]
pub enum UsePathSegmentKind {
    #[display("{_0}")]
    Name(Name),
    #[display("self")]
    SelfKw,
    #[display("super")]
    SuperKw,
    #[display("crate")]
    CrateKw,
}
