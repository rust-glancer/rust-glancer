//! Reuse lowered declaration data across solver callbacks in one body or standalone query.
//!
//! The types in this cache belong to the operation's interner and are dropped with it. Metadata,
//! headers, signatures, predicates, and type values each have their own entry, so a header read
//! during name lookup does not also load the predicates that requested the lookup.
//!
//! Lowering may return fallback data after a nested callback fails, such as a signature containing
//! an unknown type. Do not cache that result: a later cache hit would reuse the fallback without
//! reporting the failure that produced it.

use std::sync::Arc;

use rg_ir_model::{FunctionRef, GenericParamRef, ImplRef, TypeAliasRef, TypeDefRef};
use rustc_type_ir::{self as ir, Upcast, data_structures::HashMap, inherent::GenericArgs as _};

use super::SolverInterner;
use crate::solver::{
    CallableSignature, Clause, DeclarationKind, DeclarationMetadata, DefId, GenericArgs,
    ImplHeader, List, ParamEnv, Ty, types::Generics,
};

#[derive(Default)]
pub(crate) struct WorkingDeclarations<'s> {
    metadata: HashMap<DefId, Arc<DeclarationMetadata>>,
    generics: HashMap<DefId, Generics<'s>>,
    impl_headers: HashMap<ImplRef, ImplHeader<'s>>,
    functions: HashMap<FunctionRef, CallableSignature<'s>>,
    predicates: HashMap<DefId, List<'s, Clause<'s>>>,
    bounds: HashMap<DefId, List<'s, Clause<'s>>>,
    alias_values: HashMap<TypeAliasRef, Ty<'s>>,
    fields: HashMap<TypeDefRef, List<'s, Ty<'s>>>,
}

impl<'s> SolverInterner<'s> {
    pub(crate) fn metadata(self, id: DefId) -> Arc<DeclarationMetadata> {
        if let Some(data) = self.0.declarations.borrow().metadata.get(&id) {
            self.profile(|p| p.declaration_hits += 1);
            return data.clone();
        }
        let _callbacks = self.track_callbacks();
        if let Some(data) = self.0.provider.metadata(id) {
            if !self.has_unavailable() {
                self.0
                    .declarations
                    .borrow_mut()
                    .metadata
                    .insert(id, data.clone());
            }
            return data;
        }
        self.unavailable("missing declaration metadata");
        Arc::new(DeclarationMetadata {
            name: String::new(),
            generics: Vec::new(),
            parent_count: 0,
            parent: None,
            lang_item: None,
            kind: DeclarationKind::Unavailable,
        })
    }

