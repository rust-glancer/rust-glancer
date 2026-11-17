//! Lowered import facts used by both name resolution and source-site queries.
//!
//! Resolution needs a semantic `Path`, while cursor and completion queries need the exact source
//! location for each segment. `ImportPath` keeps both views together so macro expansion cannot rewrite
//! one path representation and leave another one stale.

use std::fmt;

use rg_ir_model::{CrateRef, ModuleId, Path, PathRoot, SourceId};
use rg_item_tree::{ImportAlias, UseImportKind, UsePath, UsePathSegmentKind, UserFacingAttrs};
use rg_std::{MemorySize, Shrink};
use rg_text::Name;
use wincode::{SchemaRead, SchemaWrite};

use crate::{ItemSource, binding::Visibility};

/// One lowered import declaration.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct ImportData {
    pub module: ModuleId,
    pub visibility: Visibility,
    /// Whether the source wrote this `use` as a re-export rather than a private binding.
    ///
    /// At a crate root, `use item` and `pub(crate) use item` have the same semantic visibility:
    /// both can be named by descendant modules. Import discovery still needs their source-level
    /// distinction so it does not build new imports through another module's private bindings.
    pub is_reexport: bool,
    pub kind: ImportKind,
    pub path: ImportPath,
    pub binding: ImportBinding,
    pub alias_location: Option<SourceId>,
    pub source: ItemSource,
    pub import_index: usize,
    /// Presentation attributes on the outer `use` item that exposed this route.
    pub user_facing_attrs: UserFacingAttrs,
}

impl ImportData {
    /// Preserve whether source deliberately exposed a `use` beyond a private binding.
    ///
    /// `pub(self)` and `pub(in self)` are still private to the declaring module. Invalid
    /// visibility syntax is also excluded because it does not establish a reusable route.
    pub fn source_visibility_reexports(visibility: &rg_item_tree::VisibilityLevel) -> bool {
        match visibility {
            rg_item_tree::VisibilityLevel::Public
            | rg_item_tree::VisibilityLevel::Crate
            | rg_item_tree::VisibilityLevel::Super => true,
            rg_item_tree::VisibilityLevel::Restricted(path) => path != "self",
            rg_item_tree::VisibilityLevel::Private
            | rg_item_tree::VisibilityLevel::Self_
            | rg_item_tree::VisibilityLevel::Unknown(_) => false,
        }
    }

