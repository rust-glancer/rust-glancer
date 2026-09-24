//! Value-path lookup from body scopes to declarations and types.
//!
//! DefMap owns namespace selection and shadowing. This layer gives ordinary local bindings lexical
//! priority, then projects selected value definitions into `BodyResolution` and `Ty`. Unit and
//! tuple struct constructors arrive through the value namespace like functions and constants;
//! ordinary value lookup does not recover them by falling back to a type-name match. Record
//! expressions have a separate entry point because their constructor paths use the type namespace.

use rg_def_map::{DefMapSource, NamespaceSet, ResolvePathResult};
use rg_ir_model::{
    BindingId, ConstRef, DefId, DefMapRef, EnumVariantRef, ExprId, FunctionRef, GenericDefRef,
    LocalEnumVariantRef, ModuleId, ModuleRef, Path, ScopeId, SemanticItemRef, StaticRef,
    TypeDefRef, identity::DeclarationRef,
};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::{ItemStoreSource, TypePathResolution};
use rg_std::{ExpectedUnique, UniqueVec};
use rg_ty::{
    lowering::TypeLoweringQuery,
    solver::{self, AdtTy, InferenceTable, List, SolverInterner, Ty},
};

use crate::{BodyPath, body::facts::BodyResolution, resolution::BodyResolutionContext};

/// Resolves paths used by expressions without mutating the body.
pub struct BodyValuePathQuery<'query, D, I> {
    context: BodyResolutionContext<'query, D, I>,
}

