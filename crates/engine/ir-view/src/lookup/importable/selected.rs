//! Check a selected completion's saved path against the scope at the cursor.

use rg_def_map::{DefMapQuery, NameBinding, Namespace, NamespaceSet};
use rg_ir_model::{
    DefId, DefMapRef, ModuleId, ModuleRef, Path, PathRoot, SemanticItemRef, TraitDefRef,
    identity::LexicalScopeRef,
};
use rg_semantic_ir::ItemStoreQuery;

use super::{ImportPathKind, ImportableNameSearch, walk::module_is_exposed};
use crate::{SymbolKind, lookup::name::NameNamespace};

impl ImportableNameSearch<'_, '_> {
    /// Resolve the selected trait by its exported spelling, including re-export aliases.
    pub fn trait_for_import(
        &self,
        module: ModuleRef,
        path: &Path,
    ) -> anyhow::Result<Option<TraitDefRef>> {
        let bindings = self.selected_bindings(module, path, NamespaceSet::TYPES)?;
        let [(namespace, binding)] = bindings.as_slice() else {
            return Ok(None);
        };
        let (DefId::Local(def), Some(leaf)) = (binding.def, path.segments().last()) else {
            return Ok(None);
        };
        if self
            .binding_name(module, leaf, *namespace, binding)?
            .is_none()
        {
            return Ok(None);
        }
        Ok(
            match ItemStoreQuery::new(self.db).semantic_item_for_local_def(def)? {
                Some(SemanticItemRef::Trait(trait_ref)) => Some(trait_ref),
                _ => None,
            },
        )
    }

    /// Check the exact path against the current scope. A single use can bring in both
    /// a type and a constructor, so even a type completion has to check value conflicts.
    pub fn can_import_name(
        &self,
        module: ModuleRef,
        path: &Path,
        kind: ImportPathKind,
        body_scope: Option<LexicalScopeRef>,
        accepts: impl Fn(NameNamespace, SymbolKind) -> bool,
    ) -> anyhow::Result<bool> {
        let Some(label) = path.segments().last() else {
            return Ok(false);
        };
        let maps = DefMapQuery::new(self.db);
        let resolver = maps.name_resolver();
        let bindings = self.selected_bindings(module, path, NamespaceSet::TYPES_VALUES)?;
        let short_path = Path::unqualified_name(label.as_str());
        let occupied = resolver.import_bindings(module, &short_path, NamespaceSet::TYPES_VALUES)?;
        // For `struct Widget;`, the same use brings in the type and its constructor.
        // An existing `fn Widget()` would conflict even at `let _: Widget`.
        if bindings.iter().any(|(ns, incoming)| {
            occupied
                .iter()
                .any(|(other_ns, existing)| ns == other_ns && incoming.def != existing.def)
        }) {
            return Ok(false);
        }

        // The import must still add a name the user can reach at this cursor.
        for (ns, binding) in bindings {
            if !kind.namespaces().contains(ns)
                || occupied.iter().any(|(other_ns, _)| *other_ns == ns)
            {
                continue;
            }
            if let Some(scope) = body_scope {
                // A block's own use or item can hide the new module-level import.
                let lexical = ModuleRef {
                    origin: DefMapRef::Body(scope.body_ir()),
                    module: ModuleId(scope.scope_id().0),
                };
                if !resolver
                    .resolve_lexical_path(lexical, &short_path, [ns].into_iter().collect())?
                    .resolved
                    .is_empty()
                {
                    continue;
                }
            }
            if let Some(name) = self.binding_name(module, label, ns, &binding)?
                && accepts(name.namespace(), name.kind())
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Follow just the saved route. A public item can remain reachable in Rust even after
    /// a dependency marks an intermediate module or its re-export as hidden.
    fn selected_bindings(
        &self,
        importing_module: ModuleRef,
        path: &Path,
        namespaces: NamespaceSet,
    ) -> anyhow::Result<Vec<(Namespace, NameBinding)>> {
        let Some((leaf, parents)) = path.segments().split_last() else {
            return Ok(Vec::new());
        };
        // The search starts at self, crate, or an extern crate. Resolve that same root
        // first so an absolute extern path still works when its name is shadowed.
        let root_len = usize::from(matches!(
            path.root(),
            PathRoot::Relative | PathRoot::Absolute
        ));
        let Some((root, parents)) = parents.split_at_checked(root_len) else {
            return Ok(Vec::new());
        };
        let maps = DefMapQuery::new(self.db);
        let resolver = maps.name_resolver();
        let Some(mut module) =
            resolver.import_module(importing_module, &Path::new(path.root(), root.to_vec()))?
        else {
            return Ok(Vec::new());
        };
        for parent in parents {
            let next = resolver.bindings_in_module(
                importing_module,
                module,
                parent.as_str(),
                NamespaceSet::TYPES,
            )?;
            let [(_, binding)] = next.as_slice() else {
                return Ok(Vec::new());
            };
            let DefId::Module(child) = binding.def else {
                return Ok(Vec::new());
            };
            if !module_is_exposed(self.db, importing_module, binding, child)? {
                return Ok(Vec::new());
            }
            module = child;
        }
        Ok(resolver.bindings_in_module(importing_module, module, leaf.as_str(), namespaces)?)
    }
}