    /// Returns the binding name introduced by this import when it is not a glob import.
    pub fn binding_name(&self) -> Option<Name> {
        let inferred_name = match self.kind {
            ImportKind::Named | ImportKind::SelfImport => self.path.semantic().last_name(),
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

/// One semantic import path paired with source spans for its root and ordinary segments.
///
/// The resolver reads `semantic`; source queries use `prefixes_with_locations`. Both projections stay
/// private so macro rebasing cannot accidentally make them disagree.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct ImportPath {
    semantic: Path,
    location: Option<SourceId>,
    root_locations: Vec<SourceId>,
    segment_locations: Vec<SourceId>,
}

impl ImportPath {
    /// Converts source import syntax into one semantic root plus ordinary name segments.
    ///
    /// Generated `$crate` syntax is lowered as a name by item-tree. It is accepted only when the
    /// macro expansion supplies the defining crate, so the intermediate semantic path never needs
    /// an unresolved `$crate` pseudo-segment.
    pub fn from_use_path(path: &UsePath, dollar_crate: Option<CrateRef>) -> Option<Self> {
        let (root, root_segment_count) = if path.absolute {
            (PathRoot::Absolute, 0)
        } else {
            match path.segments.first().map(|segment| &segment.kind) {
                Some(UsePathSegmentKind::CrateKw) => (PathRoot::Crate, 1),
                Some(UsePathSegmentKind::SelfKw) => (PathRoot::SelfModule, 1),
                Some(UsePathSegmentKind::SuperKw) => {
                    let count = path
                        .segments
                        .iter()
                        .take_while(|segment| matches!(segment.kind, UsePathSegmentKind::SuperKw))
                        .count();
                    (PathRoot::Super(u16::try_from(count).ok()?), count)
                }
                Some(UsePathSegmentKind::Name(name)) if name.as_str() == "$crate" => {
                    (PathRoot::DollarCrate(dollar_crate?), 1)
                }
                Some(UsePathSegmentKind::Name(_)) | None => (PathRoot::Relative, 0),
            }
        };

        let root_locations = path.segments[..root_segment_count]
            .iter()
            .map(|segment| segment.location)
            .collect();
        let mut segments = Vec::new();
        let mut segment_locations = Vec::new();
        for segment in &path.segments[root_segment_count..] {
            let UsePathSegmentKind::Name(name) = &segment.kind else {
                // Root keywords are grammatical only at the beginning. Refuse to encode recovered
                // invalid syntax as if it were an ordinary resolvable name.
                return None;
            };
            segments.push(name.clone());
            segment_locations.push(segment.location);
        }

        Some(Self {
            semantic: Path::new(root, segments),
            location: path.location,
            root_locations,
            segment_locations,
        })
    }

    pub fn semantic(&self) -> &Path {
        &self.semantic
    }

    pub fn location(&self) -> Option<SourceId> {
        self.location
    }

    /// Return every written root/name component as the semantic prefix ending at that component.
    ///
    /// For `super::super::api`, the two root tokens produce `super` and `super::super`; `api`
    /// produces the full path. Point queries can therefore navigate and complete rooted paths
    /// without pretending those tokens are ordinary name segments.
    pub fn prefixes_with_locations(&self) -> Option<Vec<(Path, SourceId)>> {
        if self.semantic.segments().len() != self.segment_locations.len() {
            return None;
        }

        let expected_root_spans = self.semantic.root().written_component_count();
        if self.root_locations.len() != expected_root_spans {
            return None;
        }

        let mut prefixes =
            Vec::with_capacity(self.root_locations.len() + self.segment_locations.len());
        match self.semantic.root() {
            PathRoot::Relative | PathRoot::Absolute => {}
            PathRoot::Crate | PathRoot::SelfModule | PathRoot::DollarCrate(_) => {
                prefixes.push((
                    Path::new(self.semantic.root(), Vec::new()),
                    self.root_locations[0],
                ));
            }
            PathRoot::Super(depth) => {
                for current_depth in 1..=depth {
                    prefixes.push((
                        Path::new(PathRoot::Super(current_depth), Vec::new()),
                        self.root_locations[usize::from(current_depth - 1)],
                    ));
                }
            }
        }

        for (index, location) in self.segment_locations.iter().copied().enumerate() {
            prefixes.push((self.semantic.prefix(index + 1)?, location));
        }
        Some(prefixes)
    }

    pub fn last_component_location(&self) -> Option<SourceId> {
        self.segment_locations
            .last()
            .copied()
            .or_else(|| self.root_locations.last().copied())
    }
}

impl fmt::Display for ImportPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.semantic.fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use rg_ir_model::SourceId;
    use rg_item_tree::{UsePath, UsePathSegment, UsePathSegmentKind};
    use rg_text::Name;

    use super::ImportPath;

    #[test]
    fn builds_semantic_paths_from_use_paths() {
        let cases = [
            (
                "crate root",
                use_path(
                    false,
                    &[
                        UsePathSegmentKind::CrateKw,
                        UsePathSegmentKind::Name(Name::new("api")),
                        UsePathSegmentKind::Name(Name::new("User")),
                    ],
                ),
                "crate::api::User",
            ),
            (
                "repeated super root",
                use_path(
                    false,
                    &[
                        UsePathSegmentKind::SuperKw,
                        UsePathSegmentKind::SuperKw,
                        UsePathSegmentKind::Name(Name::new("User")),
                    ],
                ),
                "super::super::User",
            ),
            (
                "absolute path",
                use_path(
                    true,
                    &[
                        UsePathSegmentKind::Name(Name::new("api")),
                        UsePathSegmentKind::Name(Name::new("User")),
                    ],
                ),
                "::api::User",
            ),
        ];

        for (label, path, expected) in cases {
            assert_eq!(
                ImportPath::from_use_path(&path, None)
                    .expect("valid use path")
                    .to_string(),
                expected,
                "{label}"
            );
        }
    }

    fn use_path(absolute: bool, kinds: &[UsePathSegmentKind]) -> UsePath {
        UsePath {
            location: Some(location()),
            absolute,
            segments: kinds
                .iter()
                .cloned()
                .map(|kind| UsePathSegment {
                    kind,
                    location: location(),
                })
                .collect(),
        }
    }

    fn location() -> SourceId {
        SourceId { owner: 0, local: 0 }
    }
}