impl<'query, D, I> BodyValuePathQuery<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    pub(crate) fn new(context: BodyResolutionContext<'query, D, I>) -> Self {
        Self { context }
    }

    /// Find declarations for a path without considering ordinary local bindings.
    pub fn resolve_nonlocal_path_declarations(
        &self,
        scope: ScopeId,
        path: &Path,
    ) -> Result<Vec<DeclarationRef>, PackageStoreError> {
        let (resolution, _) = self.resolve_nonlocal_path_expr(scope, path)?;
        Ok(resolution.declarations(self.context.body_ref()))
    }

    /// Find the type of a path without considering ordinary local bindings.
    pub fn resolve_nonlocal_path_ty(
        &self,
        scope: ScopeId,
        path: &Path,
    ) -> Result<rg_ty::Ty, PackageStoreError> {
        let (_, ty) = self.resolve_nonlocal_path_expr(scope, path)?;
        Ok(ty)
    }

    /// Resolve a path without considering ordinary local bindings.
    pub(crate) fn resolve_nonlocal_path_expr(
        &self,
        scope: ScopeId,
        path: &Path,
    ) -> Result<(BodyResolution, rg_ty::Ty), PackageStoreError> {
        let paths = self.context.item_paths();
        TypeLoweringQuery::new(&paths, &self.context).with_storage(|cx| {
            let (resolution, ty) = self.resolve_path_expr(scope, path, None, cx)?;
            Ok((resolution, cx.raise_ty(ty).unwrap_or(rg_ty::Ty::Unknown)))
        })
    }

    /// Resolve a body expression's path, including qualified associated items and local bindings.
    /// A local binding returns its identity with an unknown type; the caller connects that identity
    /// to its own type state. Item types come from declarations and can be resolved here directly.
    pub(crate) fn resolve_body_path_expr<'s>(
        &self,
        expr: ExprId,
        path: &BodyPath,
        cx: SolverInterner<'s>,
    ) -> Result<(BodyResolution, Ty<'s>), PackageStoreError> {
        let expr_data = self.context.body().expr_unchecked(expr);
        // Preserve associated-item syntax such as `<T as Trait>::VALUE` before trying ordinary
        // lexical lookup, which only understands a DefMap path and its visible local bindings.
        // This is declaration discovery: its independent result has no body inference variables.
        // Calls subsequently instantiate the selected declaration in the caller's live table.
        if let Some(result) = self
            .context
            .associated_items()
            .resolve_body_path(expr_data.scope, path)?
        {
            let (resolution, ty) = result;
            return Ok((
                resolution,
                cx.lower_ty(
                    &ty,
                    cx.params(self.context.body().owner().generic_def().into()),
                ),
            ));
        }

        match path.as_def_map_path() {
            Some(path) => {
                self.resolve_path_expr(expr_data.scope, &path, Some(expr_data.visible_bindings), cx)
            }
            None => Ok((BodyResolution::Unknown, cx.unknown())),
        }
    }

    /// Record constructors use type names, independently of ordinary local value bindings.
    /// For `struct User { name: Name }`, a local `let User = ...` therefore does not hide the
    /// constructor in `User { name }`.
    pub(crate) fn resolve_record_expr_path<'s>(
        &self,
        scope: ScopeId,
        path: &BodyPath,
        table: &InferenceTable<'s>,
    ) -> Result<(BodyResolution, Ty<'s>), PackageStoreError> {
        let cx = table.interner();
        let Some(def_map_path) = path.as_def_map_path() else {
            return Ok((BodyResolution::Unknown, cx.unknown()));
        };

        match self
            .context
            .type_path_query()
            .resolve_in_scope(scope, &def_map_path)?
        {
            TypePathResolution::SelfType(type_def) => {
                return Ok((
                    BodyResolution::Unknown,
                    cx.adt(self.record_nominal_ty(scope, path, type_def, table)?),
                ));
            }
            TypePathResolution::TypeDef(type_def) => {
                // Prefer the source local def so navigation stays source-shaped. Body-local
                // types already have the right identity and need no item-store lookup.
                let declaration = if type_def.origin == DefMapRef::Body(self.context.body_ref()) {
                    DeclarationRef::from(type_def)
                } else {
                    self.context
                        .item_query()
                        .local_def_for_type_def(type_def)?
                        .map(DeclarationRef::from)
                        .unwrap_or_else(|| DeclarationRef::from(type_def))
                };
                return Ok((
                    BodyResolution::Declarations([declaration].into_iter().collect()),
                    cx.adt(self.record_nominal_ty(scope, path, type_def, table)?),
                ));
            }
            TypePathResolution::TypeAlias(_)
            | TypePathResolution::Trait(_)
            | TypePathResolution::Unknown => {}
        }

        // Record enum variants live in the type namespace even though they are not themselves
        // types. Resolve that identity separately so `Choice::Record { value: 1 }` does not depend
        // on the bare-value constructor path used by tuple and unit variants.
        if let Some(variant_ref) = self
            .context
            .type_path_query()
            .resolve_enum_variant_in_scope(scope, &def_map_path)?
            && let Some(variant) = self.context.item_query().enum_variant_data(variant_ref)?
        {
            return Ok((
                BodyResolution::Declarations(
                    [DeclarationRef::EnumVariant(variant_ref)]
                        .into_iter()
                        .collect(),
                ),
                cx.adt(self.record_nominal_ty(scope, path, variant.owner, table)?),
            ));
        }

        self.resolve_path_expr(scope, &def_map_path, None, cx)
    }

    /// Preserve written record arguments, leaving omitted type arguments unknown for inference.
    fn record_nominal_ty<'s>(
        &self,
        scope: ScopeId,
        path: &BodyPath,
        type_def: TypeDefRef,
        table: &InferenceTable<'s>,
    ) -> Result<AdtTy<'s>, PackageStoreError> {
        let generics = self
            .context
            .item_paths()
            .generics()
            .generics(GenericDefRef::TypeDef(type_def))?;
        let args = if let Some(args) = path.last_segment_angle_args() {
            self.context
                .live()
                .generic_args(scope, &generics, args, table)?
        } else {
            table.interner().unknown_args(solver::DefId::Adt(type_def))
        };
        Ok(AdtTy {
            def: type_def,
            args,
        })
    }

    /// Resolve a value path from a body scope.
    ///
    /// `visible_bindings` caps which local bindings are visible for local queries.
    fn resolve_path_expr<'s>(
        &self,
        scope: ScopeId,
        path: &Path,
        visible_bindings: Option<usize>,
        cx: SolverInterner<'s>,
    ) -> Result<(BodyResolution, Ty<'s>), PackageStoreError> {
        // Single-segment paths are the only ones that can resolve to local bindings. They also
        // need lexical item lookup, so handle them before type-shaped paths.
        if let Some(name) = path.single_name()
            && let Some((resolution, ty)) =
                self.resolve_single_segment_value_name(scope, name, visible_bindings, cx)?
        {
            return Ok((resolution, ty));
        }

        // `Self` and associated-value prefixes still need type resolution. Ordinary tuple/unit
        // constructors have already taken the value-namespace path above.
        match self
            .context
            .type_path_query()
            .resolve_in_scope(scope, path)?
        {
            TypePathResolution::SelfType(type_def) => {
                return Ok((
                    BodyResolution::Unknown,
                    cx.adt(AdtTy {
                        def: type_def,
                        args: List::default(),
                    }),
                ));
            }
            TypePathResolution::TypeDef(_)
            | TypePathResolution::TypeAlias(_)
            | TypePathResolution::Trait(_)
            | TypePathResolution::Unknown => {}
        }

        // Associated value paths are split at the last segment: `Type::VALUE` resolves the
        // `Type` prefix first, then asks associated-item lookup for `VALUE`.
        if let Some((prefix, last_segment)) = path.split_prefix_name()
            && let Some((resolution, ty)) =
                self.context
                    .associated_items()
                    .resolve_path(scope, &prefix, last_segment)?
        {
            return Ok((
                resolution,
                cx.lower_ty(
                    &ty,
                    cx.params(self.context.body().owner().generic_def().into()),
                ),
            ));
        }

        // Multi-segment body paths can name body-local values nested in local modules. Single
        // names already took the lexical route above.
        if path.single_name().is_none()
            && let Some((resolution, ty)) =
                self.resolve_body_value_path_from_def_map(scope, path, cx)?
        {
            return Ok((resolution, ty));
        }

        // Finally, look from the semantic owner module. This covers ordinary module items and
        // initializer bodies whose owner/fallback modules are outside the body def map.
        let result = self.resolve_path_from_owner_modules(path)?;
        if result.resolved.is_empty() {
            return Ok((BodyResolution::Unknown, cx.unknown()));
        }

        Ok(self
            .value_name_resolution(
                BodyValueName::Candidates(self.value_candidates_for_defs(result.resolved)?),
                cx,
            )?
            .unwrap_or((BodyResolution::Unknown, cx.unknown())))
    }

    /// Search one value name through parent scopes, with an optional local binding cutoff.
    fn resolve_single_segment_value_name<'s>(
        &self,
        start_scope: ScopeId,
        name: &str,
        visible_bindings: Option<usize>,
        cx: SolverInterner<'s>,
    ) -> Result<Option<(BodyResolution, Ty<'s>)>, PackageStoreError> {
        // Value lookup is scope-ordered: an inner const/function shadows an outer binding just as
        // surely as an inner binding shadows an outer item.
        let from = ModuleRef {
            origin: DefMapRef::Body(self.context.body_ref()),
            module: ModuleId(start_scope.0),
        };
        let mut scope = Some(start_scope);
        while let Some(scope_id) = scope {
            let Some(scope_data) = self.context.body().scope(scope_id) else {
                return Ok(None);
            };

            if let Some(visible_bindings) = visible_bindings {
                for binding in scope_data.bindings.iter().rev() {
                    if binding.0 >= visible_bindings {
                        continue;
                    }

                    let Some(binding_data) = self.context.body().binding(*binding) else {
                        continue;
                    };
                    if binding_data.name.as_deref() == Some(name) {
                        return self.value_name_resolution(BodyValueName::Binding(*binding), cx);
                    }
                }
            }

            let module = ModuleRef {
                origin: DefMapRef::Body(self.context.body_ref()),
                module: ModuleId(scope_id.0),
            };
            let defs = self
                .context
                .def_map_query()
                .scope_resolver()
                .resolve_lexical_name_in_module(from, module, name, NamespaceSet::VALUES)?;
            let value_name = BodyValueName::Candidates(self.value_candidates_for_defs(defs)?);
            if let Some(resolution) = self.value_name_resolution(value_name, cx)? {
                return Ok(Some(resolution));
            }

            scope = scope_data.parent;
        }

        Ok(None)
    }

    /// Look up a path from the body owner module, then the fallback module.
    fn resolve_path_from_owner_modules(
        &self,
        path: &Path,
    ) -> Result<ResolvePathResult, PackageStoreError> {
        let owner_module = self.context.body().owner_module();
        let def_maps = self.context.def_map_query();
        let result =
            def_maps
                .scope_resolver()
                .resolve_path(owner_module, path, NamespaceSet::VALUES)?;
        if !result.resolved.is_empty() {
            return Ok(result);
        }

        let fallback_module = self.context.body().fallback_module();
        if fallback_module == owner_module {
            return Ok(result);
        }

        def_maps
            .scope_resolver()
            .resolve_path(fallback_module, path, NamespaceSet::VALUES)
    }

    /// Resolve a multi-segment value path through the body def map.
    fn resolve_body_value_path_from_def_map<'s>(
        &self,
        scope: ScopeId,
        path: &Path,
        cx: SolverInterner<'s>,
    ) -> Result<Option<(BodyResolution, Ty<'s>)>, PackageStoreError> {
        let from = ModuleRef {
            origin: DefMapRef::Body(self.context.body_ref()),
            module: ModuleId(scope.0),
        };
        let defs = self
            .context
            .def_map_query()
            .scope_resolver()
            .resolve_lexical_path(from, path, NamespaceSet::VALUES)?
            .resolved;
        self.value_name_resolution(
            BodyValueName::Candidates(self.value_candidates_for_defs(defs)?),
            cx,
        )
    }

    /// Project selected value-namespace definitions into Body IR candidates.
    ///
    /// DefMap has already decided which namespace and route won. This step only attaches semantic
    /// declaration identities and types; it never searches the type namespace for a constructor.
    fn value_candidates_for_defs(
        &self,
        defs: impl IntoIterator<Item = DefId>,
    ) -> Result<UniqueVec<BodyValueCandidate>, PackageStoreError> {
        let mut candidates = UniqueVec::new();
        for def in defs {
            match def {
                DefId::Local(local_def) => {
                    let Some(item) = self
                        .context
                        .item_query()
                        .semantic_item_for_local_def(local_def)?
                    else {
                        continue;
                    };
                    match item {
                        SemanticItemRef::Function(function) => {
                            candidates.push(BodyValueCandidate::Function(function));
                        }
                        SemanticItemRef::Const(const_ref) => {
                            candidates.push(BodyValueCandidate::Const(const_ref));
                        }
                        SemanticItemRef::Static(static_ref) => {
                            candidates.push(BodyValueCandidate::Static(static_ref));
                        }
                        SemanticItemRef::TypeDef(type_def) => {
                            if self
                                .context
                                .item_query()
                                .type_def_has_value_constructor(type_def)?
                            {
                                candidates.push(BodyValueCandidate::TypeConstructor(type_def));
                            }
                        }
                        SemanticItemRef::Trait(_)
                        | SemanticItemRef::Impl(_)
                        | SemanticItemRef::TypeAlias(_) => {}
                    }
                }
                DefId::EnumVariant(variant_def) => {
                    if let Some(candidate) = self.enum_variant_candidate(variant_def)? {
                        candidates.push(candidate);
                    }
                }
                DefId::Module(_) => {}
            }
        }

        Ok(candidates)
    }

    /// Convert one value-namespace match into body resolution and type.
    fn value_name_resolution<'s>(
        &self,
        value_name: BodyValueName,
        cx: SolverInterner<'s>,
    ) -> Result<Option<(BodyResolution, Ty<'s>)>, PackageStoreError> {
        match value_name {
            BodyValueName::Binding(binding) => {
                // A local path reports identity only. Its consumer links the binding's live
                // inference slot; semantic item lookup does not read body-local types.
                Ok(Some((BodyResolution::Binding(binding), cx.unknown())))
            }
            BodyValueName::Candidates(candidates) => {
                let mut declarations = UniqueVec::new();
                let mut tys = ExpectedUnique::new();

                for candidate in candidates {
                    match candidate {
                        BodyValueCandidate::Function(function) => {
                            declarations.push(DeclarationRef::from(function));
                            tys.push(cx.fn_def(
                                function,
                                cx.unknown_args(solver::DefId::Function(function)),
                            ));
                        }
                        BodyValueCandidate::Const(const_ref) => {
                            declarations.push(DeclarationRef::from(const_ref));
                            let paths = self.context.item_paths();
                            tys.push(
                                TypeLoweringQuery::new(&paths, &self.context)
                                    .const_ty(cx, const_ref)?
                                    .unwrap_or_else(|| cx.unknown()),
                            );
                        }
                        BodyValueCandidate::Static(static_ref) => {
                            declarations.push(DeclarationRef::from(static_ref));
                            let paths = self.context.item_paths();
                            tys.push(
                                TypeLoweringQuery::new(&paths, &self.context)
                                    .static_ty(cx, static_ref)?
                                    .unwrap_or_else(|| cx.unknown()),
                            );
                        }
                        BodyValueCandidate::TypeConstructor(type_def) => {
                            declarations.push(DeclarationRef::from(type_def));
                            tys.push(cx.adt(AdtTy {
                                def: type_def,
                                args: List::default(),
                            }));
                        }
                        BodyValueCandidate::EnumVariant(variant_ref, owner) => {
                            declarations.push(DeclarationRef::EnumVariant(variant_ref));
                            let args = self
                                .context
                                .item_query()
                                .generic_params_for_type_def(owner)?
                                .map(|params| {
                                    params
                                        .types()
                                        .map(|_| cx.unknown().into())
                                        .collect::<Vec<_>>()
                                })
                                .unwrap_or_default();
                            tys.push(cx.adt(AdtTy {
                                def: owner,
                                args: List::new(cx, &args),
                            }));
                        }
                    }
                }

                if !declarations.is_empty() {
                    return Ok(Some((
                        BodyResolution::Declarations(declarations),
                        tys.into_option().unwrap_or_else(|| cx.unknown()),
                    )));
                }

                Ok(None)
            }
        }
    }

    /// Build the constructor-like value type for an imported enum variant.
    fn enum_variant_candidate(
        &self,
        variant_def: LocalEnumVariantRef,
    ) -> Result<Option<BodyValueCandidate>, PackageStoreError> {
        let def_maps = self.context.def_map_source();
        let item_query = self.context.item_query();
        if let Some(variant_def_data) = def_maps.local_enum_variant_data(variant_def)?
            && let Some(variant_ref) =
                item_query.enum_variant_ref_for_local_enum_variant(variant_def, variant_def_data)?
            && let Some(variant_data) = item_query.enum_variant_data(variant_ref)?
        {
            Ok(Some(BodyValueCandidate::EnumVariant(
                variant_ref,
                variant_data.owner,
            )))
        } else {
            Ok(None)
        }
    }
}

/// One declaration that can satisfy a value name inside a body scope.
#[derive(Debug, Clone, PartialEq, Eq)]
enum BodyValueName {
    Binding(BindingId),
    Candidates(UniqueVec<BodyValueCandidate>),
}

/// Resolved value candidate after DefMap names have been projected through semantic item data.
#[derive(Debug, Clone, PartialEq, Eq)]
enum BodyValueCandidate {
    Function(FunctionRef),
    Const(ConstRef),
    Static(StaticRef),
    /// Unit or tuple struct selected through the value namespace.
    TypeConstructor(TypeDefRef),
    EnumVariant(EnumVariantRef, TypeDefRef),
}
