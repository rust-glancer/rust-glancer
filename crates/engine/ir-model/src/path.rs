//! Compact paths used by DefMap name resolution.
//!
//! Rust path roots are semantic instructions rather than ordinary names. This module separates
//! the root from the identifier segments so later queries cannot accidentally resolve `crate`,
//! `super`, or `$crate` as declarations in the middle of a path.
//!
//! ```text
//! api::User              -> Relative + [api, User]
//! crate::api::User       -> Crate    + [api, User]
//! super::super::User     -> Super(2) + [User]
//! ::std::vec::Vec        -> Absolute + [std, vec, Vec]
//! ```
//!
//! Source IRs that need generic arguments or `<T as Trait>` anchors keep richer path types and
//! project into this representation only for the name-resolution portion of their work.

use std::fmt;

use wincode::{SchemaRead, SchemaWrite};

use rg_std::{MemorySize, Shrink};
use rg_text::{Name, NameInterner, RustEdition};

use crate::CrateRef;

/// The semantic starting point of a DefMap path.
///
/// Rust only permits these forms at the beginning of a path. Keeping them out of `segments`
/// prevents impossible states such as `foo::$crate::bar` and lets every following segment be an
/// ordinary identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub enum PathRoot {
    /// Ordinary lexical lookup, as in `api::User`.
    Relative,
    /// The extern prelude selected by a leading `::`.
    Absolute,
    /// The current crate root selected by `crate`.
    Crate,
    /// The current module selected by `self`.
    SelfModule,
    /// One or more parent-module steps selected by leading `super` segments.
    Super(u16),
    /// The defining crate of a declarative macro expansion.
    #[memsize(skip)]
    DollarCrate(CrateRef),
}

impl PathRoot {
    pub fn is_relative(self) -> bool {
        matches!(self, Self::Relative)
    }

    pub fn is_absolute(self) -> bool {
        matches!(self, Self::Absolute)
    }

    /// Number of named root components written before ordinary path segments.
    ///
    /// Leading `::` selects a lookup root but is not itself a component. `super` retains its
    /// written depth because each token occupies one position in source-facing path accounting.
    pub fn written_component_count(self) -> usize {
        match self {
            Self::Relative | Self::Absolute => 0,
            Self::Crate | Self::SelfModule | Self::DollarCrate(_) => 1,
            Self::Super(depth) => usize::from(depth),
        }
    }
}

/// Structured path used by DefMap path-resolution queries.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct Path {
    root: PathRoot,
    segments: Vec<Name>,
}

impl Path {
    pub fn new(root: PathRoot, segments: Vec<Name>) -> Self {
        debug_assert!(!matches!(root, PathRoot::Super(0)));
        Self { root, segments }
    }

    /// Builds a single-segment relative path for ordinary lexical lookup.
    pub fn unqualified_name(name: impl AsRef<str>) -> Self {
        Self::new(PathRoot::Relative, vec![Name::new(name.as_ref())])
    }

    pub fn relative(segments: Vec<Name>) -> Self {
        Self::new(PathRoot::Relative, segments)
    }

    pub fn absolute(segments: Vec<Name>) -> Self {
        Self::new(PathRoot::Absolute, segments)
    }

    pub fn standard_prelude(
        crate_name: &'static str,
        edition: RustEdition,
        interner: &mut NameInterner,
    ) -> Self {
        Self::absolute(vec![
            interner.intern(crate_name),
            interner.intern("prelude"),
            interner.intern(edition.prelude_module()),
        ])
    }

    pub fn crate_relative_standard_prelude(
        edition: RustEdition,
        interner: &mut NameInterner,
    ) -> Self {
        Self::relative(vec![
            interner.intern("prelude"),
            interner.intern(edition.prelude_module()),
        ])
    }

    /// Converts syntax-like names into one root plus ordinary name segments.
    ///
    /// This is used by source IRs that retain keyword path segments. A keyword after the root is
    /// rejected because it cannot be represented honestly as an ordinary semantic segment.
    pub fn from_syntax_names(absolute: bool, names: Vec<Name>) -> Option<Self> {
        let labels: Vec<&str> = names.iter().map(Name::as_str).collect();
        let (root, consumed) = Self::classify_root(absolute, &labels, None)?;
        let segments = names.into_iter().skip(consumed).collect();
        Some(Self::new(root, segments))
    }

