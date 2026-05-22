//! Parsing for `#[memsize(...)]` attributes.
//!
//! The derive has a deliberately small attribute language. Keeping parsing here makes the rest of
//! the macro read like expansion logic instead of a long stream of stringly configuration checks.

use proc_macro2::Span;
use syn::{Attribute, LitStr, Path, WherePredicate, parse_quote};

/// Type-level options collected from `#[memsize(...)]`.
#[derive(Default)]
pub(crate) struct ContainerAttrs {
    /// Treat the whole type as a shallow leaf.
    pub(crate) leaf: bool,
    crate_path: Option<Path>,
    /// Let callers provide all generic bounds by hand.
    pub(crate) no_auto_bound: bool,
    /// Extra where-clause predicates injected after auto-bounds.
    pub(crate) bounds: Vec<WherePredicate>,
}

impl ContainerAttrs {
    /// Reads all container-level `memsize` attributes from a derive input.
    pub(crate) fn parse(attrs: &[Attribute]) -> syn::Result<Self> {
        let mut parsed = Self::default();

        for attr in memsize_attrs(attrs) {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("leaf") {
                    set_bool_once(&mut parsed.leaf, &meta, "leaf")?;
                    return Ok(());
                }

                if meta.path.is_ident("crate_path") {
                    let lit: LitStr = meta.value()?.parse()?;
                    if parsed.crate_path.is_some() {
                        return Err(meta.error("duplicate `crate_path` memsize attribute"));
                    }
                    parsed.crate_path = Some(parse_path_literal(&lit)?);
                    return Ok(());
                }

                if meta.path.is_ident("no_auto_bound") {
                    set_bool_once(&mut parsed.no_auto_bound, &meta, "no_auto_bound")?;
                    return Ok(());
                }

                if meta.path.is_ident("bound") {
                    let lit: LitStr = meta.value()?.parse()?;
                    parsed.bounds.push(lit.parse()?);
                    return Ok(());
                }

                Err(meta.error("unsupported memsize container attribute"))
            })?;
        }

        Ok(parsed)
    }

    /// Returns the path used in generated trait and helper references.
    pub(crate) fn crate_path(&self) -> Path {
        self.crate_path
            .clone()
            .unwrap_or_else(|| parse_quote!(::rg_memsize))
    }
}

/// Per-field options that decide how one child is recorded.
#[derive(Default)]
pub(crate) struct FieldAttrs {
    pub(crate) skip: bool,
    pub(crate) inline: bool,
    pub(crate) scope: Option<LitStr>,
    pub(crate) with: Option<Path>,
}

impl FieldAttrs {
    /// Reads field-level `memsize` attributes and checks combinations that would be ambiguous.
    pub(crate) fn parse(attrs: &[Attribute]) -> syn::Result<Self> {
        let mut parsed = Self::default();

        for attr in memsize_attrs(attrs) {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("skip") {
                    set_bool_once(&mut parsed.skip, &meta, "skip")?;
                    return Ok(());
                }

                if meta.path.is_ident("inline") {
                    set_bool_once(&mut parsed.inline, &meta, "inline")?;
                    return Ok(());
                }

                if meta.path.is_ident("scope") {
                    let lit: LitStr = meta.value()?.parse()?;
                    if parsed.scope.is_some() {
                        return Err(meta.error("duplicate `scope` memsize attribute"));
                    }
                    parsed.scope = Some(lit);
                    return Ok(());
                }

                if meta.path.is_ident("with") {
                    let lit: LitStr = meta.value()?.parse()?;
                    if parsed.with.is_some() {
                        return Err(meta.error("duplicate `with` memsize attribute"));
                    }
                    parsed.with = Some(parse_path_literal(&lit)?);
                    return Ok(());
                }

                Err(meta.error("unsupported memsize field attribute"))
            })?;
        }

        parsed.validate()?;
        Ok(parsed)
    }

    /// Returns whether the field's type must implement `MemorySize` automatically.
    pub(crate) fn needs_auto_bound(&self) -> bool {
        !self.skip && self.with.is_none()
    }

    fn validate(&self) -> syn::Result<()> {
        if self.skip && (self.inline || self.scope.is_some() || self.with.is_some()) {
            return Err(syn::Error::new(
                Span::call_site(),
                "`skip` cannot be combined with other memsize field attributes",
            ));
        }

        if self.inline && self.scope.is_some() {
            return Err(syn::Error::new(
                Span::call_site(),
                "`inline` and `scope` cannot be combined",
            ));
        }

        Ok(())
    }
}

/// Per-variant options for enums.
#[derive(Default)]
pub(crate) struct VariantAttrs {
    pub(crate) skip: bool,
    pub(crate) scope: Option<LitStr>,
}

impl VariantAttrs {
    /// Reads variant-level `memsize` attributes.
    pub(crate) fn parse(attrs: &[Attribute]) -> syn::Result<Self> {
        let mut parsed = Self::default();

        for attr in memsize_attrs(attrs) {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("skip") {
                    set_bool_once(&mut parsed.skip, &meta, "skip")?;
                    return Ok(());
                }

                if meta.path.is_ident("scope") {
                    let lit: LitStr = meta.value()?.parse()?;
                    if parsed.scope.is_some() {
                        return Err(meta.error("duplicate `scope` memsize attribute"));
                    }
                    parsed.scope = Some(lit);
                    return Ok(());
                }

                Err(meta.error("unsupported memsize variant attribute"))
            })?;
        }

        if parsed.skip && parsed.scope.is_some() {
            return Err(syn::Error::new(
                Span::call_site(),
                "`skip` cannot be combined with `scope`",
            ));
        }

        Ok(parsed)
    }
}

/// Finds only the attributes owned by this derive.
fn memsize_attrs(attrs: &[Attribute]) -> impl Iterator<Item = &Attribute> {
    attrs.iter().filter(|attr| attr.path().is_ident("memsize"))
}

/// Marks a boolean option and reports duplicates at the attribute site.
fn set_bool_once(
    slot: &mut bool,
    meta: &syn::meta::ParseNestedMeta<'_>,
    name: &str,
) -> syn::Result<()> {
    if *slot {
        return Err(meta.error(format!("duplicate `{name}` memsize attribute")));
    }

    *slot = true;
    Ok(())
}

/// Parses a string literal as a Rust path, keeping any error anchored to the literal.
fn parse_path_literal(lit: &LitStr) -> syn::Result<Path> {
    lit.parse()
        .map_err(|error| syn::Error::new(lit.span(), error.to_string()))
}
