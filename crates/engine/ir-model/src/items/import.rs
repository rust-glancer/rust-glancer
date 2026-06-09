use rg_std::{MemorySize, Shrink};
use std::fmt;
use wincode::{SchemaRead, SchemaWrite};

use rg_parse::{Span, TextSpan};
use rg_text::Name;

use super::MacroUseAttr;

/// Syntactic `extern crate` facts attached to `ItemKind::ExternCrate`.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct ExternCrateItem {
    pub name: Option<Name>,
    pub alias: ImportAlias,
    pub macro_use: Option<MacroUseAttr>,
}

/// Syntactic `use` facts attached to `ItemKind::Use`.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct UseItem {
    pub imports: Vec<UseImport>,
}

/// One leaf import produced by a potentially nested use tree.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
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
    SchemaRead,
    SchemaWrite,
    MemorySize,
    Shrink,
)]
#[memsize(leaf)]
#[shrink(leaf)]
pub enum UseImportKind {
    #[display("named")]
    Named,
    #[display("self")]
    SelfImport,
    #[display("glob")]
    Glob,
}

/// Explicit import alias, including `as _`.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub enum ImportAlias {
    Inferred,
    Explicit { name: Name, span: Span },
    Hidden,
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
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
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
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
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
    Debug, Clone, PartialEq, Eq, derive_more::Display, SchemaRead, SchemaWrite, MemorySize, Shrink,
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
