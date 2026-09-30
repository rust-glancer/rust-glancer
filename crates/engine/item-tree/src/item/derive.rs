//! The small part of derive attributes needed after the syntax tree is released.
//!
//! Keep paths here rather than deciding that a spelling such as `Clone` is a builtin. Imports can
//! rename a builtin or bring a different derive with that name into scope.

use rg_cfg_eval::{CfgExpr, CfgGate, CfgPredicate};
use rg_ir_model::{CrateRef, Path, Span};
use rg_std::{MemorySize, Shrink};
use rg_syntax::{AstNode as _, T, ast};
use wincode::{SchemaRead, SchemaWrite};

/// Construction-only attributes for derives on one struct or enum.
///
/// Field and variant payloads are also retained in Semantic IR. Keep derive-only cfg metadata
/// here so every resident field does not pay for an attribute container after construction.
#[derive(Debug, Clone, Default, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct DeriveAttrs {
    pub calls: Vec<DeriveMacroCall>,
    /// Sparse cfg gates indexed by field order, flattened across enum variants.
    /// Ungated fields have no entry but still take an index. Enum fields include their variant's
    /// gates, so a disabled variant cannot add trait bounds through its fields.
    pub field_cfg: Vec<(usize, CfgExpr)>,
    /// Conditions under which unit variants can serve as `#[default]`.
    /// Expansion counts enabled entries to require exactly one default variant.
    pub default_variants: Vec<CfgExpr>,
}

impl DeriveAttrs {
    pub(crate) fn from_struct(item: &ast::Struct, dollar_crate: Option<CrateRef>) -> Self {
        let mut attrs = Self {
            calls: DeriveMacroCall::from_attrs(item, dollar_crate),
            ..Self::default()
        };
        if !attrs.calls.is_empty() {
            attrs.collect_field_cfg(item.field_list(), &CfgExpr::default(), &mut 0);
        }
        attrs
    }

    pub(crate) fn from_enum(item: &ast::Enum, dollar_crate: Option<CrateRef>) -> Self {
        let mut attrs = Self {
            calls: DeriveMacroCall::from_attrs(item, dollar_crate),
            ..Self::default()
        };
        if !attrs.calls.is_empty()
            && let Some(variants) = item.variant_list()
        {
            let mut next_field = 0;
            for variant in variants.variants() {
                if let Some(default) = DeriveMacroCall::default_variant_cfg(&variant) {
                    attrs.default_variants.push(default);
                }
                attrs.collect_field_cfg(
                    variant.field_list(),
                    &CfgExpr::from_attrs(&variant),
                    &mut next_field,
                );
            }
        }
        attrs
    }

    /// Keep the conditions that decide which fields can contribute bounds to a derived impl.
    fn collect_field_cfg(
        &mut self,
        fields: Option<ast::FieldList>,
        parent: &CfgExpr,
        next_field: &mut usize,
    ) {
        let fields: Vec<CfgExpr> = match fields {
            Some(ast::FieldList::RecordFieldList(fields)) => fields
                .fields()
                .map(|field| CfgExpr::from_attrs(&field))
                .collect(),
            Some(ast::FieldList::TupleFieldList(fields)) => fields
                .fields()
                .map(|field| CfgExpr::from_attrs(&field))
                .collect(),
            None => Vec::new(),
        };
        for mut cfg in fields {
            cfg.gates.extend(parent.gates.iter().cloned());
            if !cfg.gates.is_empty() {
                self.field_cfg.push((*next_field, cfg));
            }
            *next_field += 1;
        }
    }
}

/// One path from a `#[derive(...)]` attribute, before macro name resolution.
///
/// `#[cfg_attr(feature = "extra", derive(Clone, Debug))]` produces two calls with the same
/// condition. Keep it unevaluated so each crate target can decide whether to use those calls.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct DeriveMacroCall {
    pub path: Path,
    pub span: Span,
    pub predicate: Option<CfgPredicate>,
}

impl DeriveMacroCall {
    pub(crate) fn from_attrs(
        item: &impl ast::HasAttrs,
        dollar_crate: Option<CrateRef>,
    ) -> Vec<Self> {
        let mut calls = Vec::new();
        Self::visit_attrs(item, |meta, predicate| {
            let Some((name, tokens)) = meta.as_simple_call() else {
                return;
            };
            if name != "derive" {
                return;
            }
            calls.extend(
                Self::parse_paths(tokens, dollar_crate).map(|(path, span)| Self {
                    path,
                    span,
                    predicate: predicate.clone(),
                }),
            );
        });
        calls
    }

