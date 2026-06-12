use std::fmt;

use rg_parse::Span;
use rg_text::Name;
use wincode::{SchemaRead, SchemaWrite};

use crate::{
    Path, PathSegment,
    items::{GenericArg, TypePath, TypePathSegment, TypeRef},
};
use rg_std::{MemorySize, Shrink};

/// Body expression/pattern path together with body-specific syntax details.
///
/// DefMap paths intentionally keep only the semantic shape. Body paths keep the richer source shape
/// and expose a DefMap projection so existing resolution can keep using DefMap paths.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct BodyPath {
    /// Full source range of the path expression or pattern.
    pub source_span: Span,
    absolute: bool,
    segments: Vec<BodyPathSegment>,
}

#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct BodyPathSegment {
    kind: BodyPathSegmentKind,
    span: Span,
    args: Option<BodyPathSegmentArgs>,
}

#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub enum BodyPathSegmentKind {
    /// `name` in `module::name`.
    Name(Name),
    /// `Self` in type position.
    SelfType,
    /// `self` in value/module path position.
    SelfKw,
    /// `super`.
    SuperKw,
    /// `crate`.
    CrateKw,
    /// `<T>` or `<T as Trait>`.
    ///
    /// This is real path syntax, but it cannot be represented as a plain name-like DefMap segment
    /// without losing the anchor semantics.
    TypeAnchor {
        ty: Option<TypeRef>,
        trait_ref: Option<TypeRef>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub enum BodyPathSegmentArgs {
    /// `<T>` or `::<T>`.
    Angle {
        colon_colon: bool,
        #[wincode(with = "rg_wincode_utils::WincodeDynamic<Vec<GenericArg>>")]
        args: Vec<GenericArg>,
    },
    /// `(A, B) -> C`.
    Parenthesized(String),
}

impl BodyPath {
    pub fn new(source_span: Span, absolute: bool, segments: Vec<BodyPathSegment>) -> Self {
        Self {
            source_span,
            absolute,
            segments,
        }
    }

    /// Returns the compact path form used by the current body resolver.
    ///
    /// This is `None` for rich syntax that has no honest DefMap-path equivalent, such as
    /// `<T as Trait>::Assoc`. Segment generic arguments are dropped in this projection, so
    /// `Maybe::<User>::Some` still projects to `Maybe::Some`.
    pub fn as_def_map_path(&self) -> Option<Path> {
        self.prefix_through(self.segments.len().checked_sub(1)?)
    }

    /// Split the outermost `Type::<T>::item` shape into `Type::<T>` and `item`.
    ///
    /// If the prefix itself contains associated paths, type resolution handles those later. This
    /// only detaches the final associated item name from the syntax-preserving type prefix.
    pub fn split_type_prefix_name(&self) -> Option<(TypeRef, &str)> {
        if self.segments.len() < 2 {
            return None;
        }
        let BodyPathSegmentKind::Name(last_segment) = self.segments.last()?.kind() else {
            return None;
        };

        let prefix_segments = self.segments[..self.segments.len() - 1]
            .iter()
            .map(BodyPathSegment::as_type_path_segment)
            .collect::<Option<Vec<_>>>()?;
        Some((
            TypeRef::Path(TypePath {
                source_span: self.source_span,
                absolute: self.absolute,
                segments: prefix_segments,
            }),
            last_segment.as_str(),
        ))
    }

    pub fn is_absolute(&self) -> bool {
        self.absolute
    }

    pub fn segments(&self) -> &[BodyPathSegment] {
        &self.segments
    }

    /// Returns the DefMap path prefix ending at `segment_idx`.
    ///
    /// This is the shape editor queries need for `Enum::Variant`: a cursor on `Enum` should
    /// resolve the enum type, while a cursor on `Variant` should resolve the variant. Rich pieces
    /// such as type anchors intentionally have no DefMap projection.
    pub fn prefix_through(&self, segment_idx: usize) -> Option<Path> {
        let segments = self
            .segments
            .iter()
            .take(segment_idx.saturating_add(1))
            .map(BodyPathSegment::as_def_map_segment)
            .collect::<Option<Vec<_>>>()?;

        (!segments.is_empty()).then_some(Path {
            absolute: self.absolute,
            segments,
        })
    }

    pub fn segment_span(&self, segment_idx: usize) -> Option<Span> {
        self.segments.get(segment_idx).map(BodyPathSegment::span)
    }

    pub fn segment_count(&self) -> usize {
        self.segments.len()
    }

    /// Returns angle arguments written on the final path segment, such as `T` in `make::<T>`.
    ///
    /// For paths, turbofish arguments belong to the segment they follow; callers that resolve a
    /// function path care about the final segment because that is the called item.
    pub fn last_segment_angle_args(&self) -> Option<&[GenericArg]> {
        self.segments
            .last()
            .and_then(BodyPathSegment::args)
            .and_then(BodyPathSegmentArgs::angle_args)
    }
}

impl BodyPathSegment {
    pub fn new(kind: BodyPathSegmentKind, span: Span, args: Option<BodyPathSegmentArgs>) -> Self {
        Self { kind, span, args }
    }

    pub fn kind(&self) -> &BodyPathSegmentKind {
        &self.kind
    }

    pub fn span(&self) -> Span {
        self.span
    }

    pub fn args(&self) -> Option<&BodyPathSegmentArgs> {
        self.args.as_ref()
    }

    fn as_def_map_segment(&self) -> Option<PathSegment> {
        match &self.kind {
            BodyPathSegmentKind::Name(name) => Some(PathSegment::Name(name.clone())),
            BodyPathSegmentKind::SelfType => Some(PathSegment::Name(Name::new("Self"))),
            BodyPathSegmentKind::SelfKw => Some(PathSegment::SelfKw),
            BodyPathSegmentKind::SuperKw => Some(PathSegment::SuperKw),
            BodyPathSegmentKind::CrateKw => Some(PathSegment::CrateKw),
            BodyPathSegmentKind::TypeAnchor { .. } => None,
        }
    }

    fn as_type_path_segment(&self) -> Option<TypePathSegment> {
        let name = match &self.kind {
            BodyPathSegmentKind::Name(name) => name.clone(),
            BodyPathSegmentKind::SelfType => Name::new("Self"),
            BodyPathSegmentKind::SelfKw => Name::new("self"),
            BodyPathSegmentKind::SuperKw => Name::new("super"),
            BodyPathSegmentKind::CrateKw => Name::new("crate"),
            BodyPathSegmentKind::TypeAnchor { .. } => return None,
        };
        let args = self
            .args
            .as_ref()
            .and_then(BodyPathSegmentArgs::angle_args)
            .unwrap_or(&[])
            .to_vec();

        Some(TypePathSegment {
            name,
            args,
            span: self.span,
        })
    }
}

impl BodyPathSegmentArgs {
    pub fn angle_args(&self) -> Option<&[GenericArg]> {
        match self {
            Self::Angle { args, .. } => Some(args),
            Self::Parenthesized(_) => None,
        }
    }
}

impl fmt::Display for BodyPath {
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

impl fmt::Display for BodyPathSegment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            BodyPathSegmentKind::Name(name) => write!(f, "{name}")?,
            BodyPathSegmentKind::SelfType => write!(f, "Self")?,
            BodyPathSegmentKind::SelfKw => write!(f, "self")?,
            BodyPathSegmentKind::SuperKw => write!(f, "super")?,
            BodyPathSegmentKind::CrateKw => write!(f, "crate")?,
            BodyPathSegmentKind::TypeAnchor { ty, trait_ref } => {
                write!(f, "<")?;
                match ty {
                    Some(ty) => write!(f, "{ty}")?,
                    None => write!(f, "<missing>")?,
                }
                if let Some(trait_ref) = trait_ref {
                    write!(f, " as {trait_ref}")?;
                }
                write!(f, ">")?;
            }
        }

        if let Some(args) = &self.args {
            write!(f, "{args}")?;
        }

        Ok(())
    }
}

