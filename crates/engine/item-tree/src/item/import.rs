use std::fmt;

use ra_syntax::{
    AstNode as _,
    ast::{self, HasName},
};

use rg_parse::Span;
use rg_text::{Name, NameInterner};

/// Syntactic `extern crate` facts attached to `ItemKind::ExternCrate`.
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
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
                let Some(path) = UsePath::from_ast(&path, interner) else {
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
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
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
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
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
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct UsePath {
    pub absolute: bool,
    pub segments: Vec<UsePathSegment>,
}

impl UsePath {
    fn empty() -> Self {
        Self {
            absolute: false,
            segments: Vec::new(),
        }
    }

    fn from_ast(path: &ast::Path, interner: &mut NameInterner) -> Option<Self> {
        let mut segments = Vec::new();

        for segment in path.segments() {
            let span = Span::from_text_range(segment.syntax().text_range());
            let lowered_segment = match segment.kind()? {
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

        Some(Self {
            absolute: path
                .first_segment()
                .is_some_and(|segment| segment.coloncolon_token().is_some()),
            segments,
        })
    }

    fn joined(&self, suffix: &Self) -> Self {
        let mut segments = self.segments.clone();
        segments.extend(suffix.segments.clone());
        Self {
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
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
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
    Debug,
    Clone,
    PartialEq,
    Eq,
    derive_more::Display,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
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
