//! Lower compiler-provided derives into ordinary impl headers.
//!
//! A derived `Clone` needs an applicable impl for method lookup; the method signature already
//! exists on the trait. We therefore retain only the header and its bounds. These declarations
//! go through the same generated-item storage and semantic lowering as declarative macro output.
//! TODO: Add method declarations if navigation to individual derived implementations is needed.

use anyhow::Context as _;
use rg_arena::Arena;
use rg_cfg_eval::CfgEvaluator;
use rg_ir_model::{CrateRef, FileId, ModuleId, ModuleRef, Path, Span};
use rg_item_tree::{
    BuiltinDeriveKind, BuiltinMacroKind, ConstExpr, DeriveAttrs, FieldItem, GenericArg,
    GenericParams, ImplItem, ItemKind, ItemNode, ItemTreeDb, ItemTreeRef, TraitBoundModifier,
    TypeBound, TypeOrConstParamData, TypePath, TypePathAnchor, TypePathSegment, TypeRef,
    VisibilityLevel, WherePredicate,
};
use rg_std::{ExpectedUnique, UniqueVec};
use rg_text::{Name, NameInterner, PackageNameInterners};

use crate::{
    CrateResolutionEnv, GeneratedItemRef, ItemSource, ItemSourceKind, LocalDefKind, LocalImplData,
    MacroDefinitionEnv, ScopeResolver,
    build::{collect::CrateState, finalize::FinalizeCrateStates},
    source::GeneratedSourceData,
};

/// One derived impl waiting to be attached to the module that owns its struct or enum.
///
/// Resolving derives borrows the collected crate states. Keep the resulting impls here until
/// collection finishes, then add them to those states with `apply`.
pub(crate) struct BuiltinDeriveExpansion {
    crate_ref: CrateRef,
    module: ModuleId,
    source: GeneratedSourceData,
}

impl BuiltinDeriveExpansion {
    /// Resolve enabled derives using the scopes left by import resolution and macro expansion.
    pub(crate) fn collect<E>(
        env: &E,
        states: &FinalizeCrateStates,
        item_tree: &ItemTreeDb,
        interners: &mut PackageNameInterners,
    ) -> anyhow::Result<Vec<Self>>
    where
        E: CrateResolutionEnv<Error = rg_package_store::PackageStoreError> + MacroDefinitionEnv,
    {
        let mut expansions = Vec::new();
        for package_states in states.iter_dirty() {
            for state in package_states {
                let cfg = state.cfg_evaluator();
                let map = state.def_map_builder.partial();
                let interner = interners
                    .package_mut(state.crate_ref.package.0)
                    .context("fetch name interner for builtin derives")?;
                for declaration in map.local_defs() {
                    if !matches!(declaration.kind, LocalDefKind::Struct | LocalDefKind::Enum) {
                        // TODO: Support union derives and declarations collected inside bodies.
                        continue;
                    }
                    let Some((item, origin_source)) =
                        Self::source_item(state, item_tree, declaration.source)
                            .context("read builtin derive input")?
                    else {
                        continue;
                    };
                    let calls = match &item.kind {
                        ItemKind::Struct(item) => &item.derives.calls,
                        ItemKind::Enum(item) => &item.derives.calls,
                        _ => continue,
                    };
                    let module = ModuleRef::krate(state.crate_ref, declaration.module);
                    for call in calls {
                        if call
                            .predicate
                            .as_ref()
                            .is_some_and(|predicate| !cfg.is_predicate_enabled(predicate))
                        {
                            continue;
                        }

                        let Some((kind, defining_crate)) =
                            Self::resolve_builtin(env, module, &call.path)
                                .context("resolve builtin derive")?
                        else {
                            continue;
                        };
                        let Some(root) = Self::trait_root(state, defining_crate) else {
                            continue;
                        };
                        // Derive paths in generated syntax still use offsets into that syntax.
                        // The item span already points to the source macro call, so use it instead.
                        let span = match declaration.source.kind {
                            ItemSourceKind::Generated(_) => item.span,
                            _ => call.span,
                        };
                        let Some(impl_item) =
                            Self::lower_impl(kind, root, item, span, cfg, interner)
                        else {
                            continue;
                        };
                        expansions.push(Self::from_impl(
                            module,
                            item.file_id,
                            origin_source,
                            span,
                            impl_item,
                        ));
                    }
                }
            }
        }
        Ok(expansions)
    }