impl fmt::Display for BodyPathSegmentArgs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Angle { colon_colon, args } => {
                if *colon_colon {
                    write!(f, "::")?;
                }
                write!(f, "<")?;
                for (idx, arg) in args.iter().enumerate() {
                    if idx > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{arg}")?;
                }
                write!(f, ">")
            }
            Self::Parenthesized(text) => write!(f, "{text}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use rg_parse::{Span, TextSpan};
    use rg_text::Name;

    use crate::items::{GenericArg, TypePath, TypePathSegment, TypeRef};

    use super::{BodyPath, BodyPathSegment, BodyPathSegmentArgs, BodyPathSegmentKind};

    #[test]
    fn splits_outermost_type_prefix_from_final_name() {
        let cases = [
            (
                "nested prefix",
                body_path(
                    false,
                    vec![name("A", None), name("B", None), name("C", None)],
                ),
                Some(("A::B", "C")),
            ),
            (
                "generic prefix",
                body_path(
                    false,
                    vec![
                        name(
                            "Vec",
                            Some(BodyPathSegmentArgs::Angle {
                                colon_colon: true,
                                args: vec![GenericArg::Type(TypeRef::Infer)],
                            }),
                        ),
                        name("new", None),
                    ],
                ),
                Some(("Vec<_>", "new")),
            ),
            (
                "single segment",
                body_path(false, vec![name("Vec", None)]),
                None,
            ),
            (
                "final non-name segment",
                body_path(
                    false,
                    vec![name("Vec", None), segment(BodyPathSegmentKind::SelfKw)],
                ),
                None,
            ),
        ];

        for (label, path, expected) in cases {
            let actual = path
                .split_type_prefix_name()
                .map(|(prefix, name)| (prefix.to_string(), name.to_owned()));
            assert_eq!(
                actual
                    .as_ref()
                    .map(|(prefix, name)| (prefix.as_str(), name.as_str())),
                expected,
                "{label}"
            );
        }
    }

    #[test]
    fn does_not_split_type_anchor_prefixes_as_plain_type_paths() {
        let path = body_path(
            false,
            vec![
                segment(BodyPathSegmentKind::TypeAnchor {
                    ty: Some(type_path_ref("T")),
                    trait_ref: Some(type_path_ref("Trait")),
                }),
                name("Assoc", None),
            ],
        );

        assert_eq!(path.split_type_prefix_name(), None);
    }

    fn body_path(absolute: bool, segments: Vec<BodyPathSegment>) -> BodyPath {
        BodyPath::new(span(), absolute, segments)
    }

    fn name(name: &str, args: Option<BodyPathSegmentArgs>) -> BodyPathSegment {
        BodyPathSegment::new(BodyPathSegmentKind::Name(Name::new(name)), span(), args)
    }

    fn segment(kind: BodyPathSegmentKind) -> BodyPathSegment {
        BodyPathSegment::new(kind, span(), None)
    }

    fn type_path_ref(name: &str) -> TypeRef {
        TypeRef::Path(TypePath {
            source_span: span(),
            absolute: false,
            segments: vec![TypePathSegment {
                name: Name::new(name),
                args: Vec::new(),
                span: span(),
            }],
        })
    }

    fn span() -> Span {
        Span {
            text: TextSpan { start: 0, end: 0 },
        }
    }
}
