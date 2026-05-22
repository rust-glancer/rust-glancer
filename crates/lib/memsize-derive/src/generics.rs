//! Generic-bound handling for the derive expansion.
//!
//! The macro should add `T: MemorySize` when a recorded field actually depends on `T`, but it
//! should stay out of the way for skipped fields and custom recorders. This module keeps that
//! bookkeeping separate from the token-generation flow.

use std::collections::{BTreeMap, BTreeSet};

use syn::{
    Generics, Ident, Path, Type, WherePredicate, parse_quote,
    visit::{self, Visit},
};

/// Adds explicit `#[memsize(bound = "...")]` predicates to the output generics.
pub(crate) fn add_configured_bounds(generics: &mut Generics, bounds: Vec<WherePredicate>) {
    if bounds.is_empty() {
        return;
    }

    let where_clause = generics.make_where_clause();
    where_clause.predicates.extend(bounds);
}

/// Adds `MemorySize` bounds for type parameters found in recorded fields.
pub(crate) fn add_auto_bounds(
    generics: &mut Generics,
    original_generics: &Generics,
    crate_path: &Path,
    bound_types: &[Type],
) {
    // Only fields recorded through `MemorySize` need automatic bounds. Skipped fields and
    // custom `with` recorders can carry unconstrained type parameters.
    let type_params = original_generics
        .type_params()
        .map(|param| (param.ident.to_string(), param.ident.clone()))
        .collect::<BTreeMap<_, _>>();
    if type_params.is_empty() {
        return;
    }

    let mut collector = TypeParamCollector {
        type_params: &type_params,
        used: BTreeSet::new(),
    };

    for ty in bound_types {
        collector.visit_type(ty);
    }

    let where_clause = generics.make_where_clause();
    for param_name in collector.used {
        let ident = type_params
            .get(&param_name)
            .expect("collector only stores known type parameters");
        where_clause
            .predicates
            .push(parse_quote!(#ident: #crate_path::MemorySize));
    }
}

/// Finds generic type parameters mentioned inside recorded field types.
struct TypeParamCollector<'a> {
    type_params: &'a BTreeMap<String, Ident>,
    used: BTreeSet<String>,
}

impl<'ast> Visit<'ast> for TypeParamCollector<'_> {
    fn visit_type_path(&mut self, node: &'ast syn::TypePath) {
        if node.qself.is_none() && node.path.segments.len() == 1 {
            if let Some(segment) = node.path.segments.first() {
                let ident = segment.ident.to_string();
                if self.type_params.contains_key(&ident) {
                    self.used.insert(ident);
                }
            }
        }

        visit::visit_type_path(self, node);
    }
}
