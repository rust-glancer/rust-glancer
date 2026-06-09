//! Parsing for `#[shrink(...)]` attributes.

use proc_macro2::Span;
use syn::{Attribute, LitStr, Path, WherePredicate, parse_quote};

/// Type-level options collected from `#[shrink(...)]`.
#[derive(Default)]
pub(crate) struct ContainerAttrs {
    /// Treat the whole type as a leaf with no child storage to compact.
    pub(crate) leaf: bool,
    crate_path: Option<Path>,
    /// Let callers provide all generic bounds by hand.
    pub(crate) no_auto_bound: bool,
    /// Extra where-clause predicates injected after auto-bounds.
    pub(crate) bounds: Vec<WherePredicate>,
}

impl ContainerAttrs {
    /// Reads all container-level `shrink` attributes from a derive input.
    pub(crate) fn parse(attrs: &[Attribute]) -> syn::Result<Self> {
        let mut parsed = Self::default();

        for attr in shrink_attrs(attrs) {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("leaf") {
                    set_bool_once(&mut parsed.leaf, &meta, "leaf")?;
                    return Ok(());
                }

                if meta.path.is_ident("crate_path") {
                    let lit: LitStr = meta.value()?.parse()?;
                    if parsed.crate_path.is_some() {
                        return Err(meta.error("duplicate `crate_path` shrink attribute"));
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

                Err(meta.error("unsupported shrink container attribute"))
            })?;
        }

        Ok(parsed)
    }

    /// Returns the path used in generated trait references.
    pub(crate) fn crate_path(&self) -> Path {
        self.crate_path
            .clone()
            .unwrap_or_else(|| parse_quote!(::rg_std))
    }
}

/// Per-field options that decide how one child is compacted.
#[derive(Default)]
pub(crate) struct FieldAttrs {
    pub(crate) skip: bool,
    pub(crate) with: Option<Path>,
}

impl FieldAttrs {
    /// Reads field-level `shrink` attributes and checks ambiguous combinations.
    pub(crate) fn parse(attrs: &[Attribute]) -> syn::Result<Self> {
        let mut parsed = Self::default();

        for attr in shrink_attrs(attrs) {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("skip") {
                    set_bool_once(&mut parsed.skip, &meta, "skip")?;
                    return Ok(());
                }

                if meta.path.is_ident("with") {
                    let lit: LitStr = meta.value()?.parse()?;
                    if parsed.with.is_some() {
                        return Err(meta.error("duplicate `with` shrink attribute"));
                    }
                    parsed.with = Some(parse_path_literal(&lit)?);
                    return Ok(());
                }

                Err(meta.error("unsupported shrink field attribute"))
            })?;
        }

        parsed.validate()?;
        Ok(parsed)
    }

    /// Returns whether the field's type must implement `Shrink` automatically.
    pub(crate) fn needs_auto_bound(&self) -> bool {
        !self.skip && self.with.is_none()
    }

    fn validate(&self) -> syn::Result<()> {
        if self.skip && self.with.is_some() {
            return Err(syn::Error::new(
                Span::call_site(),
                "`skip` cannot be combined with other shrink field attributes",
            ));
        }

        Ok(())
    }
}

/// Per-variant options for enums.
#[derive(Default)]
pub(crate) struct VariantAttrs {
    pub(crate) skip: bool,
}

impl VariantAttrs {
    /// Reads variant-level `shrink` attributes.
    pub(crate) fn parse(attrs: &[Attribute]) -> syn::Result<Self> {
        let mut parsed = Self::default();

        for attr in shrink_attrs(attrs) {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("skip") {
                    set_bool_once(&mut parsed.skip, &meta, "skip")?;
                    return Ok(());
                }

                Err(meta.error("unsupported shrink variant attribute"))
            })?;
        }

        Ok(parsed)
    }
}

/// Finds only the attributes owned by this derive.
fn shrink_attrs(attrs: &[Attribute]) -> impl Iterator<Item = &Attribute> {
    attrs.iter().filter(|attr| attr.path().is_ident("shrink"))
}

/// Marks a boolean option and reports duplicates at the attribute site.
fn set_bool_once(
    slot: &mut bool,
    meta: &syn::meta::ParseNestedMeta<'_>,
    name: &str,
) -> syn::Result<()> {
    if *slot {
        return Err(meta.error(format!("duplicate `{name}` shrink attribute")));
    }

    *slot = true;
    Ok(())
}

/// Parses a string literal as a Rust path, keeping any error anchored to the literal.
fn parse_path_literal(lit: &LitStr) -> syn::Result<Path> {
    lit.parse()
        .map_err(|error| syn::Error::new(lit.span(), error.to_string()))
}
