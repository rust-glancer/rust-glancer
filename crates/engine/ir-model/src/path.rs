use std::fmt;

use wincode::{SchemaRead, SchemaWrite};

use rg_text::{Name, NameInterner, RustEdition};

use crate::CrateRef;
use rg_std::{MemorySize, Shrink};

/// Structured path used by def-map path resolution queries.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct Path {
    pub absolute: bool,
    pub segments: Vec<PathSegment>,
}

impl Path {
    /// Builds a single-segment relative path for ordinary lexical lookup.
    pub fn unqualified_name(name: impl AsRef<str>) -> Self {
        Self {
            absolute: false,
            segments: vec![PathSegment::Name(Name::new(name.as_ref()))],
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
    pub fn from_macro_path_text(path: &str, dollar_crate: Option<CrateRef>) -> Option<Self> {
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
                "$crate" => PathSegment::DollarCrate(dollar_crate?),
                "self" => PathSegment::SelfKw,
                "super" => PathSegment::SuperKw,
                "crate" => PathSegment::CrateKw,
                name => PathSegment::Name(Name::new(name)),
            });
        }

        (!segments.is_empty()).then_some(Self { absolute, segments })
    }

    pub fn last_name(&self) -> Option<Name> {
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

    /// Returns the name of a single-segment relative path that can participate in local lookup.
    pub fn single_name(&self) -> Option<&str> {
        if self.absolute || self.segments.len() != 1 {
            return None;
        }

        match self.segments.first()? {
            PathSegment::Name(name) => Some(name.as_str()),
            PathSegment::SelfKw => Some("self"),
            PathSegment::SuperKw | PathSegment::CrateKw | PathSegment::DollarCrate(_) => None,
        }
    }

    pub fn is_self_type(&self) -> bool {
        self.is_plain_ident("Self")
    }

    pub fn is_plain_ident(&self, ident: &str) -> bool {
        !self.absolute
            && self.segments.len() == 1
            && matches!(self.segments.first(), Some(PathSegment::Name(name)) if name == ident)
    }

    pub fn last_segment_label(&self) -> Option<String> {
        last_segment_name(&self.segments).map(|name| name.to_string())
    }

    /// Splits the outermost `prefix::name` shape into `prefix` and `name`.
    ///
    /// Callers that need associated paths resolve the prefix separately; this only detaches the
    /// final plain-name segment.
    pub fn split_prefix_name(&self) -> Option<(Self, &str)> {
        if self.segments.len() < 2 {
            return None;
        }

        let PathSegment::Name(last_segment) = self.segments.last()? else {
            return None;
        };

        Some((
            Self {
                absolute: self.absolute,
                segments: self.segments[..self.segments.len() - 1].to_vec(),
            },
            last_segment.as_str(),
        ))
    }
}

impl fmt::Display for Path {
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
    Debug, Clone, PartialEq, Eq, derive_more::Display, SchemaRead, SchemaWrite, MemorySize, Shrink,
)]
pub enum PathSegment {
    #[display("{_0}")]
    Name(Name),
    #[display("self")]
    SelfKw,
    #[display("super")]
    SuperKw,
    #[display("crate")]
    CrateKw,
    #[display("$crate")]
    #[memsize(skip)]
    DollarCrate(CrateRef),
}

impl PathSegment {
    /// Classifies a syntax path name into the keyword-aware DefMap representation.
    pub fn from_syntax_name(name: &Name) -> Self {
        match name.as_str() {
            "self" => Self::SelfKw,
            "super" => Self::SuperKw,
            "crate" => Self::CrateKw,
            _ => Self::Name(name.clone()),
        }
    }
}

pub fn last_segment_name(segments: &[PathSegment]) -> Option<Name> {
    match segments.last()? {
        PathSegment::Name(name) => Some(name.clone()),
        PathSegment::SelfKw => Some(Name::new("self")),
        PathSegment::SuperKw => Some(Name::new("super")),
        PathSegment::CrateKw => Some(Name::new("crate")),
        PathSegment::DollarCrate(_) => Some(Name::new("$crate")),
    }
}

#[cfg(test)]
mod tests {
    use rg_text::Name;

    use super::{Path, PathSegment};

    #[test]
    fn classifies_single_segment_paths() {
        let cases = [
            (
                "plain name",
                path(false, vec![PathSegment::Name(Name::new("User"))]),
                Some("User"),
            ),
            (
                "self keyword",
                path(false, vec![PathSegment::SelfKw]),
                Some("self"),
            ),
            (
                "super keyword",
                path(false, vec![PathSegment::SuperKw]),
                None,
            ),
            (
                "crate keyword",
                path(false, vec![PathSegment::CrateKw]),
                None,
            ),
            (
                "absolute name",
                path(true, vec![PathSegment::Name(Name::new("User"))]),
                None,
            ),
            (
                "qualified name",
                path(
                    false,
                    vec![
                        PathSegment::Name(Name::new("api")),
                        PathSegment::Name(Name::new("User")),
                    ],
                ),
                None,
            ),
        ];

        for (label, path, expected) in cases {
            assert_eq!(path.single_name(), expected, "{label}");
        }
    }

    #[test]
    fn splits_prefix_from_final_name_segment() {
        let cases = [
            (
                "relative name path",
                path(
                    false,
                    vec![
                        PathSegment::Name(Name::new("api")),
                        PathSegment::Name(Name::new("User")),
                    ],
                ),
                Some(("api", "User")),
            ),
            (
                "nested name path",
                path(
                    false,
                    vec![
                        PathSegment::Name(Name::new("api")),
                        PathSegment::Name(Name::new("User")),
                        PathSegment::Name(Name::new("Id")),
                    ],
                ),
                Some(("api::User", "Id")),
            ),
            (
                "absolute name path",
                path(
                    true,
                    vec![
                        PathSegment::Name(Name::new("api")),
                        PathSegment::Name(Name::new("User")),
                    ],
                ),
                Some(("::api", "User")),
            ),
            (
                "single segment path",
                path(false, vec![PathSegment::Name(Name::new("User"))]),
                None,
            ),
            (
                "final keyword path",
                path(
                    false,
                    vec![PathSegment::Name(Name::new("api")), PathSegment::SelfKw],
                ),
                None,
            ),
        ];

        for (label, path, expected) in cases {
            let actual = path
                .split_prefix_name()
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
    fn classifies_plain_identifier_paths() {
        let cases = [
            (
                "Self type",
                path(false, vec![PathSegment::Name(Name::new("Self"))]),
                false,
                true,
                true,
            ),
            (
                "self keyword",
                path(false, vec![PathSegment::SelfKw]),
                false,
                false,
                false,
            ),
            (
                "other plain ident",
                path(false, vec![PathSegment::Name(Name::new("User"))]),
                true,
                false,
                false,
            ),
            (
                "absolute Self",
                path(true, vec![PathSegment::Name(Name::new("Self"))]),
                false,
                false,
                false,
            ),
        ];

        for (label, path, is_user, is_self_ident, is_self_type) in cases {
            assert_eq!(path.is_plain_ident("User"), is_user, "{label}");
            assert_eq!(path.is_plain_ident("Self"), is_self_ident, "{label}");
            assert_eq!(path.is_self_type(), is_self_type, "{label}");
        }
    }

    fn path(absolute: bool, segments: Vec<PathSegment>) -> Path {
        Path { absolute, segments }
    }
}