    /// Parses the textual callee path stored in item-tree or AST macro-call data.
    ///
    /// `$crate` only has meaning after resolution has selected the macro definition crate. Callers
    /// without that origin pass `None`, and the path is rejected instead of being guessed from the
    /// call site.
    pub fn from_macro_path_text(path: &str, dollar_crate: Option<CrateRef>) -> Option<Self> {
        let path = path.trim();
        let absolute = path.starts_with("::");
        let path = path.strip_prefix("::").unwrap_or(path);
        let labels: Vec<&str> = path.split("::").map(str::trim).collect();
        if labels.is_empty() || labels.iter().any(|segment| segment.is_empty()) {
            return None;
        }

        let (root, consumed) = Self::classify_root(absolute, &labels, dollar_crate)?;
        let segments = labels[consumed..].iter().map(Name::new).collect();
        Some(Self::new(root, segments))
    }

    /// Classifies only the leading syntax components as a path root.
    fn classify_root(
        absolute: bool,
        labels: &[&str],
        dollar_crate: Option<CrateRef>,
    ) -> Option<(PathRoot, usize)> {
        if absolute {
            if labels
                .first()
                .is_some_and(|label| Self::is_root_keyword(label))
            {
                return None;
            }
            return Some((PathRoot::Absolute, 0));
        }

        let (root, consumed) = match labels.first().copied() {
            Some("crate") => (PathRoot::Crate, 1),
            Some("self") => (PathRoot::SelfModule, 1),
            Some("$crate") => (PathRoot::DollarCrate(dollar_crate?), 1),
            Some("super") => {
                let depth = labels.iter().take_while(|label| **label == "super").count();
                let depth = u16::try_from(depth).ok()?;
                (PathRoot::Super(depth), usize::from(depth))
            }
            Some(_) => (PathRoot::Relative, 0),
            None => return None,
        };

        if labels[consumed..]
            .iter()
            .any(|label| Self::is_root_keyword(label))
        {
            return None;
        }
        Some((root, consumed))
    }

    fn is_root_keyword(label: &str) -> bool {
        matches!(label, "crate" | "self" | "super" | "$crate")
    }

    pub fn root(&self) -> PathRoot {
        self.root
    }

    pub fn segments(&self) -> &[Name] {
        &self.segments
    }

    /// Number of written root and ordinary name components in this path.
    pub fn component_count(&self) -> usize {
        self.root.written_component_count() + self.segments.len()
    }

    pub fn is_relative(&self) -> bool {
        self.root.is_relative()
    }

    pub fn is_absolute(&self) -> bool {
        self.root.is_absolute()
    }

    /// Whether this path has neither a semantic root nor an ordinary segment.
    pub fn is_empty(&self) -> bool {
        matches!(self.root, PathRoot::Relative | PathRoot::Absolute) && self.segments.is_empty()
    }

    /// Keep the same root and the first `segment_count` ordinary name segments.
    pub fn prefix(&self, segment_count: usize) -> Option<Self> {
        (segment_count <= self.segments.len())
            .then(|| Self::new(self.root, self.segments[..segment_count].to_vec()))
    }

    pub fn last_name(&self) -> Option<Name> {
        self.segments
            .last()
            .cloned()
            .or_else(|| self.root_label().map(Name::new))
    }

    /// Returns the name for a path that is exactly one relative named segment.
    pub fn relative_single_name(&self) -> Option<&Name> {
        if !self.is_relative() || self.segments.len() != 1 {
            return None;
        }
        self.segments.first()
    }

    /// Returns the name of a single-segment relative path that can participate in local lookup.
    pub fn single_name(&self) -> Option<&str> {
        self.relative_single_name().map(Name::as_str)
    }

    pub fn is_self_type(&self) -> bool {
        self.is_plain_ident("Self")
    }

    pub fn is_plain_ident(&self, ident: &str) -> bool {
        self.is_relative()
            && self.segments.len() == 1
            && self.segments.first().is_some_and(|name| name == ident)
    }

    pub fn last_segment_label(&self) -> Option<String> {
        self.last_name().map(|name| name.to_string())
    }

    /// Splits the outermost `prefix::name` shape into `prefix` and `name`.
    ///
    /// A rooted path needs only one ordinary segment (`crate::User`) because its root is already a
    /// meaningful prefix. A relative path still needs at least two (`api::User`).
    pub fn split_prefix_name(&self) -> Option<(Self, &str)> {
        let minimum_segments = usize::from(self.is_relative()) + 1;
        if self.segments.len() < minimum_segments {
            return None;
        }

        let last_segment = self.segments.last()?;
        Some((
            Self::new(self.root, self.segments[..self.segments.len() - 1].to_vec()),
            last_segment.as_str(),
        ))
    }

