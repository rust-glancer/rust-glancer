use std::fmt;

use rg_parse::{Span, TextSpan};
use rg_text::Name;

use super::MacroUseAttr;

/// Syntactic `extern crate` facts attached to `ItemKind::ExternCrate`.
#[derive(
    Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite, rg_memsize::MemorySize,
)]
pub struct ExternCrateItem {
    pub name: Option<Name>,
    pub alias: ImportAlias,
    pub macro_use: Option<MacroUseAttr>,
}

impl ExternCrateItem {
    pub fn shrink_to_fit(&mut self) {
        if let Some(name) = &mut self.name {
            name.shrink_to_fit();
        }
        self.alias.shrink_to_fit();
        if let Some(macro_use) = &mut self.macro_use {
            macro_use.shrink_to_fit();
        }
    }
}

/// Syntactic `use` facts attached to `ItemKind::Use`.
#[derive(
    Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite, rg_memsize::MemorySize,
)]
pub struct UseItem {
    pub imports: Vec<UseImport>,
}

impl UseItem {
    pub fn shrink_to_fit(&mut self) {
        self.imports.shrink_to_fit();
        for import in &mut self.imports {
            import.shrink_to_fit();
        }
    }
}

/// One leaf import produced by a potentially nested use tree.
#[derive(
    Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite, rg_memsize::MemorySize,
)]
pub struct UseImport {
    pub kind: UseImportKind,
    pub path: UsePath,
    pub alias: ImportAlias,
}

impl UseImport {
    pub fn shrink_to_fit(&mut self) {
        self.path.shrink_to_fit();
        self.alias.shrink_to_fit();
    }
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
    rg_memsize::MemorySize,
)]
#[memsize(leaf)]
pub enum UseImportKind {
    #[display("named")]
    Named,
    #[display("self")]
    SelfImport,
    #[display("glob")]
    Glob,
}

/// Explicit import alias, including `as _`.
#[derive(
    Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite, rg_memsize::MemorySize,
)]
pub enum ImportAlias {
    Inferred,
    Explicit { name: Name, span: Span },
    Hidden,
}

impl ImportAlias {
    pub fn shrink_to_fit(&mut self) {
        match self {
            Self::Explicit { name, .. } => name.shrink_to_fit(),
            Self::Inferred | Self::Hidden => {}
        }
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
#[derive(
    Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite, rg_memsize::MemorySize,
)]
pub struct UsePath {
    pub source_span: Option<Span>,
    pub absolute: bool,
    pub segments: Vec<UsePathSegment>,
}

impl UsePath {
    pub fn empty() -> Self {
        Self {
            source_span: None,
            absolute: false,
            segments: Vec::new(),
        }
    }

    pub fn joined(&self, suffix: &Self) -> Self {
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

    pub fn without_trailing_self(&self) -> Self {
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

    pub fn ends_with_self(&self) -> bool {
        matches!(
            self.segments.last().map(|segment| &segment.kind),
            Some(UsePathSegmentKind::SelfKw)
        )
    }

    pub fn shrink_to_fit(&mut self) {
        self.segments.shrink_to_fit();
        for segment in &mut self.segments {
            segment.shrink_to_fit();
        }
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
#[derive(
    Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite, rg_memsize::MemorySize,
)]
pub struct UsePathSegment {
    pub kind: UsePathSegmentKind,
    pub span: Span,
}

impl UsePathSegment {
    pub fn shrink_to_fit(&mut self) {
        self.kind.shrink_to_fit();
    }
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
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
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

impl UsePathSegmentKind {
    pub fn shrink_to_fit(&mut self) {
        match self {
            Self::Name(name) => name.shrink_to_fit(),
            Self::SelfKw | Self::SuperKw | Self::CrateKw => {}
        }
    }
}