    /// Build the condition under which this variant can be chosen by a `Default` derive.
    ///
    /// Default derives select a unit variant. Fields on the other variants impose no new generic
    /// bounds, but both the marker and the chosen variant itself can be cfg-gated.
    pub(crate) fn default_variant_cfg(variant: &ast::Variant) -> Option<CfgExpr> {
        if variant.field_list().is_some() {
            return None;
        }
        let mut defaults = Vec::new();
        let mut non_exhaustive = Vec::new();
        Self::visit_attrs(variant, |meta, predicate| {
            match meta.as_simple_atom().as_deref() {
                Some("default") => defaults.push(predicate.unwrap_or(CfgPredicate::True)),
                Some("non_exhaustive") => {
                    non_exhaustive.push(predicate.unwrap_or(CfgPredicate::True))
                }
                _ => {}
            }
        });
        if defaults.is_empty() {
            return None;
        }
        // A cfg-disabled marker behaves as if it were absent. Require an active `#[default]`
        // and exclude an active `#[non_exhaustive]`, in addition to the variant's own cfg.
        let mut cfg = CfgExpr::from_attrs(variant);
        cfg.gates.push(CfgGate::Direct(CfgPredicate::Any(defaults)));
        if !non_exhaustive.is_empty() {
            cfg.gates
                .push(CfgGate::Direct(CfgPredicate::Not(vec![CfgPredicate::Any(
                    non_exhaustive,
                )])));
        }
        Some(cfg)
    }

    /// Read outer attributes together with the `cfg_attr` conditions that guard them.
    fn visit_attrs(
        item: &impl ast::HasAttrs,
        mut visit: impl FnMut(ast::Meta, Option<CfgPredicate>),
    ) {
        // TODO: Extract attribute-parsing machinery and such (check r-a for references).
        for attr in item.attrs().filter(|attr| attr.kind().is_outer()) {
            let Some(meta) = attr.meta() else { continue };
            Self::visit_meta(meta, None, &mut visit);
        }
    }

    /// Unwrap nested `cfg_attr`s while keeping the conditions that reach each inner attribute.
    /// For `cfg_attr(a, cfg_attr(b, derive(Clone)))`, the callback receives `derive(Clone)`
    /// guarded by `all(a, b)`.
    fn visit_meta(
        meta: ast::Meta,
        predicate: Option<CfgPredicate>,
        visit: &mut impl FnMut(ast::Meta, Option<CfgPredicate>),
    ) {
        if let ast::Meta::CfgAttrMeta(meta) = meta {
            let nested_predicate = meta
                .cfg_predicate()
                .map(CfgPredicate::from_ast)
                .unwrap_or(CfgPredicate::Invalid);
            let predicate = match predicate {
                Some(parent) => CfgPredicate::All(vec![parent, nested_predicate]),
                None => nested_predicate,
            };
            for nested in meta.metas() {
                Self::visit_meta(nested, Some(predicate.clone()), visit);
            }
        } else {
            visit(meta, predicate);
        }
    }

    /// Syntax exposes the arguments of `#[derive(...)]` as a token tree.
    /// A derive list contains paths separated by commas. Reading tokens skips comments
    /// without losing the original range of each path, including a renamed import.
    /// The expansion's defining crate gives a path like `$crate::Duplicate` its root;
    /// ordinary source lowering has no such origin.
    fn parse_paths(
        tokens: ast::TokenTree,
        dollar_crate: Option<CrateRef>,
    ) -> impl Iterator<Item = (Path, Span)> {
        let mut path_text = String::new();
        let mut range = None::<rg_syntax::TextRange>;
        tokens
            .syntax()
            .children_with_tokens()
            .filter_map(|it| it.into_token())
            .filter_map(move |token| {
                if token.kind().is_trivia() || token.kind() == T!['('] {
                    return None;
                }
                if matches!(token.kind(), T![,] | T![')']) {
                    let path = range.take().and_then(|range| {
                        Path::from_macro_path_text(&path_text, dollar_crate)
                            .map(|path| (path, Span::from_text_range(range)))
                    });
                    path_text.clear();
                    path
                } else {
                    range = Some(
                        range.map_or(token.text_range(), |range| range.cover(token.text_range())),
                    );
                    path_text.push_str(token.text());
                    None
                }
            })
    }
}

/// The derive implemented by a macro definition marked `#[rustc_builtin_macro]`.
/// This is attached to the definition, so an imported alias keeps the same builtin kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[memsize(leaf)]
#[shrink(leaf)]
pub enum BuiltinDeriveKind {
    Clone,
    Copy,
    Debug,
    Default,
    Hash,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
}

impl BuiltinDeriveKind {
    pub(crate) fn from_macro_name(name: &str) -> Option<Self> {
        Some(match name {
            "Clone" => Self::Clone,
            "Copy" => Self::Copy,
            "Debug" => Self::Debug,
            "Default" => Self::Default,
            "Hash" => Self::Hash,
            "PartialEq" => Self::PartialEq,
            "Eq" => Self::Eq,
            "PartialOrd" => Self::PartialOrd,
            "Ord" => Self::Ord,
            _ => return None,
        })
    }
}