    fn root_label(&self) -> Option<&'static str> {
        match self.root {
            PathRoot::Relative | PathRoot::Absolute => None,
            PathRoot::Crate => Some("crate"),
            PathRoot::SelfModule => Some("self"),
            PathRoot::Super(_) => Some("super"),
            PathRoot::DollarCrate(_) => Some("$crate"),
        }
    }
}

impl fmt::Display for Path {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut needs_separator = match self.root {
            PathRoot::Relative => false,
            PathRoot::Absolute => {
                f.write_str("::")?;
                false
            }
            PathRoot::Crate => {
                f.write_str("crate")?;
                true
            }
            PathRoot::SelfModule => {
                f.write_str("self")?;
                true
            }
            PathRoot::Super(depth) => {
                for index in 0..depth {
                    if index > 0 {
                        f.write_str("::")?;
                    }
                    f.write_str("super")?;
                }
                true
            }
            PathRoot::DollarCrate(_) => {
                f.write_str("$crate")?;
                true
            }
        };

        for segment in &self.segments {
            if needs_separator {
                f.write_str("::")?;
            }
            write!(f, "{segment}")?;
            needs_separator = true;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use rg_text::Name;

    use super::{Path, PathRoot};

    #[test]
    fn recognizes_unqualified_names() {
        let cases = [
            (
                "plain name",
                path(PathRoot::Relative, &["User"]),
                Some("User"),
                true,
                false,
            ),
            (
                "Self type",
                path(PathRoot::Relative, &["Self"]),
                Some("Self"),
                false,
                true,
            ),
            (
                "self root",
                path(PathRoot::SelfModule, &[]),
                None,
                false,
                false,
            ),
            (
                "super root",
                path(PathRoot::Super(1), &[]),
                None,
                false,
                false,
            ),
            ("crate root", path(PathRoot::Crate, &[]), None, false, false),
            (
                "absolute name",
                path(PathRoot::Absolute, &["User"]),
                None,
                false,
                false,
            ),
            (
                "absolute Self",
                path(PathRoot::Absolute, &["Self"]),
                None,
                false,
                false,
            ),
            (
                "qualified name",
                path(PathRoot::Relative, &["api", "User"]),
                None,
                false,
                false,
            ),
        ];

        for (label, path, single_name, is_user, is_self) in cases {
            assert_eq!(path.single_name(), single_name, "{label}");
            assert_eq!(path.is_plain_ident("User"), is_user, "{label}");
            assert_eq!(path.is_plain_ident("Self"), is_self, "{label}");
            assert_eq!(path.is_self_type(), is_self, "{label}");
        }
    }

    #[test]
    fn splits_prefix_from_final_name_segment() {
        let cases = [
            (
                "relative name path",
                path(PathRoot::Relative, &["api", "User"]),
                Some((path(PathRoot::Relative, &["api"]), "User")),
            ),
            (
                "nested name path",
                path(PathRoot::Relative, &["api", "User", "Id"]),
                Some((path(PathRoot::Relative, &["api", "User"]), "Id")),
            ),
            (
                "absolute name path",
                path(PathRoot::Absolute, &["api", "User"]),
                Some((path(PathRoot::Absolute, &["api"]), "User")),
            ),
            (
                "rooted name path",
                path(PathRoot::Crate, &["User"]),
                Some((path(PathRoot::Crate, &[]), "User")),
            ),
            (
                "single relative segment",
                path(PathRoot::Relative, &["User"]),
                None,
            ),
        ];

        for (label, path, expected) in cases {
            assert_eq!(path.split_prefix_name(), expected, "{label}");
        }
    }

    #[test]
    fn classifies_and_validates_syntax_path_roots() {
        assert!(Path::from_syntax_names(false, names(&["foo", "super", "Bar"])).is_none());
        assert!(Path::from_syntax_names(true, names(&["crate", "Bar"])).is_none());
        assert_eq!(
            Path::from_syntax_names(false, names(&["super", "super", "Bar"])),
            Some(path(PathRoot::Super(2), &["Bar"]))
        );
    }

    fn path(root: PathRoot, segments: &[&str]) -> Path {
        Path::new(root, names(segments))
    }

    fn names(segments: &[&str]) -> Vec<Name> {
        segments.iter().map(Name::new).collect()
    }
}