    pub fn params(self, owner: DefId) -> &'s [GenericParamRef] {
        self.generics(owner).params
    }

    pub(crate) fn generics(self, id: DefId) -> Generics<'s> {
        if let Some(&generics) = self.0.declarations.borrow().generics.get(&id) {
            return generics;
        }
        let _callbacks = self.track_callbacks();
        let data = self.metadata(id);
        // Parameter identities are already cached by declaration. Unlike a type's arguments,
        // they rarely recur under another owner, so a second content interner adds little reuse.
        self.profile(|p| {
            p.parameter_slice_requests += u64::from(!data.generics.is_empty());
            p.parameter_slice_bytes += std::mem::size_of_val(data.generics.as_slice()) as u64;
        });
        let generics = Generics {
            params: self.0.arena.alloc_slice_copy(&data.generics),
            parent_count: data.parent_count,
        };
        if !self.has_unavailable() {
            self.0
                .declarations
                .borrow_mut()
                .generics
                .insert(id, generics);
        }
        generics
    }

    /// Build the assumptions used while checking code inside this item.
    ///
    /// Inside `fn f<T: Clone>()`, the solver can use `T: Clone` without finding a concrete impl.
    /// Requirements from an enclosing trait or impl are available in the same way.
    pub fn parameter_environment(self, id: DefId) -> ParamEnv<'s> {
        let callbacks = self.track_callbacks();
        let mut clauses = self.predicates(id).to_vec();
        // Inside `trait Render` and its associated items, `Self` implements `Render` even
        // without a written bound. Add this assumption here; including it in the trait's
        // declared requirements would make checking those requirements ask for the trait itself.
        let mut owner = Some(id);
        while let Some(id) = owner {
            if matches!(id, DefId::Trait(_)) {
                clauses.push(
                    ir::TraitRef::new_from_args(self, id, GenericArgs::identity_for_item(self, id))
                        .upcast(self),
                );
            }
            owner = self.metadata(id).parent;
        }
        // Make implied bounds available too: if `Derived: Base`, `T: Derived` supplies `T: Base`.
        let clauses = ir::elaborate::elaborate(self, clauses).collect::<Vec<_>>();
        // Later goals start their own callback scopes after these reads have happened. Keep
        // any failure with the environment so every goal using these assumptions sees it.
        ParamEnv {
            clauses: List::new(self, &clauses),
            unavailable: callbacks.failure().is_some(),
        }
    }

    /// Read a function's parameter and return types with its generic parameters still in place.
    /// For `fn id<T>(value: T) -> T`, both positions contain `T`; each call supplies its own
    /// substitution. Requirements such as `T: Clone` are read separately.
    pub fn function_signature(self, id: FunctionRef) -> Option<CallableSignature<'s>> {
        if let Some(&data) = self.0.declarations.borrow().functions.get(&id) {
            self.profile(|p| p.declaration_hits += 1);
            return Some(data);
        }
        let callbacks = self.track_callbacks();
        let Some(data) = self.0.provider.function_signature(self, id) else {
            self.unavailable("missing function signature");
            return None;
        };
        // A nested read may have failed even though lowering returned a signature. Its fallback
        // types must not become constraints on the call's arguments or return value.
        if callbacks.failure().is_some() {
            return None;
        }
        self.0.declarations.borrow_mut().functions.insert(id, data);
        Some(data)
    }

    pub(crate) fn impl_header(self, id: ImplRef) -> Option<ImplHeader<'s>> {
        if let Some(&data) = self.0.declarations.borrow().impl_headers.get(&id) {
            self.profile(|p| p.declaration_hits += 1);
            return Some(data);
        }
        let _callbacks = self.track_callbacks();
        let Some(data) = self.0.provider.impl_header(self, id) else {
            self.unavailable("missing impl header");
            return None;
        };
        if !self.has_unavailable() {
            self.0
                .declarations
                .borrow_mut()
                .impl_headers
                .insert(id, data);
        }
        Some(data)
    }

    /// Read the requirements on an item, including those inherited from its enclosing impl or
    /// trait. For `fn copy<T: Clone>(...)`, this includes `T: Clone`; a call substitutes its
    /// chosen type for `T` before asking the solver to prove the requirement.
    pub(crate) fn predicates(self, id: DefId) -> List<'s, Clause<'s>> {
        if let Some(&data) = self.0.declarations.borrow().predicates.get(&id) {
            self.profile(|p| p.declaration_hits += 1);
            return data;
        }
        let _callbacks = self.track_callbacks();
        let Some(data) = self.0.provider.predicates(self, id) else {
            self.unavailable("missing declaration predicates");
            return List::default();
        };
        if !self.has_unavailable() {
            self.0.declarations.borrow_mut().predicates.insert(id, data);
        }
        data
    }

    /// Read the guarantees declared for an associated type or `impl Trait`.
    /// For `trait Factory { type Item: Clone; }`, knowing `T: Factory` lets the solver use
    /// `<T as Factory>::Item: Clone` without knowing the concrete type of `Item`.
    pub(crate) fn bounds(self, id: DefId) -> List<'s, Clause<'s>> {
        if let Some(&data) = self.0.declarations.borrow().bounds.get(&id) {
            self.profile(|p| p.declaration_hits += 1);
            return data;
        }
        let _callbacks = self.track_callbacks();
        let Some(data) = self.0.provider.bounds(self, id) else {
            self.unavailable("missing declaration bounds");
            return List::default();
        };
        if !self.has_unavailable() {
            self.0.declarations.borrow_mut().bounds.insert(id, data);
        }
        data
    }

    pub(crate) fn alias_value(self, id: TypeAliasRef) -> Ty<'s> {
        if let Some(&data) = self.0.declarations.borrow().alias_values.get(&id) {
            self.profile(|p| p.declaration_hits += 1);
            return data;
        }
        let _callbacks = self.track_callbacks();
        let Some(data) = self.0.provider.alias_value(self, id) else {
            self.unavailable("missing alias value");
            return self.unknown();
        };
        if !self.has_unavailable() {
            self.0
                .declarations
                .borrow_mut()
                .alias_values
                .insert(id, data);
        }
        data
    }

    pub(crate) fn field_tys(self, id: DefId) -> List<'s, Ty<'s>> {
        let DefId::Adt(id) = id else {
            self.unavailable("ADT identity");
            return List::default();
        };
        if let Some(&data) = self.0.declarations.borrow().fields.get(&id) {
            self.profile(|p| p.declaration_hits += 1);
            return data;
        }
        let _callbacks = self.track_callbacks();
        let Some(data) = self.0.provider.field_tys(self, id) else {
            self.unavailable("missing ADT fields");
            return List::default();
        };
        if !self.has_unavailable() {
            self.0.declarations.borrow_mut().fields.insert(id, data);
        }
        data
    }
}
