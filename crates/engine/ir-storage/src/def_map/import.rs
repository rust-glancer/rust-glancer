use rg_std::{MemorySize, Shrink};
use std::fmt;
use wincode::{SchemaRead, SchemaWrite};

use rg_ir_model::{
    ModuleId, Path, PathSegment, TargetRef,
    hir::source::ItemSource,
    items::{ImportAlias, UseImportKind, UsePath, VisibilityLevel},
    last_segment_name,
};
use rg_parse::Span;
use rg_text::{Name, NameInterner};
use rg_workspace::RustEdition;

/// One lowered import declaration.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct ImportData {
    pub module: ModuleId,
    pub visibility: VisibilityLevel,
    pub kind: ImportKind,
    pub path: ImportPath,
    pub source_path: ImportSourcePath,
    pub binding: ImportBinding,
    pub alias_span: Option<Span>,
    pub source: ItemSource,
    pub import_index: usize,
}

impl ImportData {
    /// Returns the binding name introduced by this import when it is not a glob import.
    pub fn binding_name(&self) -> Option<Name> {
        let inferred_name = match self.kind {
            ImportKind::Named => self.path.last_name(),
            ImportKind::SelfImport => self.path.last_name(),
            ImportKind::Glob => None,
        };

        self.binding.resolve(inferred_name)
    }
}

/// Binding strategy for one lowered import or extern crate item.
#[derive(
    Debug, Clone, PartialEq, Eq, derive_more::Display, SchemaRead, SchemaWrite, MemorySize, Shrink,
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
    pub fn from_alias(alias: &ImportAlias) -> Self {
        match alias {
            ImportAlias::Inferred => Self::Inferred,
            ImportAlias::Explicit { name, .. } => Self::Explicit(name.clone()),
            ImportAlias::Hidden => Self::Hidden,
        }
    }

    pub fn resolve(&self, inferred_name: Option<Name>) -> Option<Name> {
        match self {
            Self::Inferred => inferred_name,
            Self::Explicit(name) => Some(name.clone()),
            Self::Hidden => None,
        }
    }
}

/// Import form that matters for scope propagation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[memsize(leaf)]
#[shrink(leaf)]
pub enum ImportKind {
    Named,
    SelfImport,
    Glob,
}

impl ImportKind {
    pub fn from_use_kind(kind: UseImportKind) -> Self {
        match kind {
            UseImportKind::Named => Self::Named,
            UseImportKind::SelfImport => Self::SelfImport,
            UseImportKind::Glob => Self::Glob,
        }
    }
}

/// Structured path used during import resolution.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct ImportPath {
    pub absolute: bool,
    pub segments: Vec<PathSegment>,
}

impl ImportPath {
    pub fn from_use_path(path: &UsePath) -> Self {
        let path = Path::from_use_path(path);
        Self {
            absolute: path.absolute,
            segments: path.segments,
        }
    }

    pub fn standard_prelude(
        crate_name: &'static str,
        edition: RustEdition,
        interner: &mut NameInterner,
    ) -> Self {
        Self {
            absolute: true,
            segments: vec![
                PathSegment::Name(interner.intern(crate_name)),
                PathSegment::Name(interner.intern("prelude")),
                PathSegment::Name(interner.intern(edition.prelude_module())),
            ],
        }
    }

    pub fn crate_relative_standard_prelude(
        edition: RustEdition,
        interner: &mut NameInterner,
    ) -> Self {
        Self {
            absolute: false,
            segments: vec![
                PathSegment::Name(interner.intern("prelude")),
                PathSegment::Name(interner.intern(edition.prelude_module())),
            ],
        }
    }

    /// Parses the textual callee path stored in item-tree or AST macro-call data.
    ///
    /// A `$crate` segment only has meaning after resolution has selected the macro definition crate.
    /// Callers that do not have that origin pass `None`, and `$crate` paths are rejected instead of
    /// being guessed from the call site.
    pub fn from_macro_path_text(
        path: &str,
        dollar_crate_target: Option<TargetRef>,
    ) -> Option<Self> {
        let path = path.trim();
        let absolute = path.starts_with("::");
        let path = path.trim_start_matches("::");
        let mut segments = Vec::new();

        for segment in path.split("::") {
            let segment = segment.trim();
            if segment.is_empty() {
                return None;
            }
            segments.push(match segment {
                "$crate" => PathSegment::DollarCrate(dollar_crate_target?),
                "self" => PathSegment::SelfKw,
                "super" => PathSegment::SuperKw,
                "crate" => PathSegment::CrateKw,
                name => PathSegment::Name(Name::new(name)),
            });
        }

        (!segments.is_empty()).then_some(Self { absolute, segments })
    }

    pub(super) fn last_name(&self) -> Option<Name> {
        last_segment_name(&self.segments)
    }

    /// Returns the name for a path that is exactly one relative named segment.
    pub fn relative_single_name(&self) -> Option<&Name> {
        if self.absolute || self.segments.len() != 1 {
            return None;
        }

        match self.segments.first()? {
            PathSegment::Name(name) => Some(name),
            PathSegment::SelfKw
            | PathSegment::SuperKw
            | PathSegment::CrateKw
            | PathSegment::DollarCrate(_) => None,
        }
    }
}

/// Import path plus source spans for each segment.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct ImportSourcePath {
    pub source_span: Option<Span>,
    pub absolute: bool,
    pub segments: Vec<ImportSourcePathSegment>,
}

impl ImportSourcePath {
    pub fn from_use_path(path: &UsePath) -> Self {
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
            source_span: path.source_span,
            absolute: path.absolute,
            segments,
        }
    }

    pub fn segments(&self) -> &[ImportSourcePathSegment] {
        &self.segments
    }

    pub fn source_span(&self) -> Option<Span> {
        self.source_span
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
}

/// One source-spanned import path segment.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct ImportSourcePathSegment {
    pub segment: PathSegment,
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