    /// Add the impl to generated-item storage and the owning module's impl list.
    pub(crate) fn apply(self, states: &mut FinalizeCrateStates) {
        let state = states
            .crate_state_mut(self.crate_ref)
            .expect("derive belongs to a collected crate");
        let file_id = self.source.origin_file_id;
        let span = self.source.origin_span;
        let item = self.source.top_level[0];
        let source = state.def_map_builder.alloc_generated_source(self.source);
        let local_impl = state.def_map_builder.alloc_local_impl(LocalImplData {
            module: self.module,
            source: ItemSource::synthetic(file_id, GeneratedItemRef { source, item }),
            file_id,
            span,
        });
        state
            .def_map_builder
            .module_mut(self.module)
            .expect("derive owner module exists")
            .impls
            .push(local_impl);
    }

    /// A derive can come from a source file or from another macro's output. Keep the original
    /// item-tree source alongside the item so the generated impl retains its source provenance.
    fn source_item<'a>(
        state: &'a CrateState,
        item_tree: &'a ItemTreeDb,
        source: ItemSource,
    ) -> anyhow::Result<Option<(&'a ItemNode, ItemTreeRef)>> {
        let (item, origin_source) = match source.kind {
            ItemSourceKind::ItemTree(source) => (
                item_tree
                    .package(state.crate_ref.package.0)
                    .and_then(|package| package.item(source)),
                source,
            ),
            ItemSourceKind::Generated(source) => {
                let source_data = state
                    .def_map_builder
                    .partial()
                    .generated_source(source.source)
                    .context("fetch generated derive input")?;
                (source_data.item(source.item), source_data.origin_source)
            }
            ItemSourceKind::Synthetic(_) | ItemSourceKind::Body(_) => return Ok(None),
        };
        Ok(item.map(|item| (item, origin_source)))
    }

    /// Derives use the macro namespace, including imported aliases and the prelude. They do not
    /// use the textual scope of `macro_rules!` invocations.
    fn resolve_builtin<E>(
        env: &E,
        module: ModuleRef,
        path: &Path,
    ) -> anyhow::Result<Option<(BuiltinDeriveKind, CrateRef)>>
    where
        E: CrateResolutionEnv<Error = rg_package_store::PackageStoreError> + MacroDefinitionEnv,
    {
        let resolver = ScopeResolver::new(env);
        let bindings = if let Some(name) = path.relative_single_name() {
            let bindings = resolver
                .visible_unqualified_macro_bindings(module, [module], name)
                .context("resolve unqualified derive macro")?;
            if bindings.module_scope.is_empty() {
                bindings.standard_prelude
            } else {
                bindings.module_scope
            }
        } else {
            resolver
                .macro_bindings(module, path)
                .context("resolve qualified derive macro")?
        };
        let mut definitions = ExpectedUnique::new();
        for binding in bindings {
            if let Some(definition) = env
                .macro_definition_view(binding.def)
                .context("read derive macro definition")?
            {
                definitions.push(definition);
            }
        }
        let ExpectedUnique::One(definition) = definitions else {
            return Ok(None);
        };
        let Some(BuiltinMacroKind::Derive(kind)) = definition.data.builtin else {
            return Ok(None);
        };
        Ok(Some((kind, definition.def_ref.origin.origin_crate())))
    }

    /// Core also derives traits on its own types. Use `crate::` there and the absolute core root
    /// elsewhere, so a caller's module named `core` cannot capture the trait path. Missing roots
    /// must not become guessed impls.
    // TODO: Remove this core-root requirement when generated trait references
    // carry compiler identities instead of paths.
    fn trait_root(state: &CrateState, defining_crate: CrateRef) -> Option<&'static str> {
        if defining_crate == state.crate_ref {
            Some("crate")
        } else if state
            .extern_prelude
            .resolve("core")
            .is_some_and(|root| root.origin.origin_crate() == defining_crate)
        {
            Some("core")
        } else {
            None
        }
    }

    // Store each impl as an ordinary generated item, retaining the location of its derive.
    fn from_impl(
        module: ModuleRef,
        file_id: FileId,
        origin_source: ItemTreeRef,
        span: Span,
        impl_item: ImplItem,
    ) -> Self {
        let mut items = Arena::new();
        let impl_id = items.alloc(ItemNode::source(
            ItemKind::Impl(impl_item),
            None,
            None,
            VisibilityLevel::Private,
            None,
            span,
            file_id,
        ));
        Self {
            crate_ref: module.origin.origin_crate(),
            module: module.module,
            source: GeneratedSourceData {
                origin_file_id: file_id,
                origin_span: span,
                origin_source,
                top_level: vec![impl_id],
                items,
            },
        }
    }

    /// Build the receiver, trait, and bounds for one derived impl.
    ///
    /// For `#[derive(Clone)] struct Wrap<T>(T);`, this produces the header
    /// `impl<T: Clone> Clone for Wrap<T>`. The item's existing bounds are kept too.
    fn lower_impl(
        kind: BuiltinDeriveKind,
        root: &str,
        item: &ItemNode,
        span: Span,
        cfg: CfgEvaluator<'_>,
        interner: &mut NameInterner,
    ) -> Option<ImplItem> {
        let name = item.name.as_ref()?;
        let (generics, attrs, fields, add_bounds) = match &item.kind {
            ItemKind::Struct(item) => (
                &item.generics,
                &item.derives,
                item.fields.fields().iter().collect::<Vec<_>>(),
                true,
            ),
            ItemKind::Enum(item) => {
                // Default constructs a single unit variant. For `enum Choice<T> { Empty, Some(T) }`,
                // choosing `Empty` needs no `T: Default` bound, even though another variant uses T.
                if kind == BuiltinDeriveKind::Default {
                    let defaults = item
                        .derives
                        .default_variants
                        .iter()
                        .filter(|default| cfg.is_enabled(default))
                        .count();
                    if defaults != 1 {
                        return None;
                    }
                }
                let fields = item
                    .variants
                    .iter()
                    .flat_map(|variant| variant.fields.fields())
                    .collect();
                (
                    &item.generics,
                    &item.derives,
                    fields,
                    kind != BuiltinDeriveKind::Default,
                )
            }
            _ => return None,
        };
        let trait_ty = Self::trait_type(kind, root, span, interner);
        let bound = TypeBound::Trait {
            ty: trait_ty.clone(),
            modifier: TraitBoundModifier::None,
        };
        let (mut impl_generics, args) =
            Self::lower_generics(generics, span, add_bounds.then_some(&bound));
        if add_bounds {
            Self::add_projection_bounds(&mut impl_generics, attrs, &fields, cfg, &bound);
        }
        let mut self_path = Self::type_path([name.clone()], span);
        self_path.segments[0].args = args;
        Some(ImplItem {
            generics: impl_generics,
            trait_ref: Some(trait_ty),
            self_ty: TypeRef::Path(self_path),
            // These are analysis headers. Required and provided signatures come from the trait;
            // no method bodies or extra associated declarations need to remain resident.
            items: Vec::new(),
            is_unsafe: false,
        })
    }

    fn trait_type(
        kind: BuiltinDeriveKind,
        root: &str,
        span: Span,
        interner: &mut NameInterner,
    ) -> TypeRef {
        // TODO: Use lang/diagnostic-item identities for this trait and its generated bounds.
        // Generated TypeRefs need to carry those identities into semantic/type lowering so the
        // impl does not depend on core's module layout or the caller's extern-prelude spelling.
        let (module, trait_name) = match kind {
            BuiltinDeriveKind::Clone => ("clone", "Clone"),
            BuiltinDeriveKind::Copy => ("marker", "Copy"),
            BuiltinDeriveKind::Debug => ("fmt", "Debug"),
            BuiltinDeriveKind::Default => ("default", "Default"),
            BuiltinDeriveKind::Hash => ("hash", "Hash"),
            BuiltinDeriveKind::PartialEq => ("cmp", "PartialEq"),
            BuiltinDeriveKind::Eq => ("cmp", "Eq"),
            BuiltinDeriveKind::PartialOrd => ("cmp", "PartialOrd"),
            BuiltinDeriveKind::Ord => ("cmp", "Ord"),
        };
        let mut trait_path = Self::type_path(
            [root, module, trait_name].map(|name| interner.intern(name)),
            span,
        );
        trait_path.absolute = root != "crate";
        TypeRef::Path(trait_path)
    }

    /// Prepare impl parameters and the matching receiver arguments, such as `<'a, T, N>`.
    ///
    /// When a derive needs bounds, rustc places them on every type parameter, even in
    /// `PhantomData<T>` or `fn() -> T`. Keep that behavior, the user's existing bounds, and the
    /// original order of type/const arguments. Defaults belong on the declaration, not on its impl.
    fn lower_generics(
        generics: &GenericParams,
        span: Span,
        bound: Option<&TypeBound>,
    ) -> (GenericParams, Vec<GenericArg>) {
        let mut impl_generics = generics.clone();
        let mut args = generics
            .lifetimes
            .iter()
            .map(|param| GenericArg::Lifetime(param.name.clone()))
            .collect::<Vec<_>>();

        for param in &mut impl_generics.type_or_consts {
            match param {
                TypeOrConstParamData::Type(param) => {
                    args.push(GenericArg::Type(TypeRef::Path(Self::type_path(
                        [param.name.clone()],
                        span,
                    ))));
                    param.default = None;
                    if let Some(bound) = bound {
                        param.bounds.push(bound.clone());
                    }
                }
                TypeOrConstParamData::Const(param) => {
                    args.push(GenericArg::Const(ConstExpr::new(
                        param.name.to_string(),
                        span,
                    )));
                    param.default = None;
                }
            }
        }
        (impl_generics, args)
    }

    /// A `T: Clone` bound says nothing about `T::Item`. Rustc adds a bound for each shorthand
    /// projection found in fields, including one nested inside another field type.
    fn add_projection_bounds(
        generics: &mut GenericParams,
        attrs: &DeriveAttrs,
        fields: &[&FieldItem],
        cfg: CfgEvaluator<'_>,
        bound: &TypeBound,
    ) {
        let type_params = generics
            .type_param_names()
            .map(Name::as_str)
            .collect::<Vec<_>>();
        let mut projections = UniqueVec::new();
        for (index, field) in fields.iter().enumerate() {
            if attrs
                .field_cfg
                .iter()
                .any(|(field, gates)| *field == index && !cfg.is_enabled(gates))
            {
                continue;
            }
            Self::collect_projections(&field.ty, &type_params, &mut projections);
        }
        for ty in projections.into_vec() {
            generics.where_predicates.push(WherePredicate::Type {
                ty,
                bounds: vec![bound.clone()],
            });
        }
    }

    fn type_path(names: impl IntoIterator<Item = Name>, span: Span) -> TypePath {
        TypePath {
            source_span: span,
            absolute: false,
            anchor: None,
            segments: names
                .into_iter()
                .map(|name| TypePathSegment {
                    name,
                    args: Vec::new(),
                    span,
                })
                .collect(),
        }
    }

    /// Find shorthand projections starting with one of this item's type parameters.
    ///
    /// `Option<T::Item>` contributes `T::Item`. A qualified `<T as Family>::Item` does not
    /// contribute an extra bound: rustc selects written paths starting with `T`, rather than
    /// checking whether a projection's base mentions `T`.
    fn collect_projections(
        ty: &TypeRef,
        type_params: &[&str],
        projections: &mut UniqueVec<TypeRef>,
    ) {
        match ty {
            TypeRef::Path(path) => {
                let is_projection = path.anchor.is_none()
                    && !path.absolute
                    && path.segments.len() > 1
                    && type_params.contains(&path.segments[0].name.as_str());
                // A projection's base or arguments can contain more projections, so keep walking
                // after recording it as a bound's target.
                if is_projection {
                    projections.push(ty.clone());
                }
                if let Some(anchor) = &path.anchor {
                    match anchor {
                        TypePathAnchor::Type(ty) => {
                            Self::collect_projections(ty, type_params, projections)
                        }
                        TypePathAnchor::QualifiedTrait { self_ty, trait_ty } => {
                            Self::collect_projections(self_ty, type_params, projections);
                            Self::collect_projections(trait_ty, type_params, projections);
                        }
                    }
                }
                for arg in path.segments.iter().flat_map(|segment| &segment.args) {
                    match arg {
                        GenericArg::Type(ty) | GenericArg::AssocType { ty: Some(ty), .. } => {
                            Self::collect_projections(ty, type_params, projections)
                        }
                        GenericArg::FnTraitArgs { params, ret } => {
                            for ty in params {
                                Self::collect_projections(ty, type_params, projections);
                            }
                            Self::collect_projections(ret, type_params, projections);
                        }
                        _ => {}
                    }
                }
            }
            TypeRef::Tuple(types) => {
                for ty in types {
                    Self::collect_projections(ty, type_params, projections);
                }
            }
            TypeRef::Reference { inner, .. }
            | TypeRef::RawPointer { inner, .. }
            | TypeRef::Slice(inner)
            | TypeRef::Array { inner, .. } => {
                Self::collect_projections(inner, type_params, projections)
            }
            TypeRef::FnPointer { params, ret } => {
                for ty in params {
                    Self::collect_projections(ty, type_params, projections);
                }
                Self::collect_projections(ret, type_params, projections);
            }
            TypeRef::ImplTrait(bounds) | TypeRef::DynTrait(bounds) => {
                for ty in bounds.iter().filter_map(TypeBound::trait_ty) {
                    Self::collect_projections(ty, type_params, projections);
                }
            }
            _ => {}
        }
    }
}
