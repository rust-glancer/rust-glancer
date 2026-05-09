use std::fmt;

use rg_item_tree::{ImportAlias, ItemTreeRef, UseImportKind, UsePath, VisibilityLevel};
use rg_parse::Span;
use rg_text::{Name, NameInterner};
use rg_workspace::RustEdition;

use super::{ModuleId, Path, PathSegment, path::last_segment_name};

/// One lowered import declaration.
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ImportData {
    pub module: ModuleId,
    pub visibility: VisibilityLevel,
    pub kind: ImportKind,
    pub path: ImportPath,
    pub source_path: ImportSourcePath,
    pub binding: ImportBinding,
    pub alias_span: Option<Span>,
    pub source: ItemTreeRef,
    pub import_index: usize,
}

impl ImportData {
    /// Returns the binding name introduced by this import when it is not a glob import.
    pub(super) fn binding_name(&self) -> Option<Name> {
        let inferred_name = match self.kind {
            ImportKind::Named => self.path.last_name(),
            ImportKind::SelfImport => self.path.last_name(),
            ImportKind::Glob => None,
        };

        self.binding.resolve(inferred_name)
    }

    pub(crate) fn shrink_to_fit(&mut self) {
        self.path.shrink_to_fit();
        self.source_path.shrink_to_fit();
        self.binding.shrink_to_fit();
    }
}

/// Binding strategy for one lowered import or extern crate item.
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
pub enum ImportBinding {
    #[display("")]
    Inferred,
    #[display(" as {_0}")]
    Explicit(Name),
    #[display(" as _")]
    Hidden,
}

impl ImportBinding {
    pub(super) fn from_alias(alias: &ImportAlias) -> Self {
        match alias {
            ImportAlias::Inferred => Self::Inferred,
            ImportAlias::Explicit { name, .. } => Self::Explicit(name.clone()),
            ImportAlias::Hidden => Self::Hidden,
        }
    }

    pub(super) fn resolve(&self, inferred_name: Option<Name>) -> Option<Name> {
        match self {
            Self::Inferred => inferred_name,
            Self::Explicit(name) => Some(name.clone()),
            Self::Hidden => None,
        }
    }

    fn shrink_to_fit(&mut self) {
        if let Self::Explicit(name) = self {
            name.shrink_to_fit();
        }
    }
}

/// Import form that matters for scope propagation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum ImportKind {
    Named,
    SelfImport,
    Glob,
}

impl ImportKind {
    pub(super) fn from_use_kind(kind: UseImportKind) -> Self {
        match kind {
            UseImportKind::Named => Self::Named,
            UseImportKind::SelfImport => Self::SelfImport,
            UseImportKind::Glob => Self::Glob,
        }
    }
}

/// Structured path used during import resolution.
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ImportPath {
    pub absolute: bool,
    pub segments: Vec<PathSegment>,
}

impl ImportPath {
    pub(super) fn from_use_path(path: &UsePath) -> Self {
        let path = Path::from_use_path(path);
        Self {
            absolute: path.absolute,
            segments: path.segments,
        }
    }

    pub(super) fn standard_prelude(edition: RustEdition, interner: &mut NameInterner) -> Self {
        Self {
            absolute: true,
            segments: vec![
                PathSegment::Name(interner.intern("std")),
                PathSegment::Name(interner.intern("prelude")),
                PathSegment::Name(interner.intern(edition.prelude_module())),
            ],
        }
    }

    pub(super) fn last_name(&self) -> Option<Name> {
        last_segment_name(&self.segments)
    }

    fn shrink_to_fit(&mut self) {
        self.segments.shrink_to_fit();
        for segment in &mut self.segments {
            segment.shrink_to_fit();
        }
    }
}

/// Import path plus source spans for each segment.
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ImportSourcePath {
    pub(crate) absolute: bool,
    pub(crate) segments: Vec<ImportSourcePathSegment>,
}

impl ImportSourcePath {
    pub(super) fn from_use_path(path: &UsePath) -> Self {
        let def_map_path = Path::from_use_path(path);
        let segments = def_map_path
            .segments
            .into_iter()
            .zip(path.segments.iter())
            .map(|(segment, source_segment)| ImportSourcePathSegment {
                segment,
                span: source_segment.span,
            })
            .collect();

        Self {
            absolute: path.absolute,
            segments,
        }
    }

    pub fn segments(&self) -> &[ImportSourcePathSegment] {
        &self.segments
    }

    pub fn prefix_path(&self, segment_idx: usize) -> Path {
        Path {
            absolute: self.absolute,
            segments: self
                .segments
                .iter()
                .take(segment_idx + 1)
                .map(|segment| segment.segment.clone())
                .collect(),
        }
    }

    fn shrink_to_fit(&mut self) {
        self.segments.shrink_to_fit();
        for segment in &mut self.segments {
            segment.segment.shrink_to_fit();
        }
    }
}

/// One source-spanned import path segment.
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ImportSourcePathSegment {
    pub(crate) segment: PathSegment,
    pub span: Span,
}

impl From<&ImportPath> for Path {
    fn from(path: &ImportPath) -> Self {
        Self {
            absolute: path.absolute,
            segments: path.segments.clone(),
        }
    }
}

impl fmt::Display for ImportPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Path::from(self).fmt(f)
    }
}
