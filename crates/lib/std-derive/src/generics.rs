//! Generic-bound handling for the derive expansion.
//!
//! Derives should add trait bounds only when generated field traversal actually needs them.
//! Skipped fields and custom handlers can carry unconstrained type parameters, so this module keeps
//! that bookkeeping separate from each derive's token-generation flow.

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

/// Adds trait bounds for type parameters found in generated field traversal.
pub(crate) fn add_auto_bounds(
    generics: &mut Generics,
    original_generics: &Generics,
    crate_path: &Path,
    trait_name: &Ident,
    bound_types: &[Type],
) {
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
            .push(parse_quote!(#ident: #crate_path::#trait_name));
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
