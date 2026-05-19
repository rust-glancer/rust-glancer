use std::fmt;

use rg_def_map::{Path, PathSegment};
use rg_item_tree::{GenericArg, TypeRef};
use rg_parse::Span;
use rg_text::Name;

/// Body expression/pattern path together with body-specific syntax details.
///
/// DefMap paths intentionally keep only the semantic shape. Body IR keeps the richer source shape
/// and exposes a DefMap projection so existing resolution can keep using DefMap paths.
#[derive(Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub struct BodyPath {
    /// Full source range of the path expression or pattern.
    pub source_span: Span,
    pub(crate) absolute: bool,
    // Cached projection into the compact DefMap path shape used by existing resolution.
    // Generic arguments are intentionally erased here; type anchors make the projection absent.
    pub(crate) def_map_path: Option<Path>,
    pub(crate) segments: Vec<BodyPathSegment>,
}

#[derive(Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub(crate) struct BodyPathSegment {
    pub(crate) kind: BodyPathSegmentKind,
    pub(crate) span: Span,
    pub(crate) args: Option<BodyPathSegmentArgs>,
}

#[derive(Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub(crate) enum BodyPathSegmentKind {
    Name(Name),
    SelfType,
    SelfKw,
    SuperKw,
    CrateKw,
    // `<T>` / `<T as Trait>` is real path syntax, but it cannot be represented as a plain
    // name-like DefMap segment without losing the anchor semantics.
    TypeAnchor {
        ty: Option<TypeRef>,
        trait_ref: Option<TypeRef>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub(crate) enum BodyPathSegmentArgs {
    Angle {
        colon_colon: bool,
        #[wincode(with = "rg_wincode_utils::WincodeDynamic<Vec<GenericArg>>")]
        args: Vec<GenericArg>,
    },
    Parenthesized(String),
}

impl BodyPath {
    pub(crate) fn new(source_span: Span, absolute: bool, segments: Vec<BodyPathSegment>) -> Self {
        let def_map_path = Self::build_def_map_path(absolute, &segments);

        Self {
            source_span,
            absolute,
            def_map_path,
            segments,
        }
    }

    /// Returns the compact path form used by the current body resolver.
    ///
    /// This is `None` for rich syntax that has no honest DefMap-path equivalent, such as
    /// `<T as Trait>::Assoc`. Segment generic arguments are dropped in this projection, so
    /// `Maybe::<User>::Some` still projects to `Maybe::Some`.
    pub fn as_def_map_path(&self) -> Option<&Path> {
        self.def_map_path.as_ref()
    }

    pub fn is_absolute(&self) -> bool {
        self.absolute
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
        self.segments.get(segment_idx).map(|segment| segment.span)
    }

    pub fn segment_count(&self) -> usize {
        self.segments.len()
    }

    pub(crate) fn walk_type_refs<'path>(&'path self, visit: &mut impl FnMut(&'path TypeRef)) {
        for segment in &self.segments {
            segment.walk_type_refs(visit);
        }
    }

    pub(crate) fn shrink_to_fit(&mut self) {
        if let Some(path) = &mut self.def_map_path {
            path.shrink_to_fit();
        }
        self.segments.shrink_to_fit();
        for segment in &mut self.segments {
            segment.shrink_to_fit();
        }
    }

    fn build_def_map_path(absolute: bool, segments: &[BodyPathSegment]) -> Option<Path> {
        // Build the resolver-facing view only when every segment has a lossless DefMap shape.
        // A missing projection is safer than letting rich syntax resolve as a misleading name.
        let segments = segments
            .iter()
            .map(BodyPathSegment::as_def_map_segment)
            .collect::<Option<Vec<_>>>()?;

        (!segments.is_empty()).then_some(Path { absolute, segments })
    }
}

impl BodyPathSegment {
    pub(crate) fn new(
        kind: BodyPathSegmentKind,
        span: Span,
        args: Option<BodyPathSegmentArgs>,
    ) -> Self {
        Self { kind, span, args }
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

    fn walk_type_refs<'path>(&'path self, visit: &mut impl FnMut(&'path TypeRef)) {
        if let BodyPathSegmentKind::TypeAnchor { ty, trait_ref } = &self.kind {
            if let Some(ty) = ty {
                visit(ty);
            }
            if let Some(trait_ref) = trait_ref {
                visit(trait_ref);
            }
        }

        if let Some(args) = &self.args {
            args.walk_type_refs(visit);
        }
    }

    fn shrink_to_fit(&mut self) {
        self.kind.shrink_to_fit();
        if let Some(args) = &mut self.args {
            args.shrink_to_fit();
        }
    }
}

impl BodyPathSegmentKind {
    fn shrink_to_fit(&mut self) {
        match self {
            Self::Name(name) => name.shrink_to_fit(),
            Self::TypeAnchor { ty, trait_ref } => {
                if let Some(ty) = ty {
                    ty.shrink_to_fit();
                }
                if let Some(trait_ref) = trait_ref {
                    trait_ref.shrink_to_fit();
                }
            }
            Self::SelfType | Self::SelfKw | Self::SuperKw | Self::CrateKw => {}
        }
    }
}

impl BodyPathSegmentArgs {
    fn walk_type_refs<'path>(&'path self, visit: &mut impl FnMut(&'path TypeRef)) {
        let Self::Angle { args, .. } = self else {
            return;
        };

        for arg in args {
            match arg {
                GenericArg::Type(ty) => visit(ty),
                GenericArg::AssocType { ty: Some(ty), .. } => visit(ty),
                GenericArg::Lifetime(_)
                | GenericArg::Const(_)
                | GenericArg::AssocType { ty: None, .. }
                | GenericArg::Unsupported(_) => {}
            }
        }
    }

    fn shrink_to_fit(&mut self) {
        match self {
            Self::Angle { args, .. } => {
                args.shrink_to_fit();
                for arg in args {
                    arg.shrink_to_fit();
                }
            }
            Self::Parenthesized(text) => text.shrink_to_fit(),
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
