//! Edition-aware rendering for names, type syntax, and semantic paths.
//!
//! Lowering removes source-only rawness because it is not part of semantic name identity. These
//! borrowed adapters restore `r#` while writing identifiers, type refs, bounds, field keys, and
//! rooted paths into the caller's final output, so presentation does not need a second owned
//! spelling beside each canonical name.

use std::fmt;

use rg_ir_model::{Path, PathRoot};
use rg_item_tree::{
    FieldKey, TypeBound, TypeBoundListDisplay, TypeNameFormatter, TypeRef, TypeRefDisplay,
};
use rg_text::{Name, RustEdition};

/// Creates borrowed Rust-syntax display adapters for one use-site edition.
#[derive(Debug, Clone, Copy)]
pub struct SyntaxRenderer {
    edition: RustEdition,
}

impl SyntaxRenderer {
    pub fn new(edition: RustEdition) -> Self {
        Self { edition }
    }

    pub fn edition(self) -> RustEdition {
        self.edition
    }

    /// Displays a canonical identifier without allocating an intermediate string.
    pub fn identifier<'a>(self, name: &'a str) -> NameDisplay<'a> {
        self.name(name)
    }

    /// Displays a canonical semantic name, including lifetime and label names.
    pub fn name<'a>(self, name: &'a str) -> NameDisplay<'a> {
        NameDisplay {
            name,
            edition: self.syntax_edition(),
        }
    }

    pub fn field_key<'a>(self, key: &'a FieldKey) -> FieldKeyDisplay<'a> {
        FieldKeyDisplay {
            syntax: self,
            key,
            declaration: false,
        }
    }

    pub fn field_declaration_label<'a>(self, key: &'a FieldKey) -> FieldKeyDisplay<'a> {
        FieldKeyDisplay {
            syntax: self,
            key,
            declaration: true,
        }
    }

    pub fn type_ref<'a>(self, ty: &'a TypeRef) -> TypeRefDisplay<'a, Self> {
        ty.display_with(self)
    }

    pub fn type_bounds<'a>(self, bounds: &'a [TypeBound]) -> TypeBoundListDisplay<'a, Self> {
        TypeBound::display_list_with(bounds, self)
    }

    /// Displays a semantic path as valid source for the use-site edition.
    pub fn path<'a>(self, path: &'a Path) -> PathDisplay<'a> {
        PathDisplay { syntax: self, path }
    }

    fn syntax_edition(self) -> rg_syntax::Edition {
        match self.edition {
            RustEdition::Edition2015 => rg_syntax::Edition::Edition2015,
            RustEdition::Edition2018 => rg_syntax::Edition::Edition2018,
            RustEdition::Edition2021 => rg_syntax::Edition::Edition2021,
            RustEdition::Edition2024 => rg_syntax::Edition::Edition2024,
        }
    }
}

impl TypeNameFormatter for SyntaxRenderer {
    fn fmt_name(&self, name: &Name, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.name(name), f)
    }
}

/// Borrowed display of one canonical semantic name.
#[derive(Debug, Clone, Copy)]
pub struct NameDisplay<'a> {
    name: &'a str,
    edition: rg_syntax::Edition,
}

impl fmt::Display for NameDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (apostrophe, name) = match self.name.strip_prefix('\'') {
            Some(name) => (true, name),
            None => (false, self.name),
        };

        if apostrophe {
            f.write_str("'")?;
        }
        if name != "static" && rg_syntax::utils::is_raw_identifier(name, self.edition) {
            f.write_str("r#")?;
        }
        f.write_str(name)
    }
}

/// Borrowed display of one semantic path using edition-correct identifier spelling.
#[derive(Debug, Clone, Copy)]
pub struct PathDisplay<'a> {
    syntax: SyntaxRenderer,
    path: &'a Path,
}

impl fmt::Display for PathDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut needs_separator = match self.path.root() {
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

        for segment in self.path.segments() {
            if needs_separator {
                f.write_str("::")?;
            }
            fmt::Display::fmt(&self.syntax.identifier(segment.as_str()), f)?;
            needs_separator = true;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
pub struct FieldKeyDisplay<'a> {
    syntax: SyntaxRenderer,
    key: &'a FieldKey,
    declaration: bool,
}

impl fmt::Display for FieldKeyDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.key {
            FieldKey::Named(name) => write!(f, "{}", self.syntax.identifier(name)),
            FieldKey::Tuple(index) if self.declaration => write!(f, "#{index}"),
            FieldKey::Tuple(index) => write!(f, "{index}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use rg_ir_model::Mutability;
    use rg_item_tree::{TypeBound, TypeRef};
    use rg_text::{Name, RustEdition};

    use super::SyntaxRenderer;

    #[test]
    fn name_rendering_uses_the_use_site_edition_without_allocated_state() {
        let edition_2021 = SyntaxRenderer::new(RustEdition::Edition2021);
        let edition_2024 = SyntaxRenderer::new(RustEdition::Edition2024);

        assert_eq!(edition_2021.identifier("type").to_string(), "r#type");
        assert_eq!(edition_2021.identifier("gen").to_string(), "gen");
        assert_eq!(edition_2024.identifier("gen").to_string(), "r#gen");
        assert_eq!(edition_2024.identifier("Self").to_string(), "Self");
        assert_eq!(edition_2024.name("'fn").to_string(), "'r#fn");
        assert_eq!(edition_2024.name("'static").to_string(), "'static");

        let ty = TypeRef::Reference {
            lifetime: Some(Name::new("'fn")),
            mutability: Mutability::Shared,
            inner: Box::new(TypeRef::ImplTrait(vec![TypeBound::Lifetime(Name::new(
                "'gen",
            ))])),
        };
        assert_eq!(ty.to_string(), "&'fn impl 'gen");
        assert_eq!(edition_2024.type_ref(&ty).to_string(), "&'r#fn impl 'r#gen");
    }
}
