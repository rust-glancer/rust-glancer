//! Composite projection from declarations and paths into indexed types.
//!
//! Type inference owns a much larger vocabulary than editor features need. `IndexedType` keeps
//! that compiler representation behind the view boundary while still supporting the concrete
//! operations used by hover, navigation, completion, and inlay hints.

pub mod locals;

use anyhow::Context as _;
use rg_ir_model::{
    BodyRef, EnumVariantRef, FieldRef, ModuleRef, Path, PrimitiveTy, ScopeId, SemanticItemRef,
    TypeDefRef, identity::DeclarationRef, identity::ExprRef,
};
use rg_semantic_ir::{ItemStoreQuery, TypePathContext, TypePathResolution};
use rg_std::ExpectedUnique;
use rg_ty::{
    AdtTy, AliasTy, GenericArg, ItemPathQuery, ReferencePeelingCandidates, SemanticSignatureQuery,
    Ty, TypeLoweringAnchor, TypeLoweringEnv, TypeLoweringQuery, TypePathResolver as _,
};

use crate::{
    IndexedViewDb,
    body::BodyResolutionView,
    source::{IndexedSignatureTypeScope, IndexedTypePath, IndexedTypePathScope},
    ty::locals::BodyView,
};

/// An inferred type carried across the compiler-to-editor boundary.
///
/// The wrapped type is intentionally opaque. Editor features should ask a view to render it,
/// inspect the small set of stable properties exposed here, or feed it back into another view
/// query. This avoids coupling analysis workflows to solver-only variants and substitutions.
#[derive(Clone, PartialEq, Eq)]
pub struct IndexedType(Ty);

impl std::fmt::Debug for IndexedType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("IndexedType(..)")
    }
}

impl IndexedType {
    pub(crate) fn new(ty: Ty) -> Self {
        Self(ty)
    }

    pub(crate) fn raw(&self) -> &Ty {
        &self.0
    }

    /// Return the primitive kind when this is exactly a primitive type.
    pub fn primitive(&self) -> Option<PrimitiveTy> {
        match self.raw() {
            Ty::Primitive(primitive) => Some(*primitive),
            _ => None,
        }
    }

    /// Whether displaying this type after a method-chain segment would only add noise.
    pub fn is_unit_or_never(&self) -> bool {
        matches!(self.raw(), Ty::Unit | Ty::Never)
    }

    /// Iterate nominal definitions represented by this type after peeling references.
    pub fn nominal_type_defs(&self) -> impl Iterator<Item = TypeDefRef> + '_ {
        ReferencePeelingCandidates::new(self.raw())
            .filter_map(|candidate| candidate.ty().as_adts().first().map(|ty| ty.def))
    }

    /// Return the nominal definition only when reference peeling identifies exactly one.
    ///
    /// `Widget`, `&Widget`, and `&&Widget` all identify `Widget`. A type with no nominal candidate
    /// or with more than one candidate stays non-unique so callers cannot silently choose one.
    pub fn unique_nominal_type_def(&self) -> ExpectedUnique<TypeDefRef> {
        let mut result = ExpectedUnique::new();
        for type_def in self.nominal_type_defs() {
            result.push(type_def);
        }
        result
    }

    /// Return each nominal type contained in this type, including generic arguments.
    ///
    /// For `Wrapper<User>`, this returns both `Wrapper` and `User`. References and other
    /// structural wrappers do not hide the types inside them.
    pub fn contained_nominal_type_defs(&self) -> Vec<TypeDefRef> {
        let mut type_defs = Vec::new();
        Self::collect_nominal_type_defs(self.raw(), &mut type_defs);
        type_defs
    }

    fn collect_nominal_type_defs(ty: &Ty, type_defs: &mut Vec<TypeDefRef>) {
        match ty {
            Ty::Adt(adt) => {
                if !type_defs.contains(&adt.def) {
                    type_defs.push(adt.def);
                }
                Self::collect_nominal_type_args(&adt.args, type_defs);
            }
            Ty::Tuple(fields) => {
                for field in fields {
                    Self::collect_nominal_type_defs(field, type_defs);
                }
            }
            Ty::Array { inner, .. }
            | Ty::Slice(inner)
            | Ty::Reference { inner, .. }
            | Ty::RawPointer { inner, .. } => Self::collect_nominal_type_defs(inner, type_defs),
            Ty::FnPointer { params, ret } => {
                for param in params {
                    Self::collect_nominal_type_defs(param, type_defs);
                }
                Self::collect_nominal_type_defs(ret, type_defs);
            }
            Ty::Alias(AliasTy::Projection(alias)) => {
                Self::collect_nominal_type_args(&alias.args, type_defs)
            }
            Ty::Alias(AliasTy::Opaque(alias)) => {
                Self::collect_nominal_type_args(&alias.args, type_defs)
            }
            Ty::Closure(closure) => {
                for param in &closure.params {
                    Self::collect_nominal_type_defs(param, type_defs);
                }
                Self::collect_nominal_type_defs(&closure.ret, type_defs);
            }
            Ty::FnDef(function) => Self::collect_nominal_type_args(&function.args, type_defs),
            Ty::Unit
            | Ty::Never
            | Ty::Primitive(_)
            | Ty::Param(_)
            | Ty::Unknown
            | Ty::InferVar { .. } => {}
        }
    }

    fn collect_nominal_type_args(args: &[GenericArg], type_defs: &mut Vec<TypeDefRef>) {
        for arg in args {
            if let GenericArg::Type(ty) = arg {
                Self::collect_nominal_type_defs(ty, type_defs);
            }
        }
    }
}

/// Projects indexed declarations and body facts into opaque editor-facing types.
pub struct TyView<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> TyView<'a, 'db> {
    pub fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self { db }
    }

    /// Return the resolved body type for an expression.
    pub fn ty_for_expr(&self, expr: ExprRef) -> anyhow::Result<Option<IndexedType>> {
        self.body_view().expr_ty(expr.body_ir(), expr.expr_id())
    }

    /// Project a declaration into its type when that question is meaningful.
    pub fn ty_for_declaration(
        &self,
        declaration: DeclarationRef,
    ) -> anyhow::Result<Option<IndexedType>> {
        let ty = match declaration {
            DeclarationRef::Module(_) => Ok(None),
            DeclarationRef::LocalDef(local_def) => {
                let Some(SemanticItemRef::TypeDef(ty)) = ItemStoreQuery::new(self.db)
                    .semantic_item_for_local_def(local_def)
                    .context("look up local definition item")?
                else {
                    return Ok(None);
                };
                Ok(Some(Ty::adt(AdtTy::bare(ty))))
            }
            DeclarationRef::Item(SemanticItemRef::TypeDef(ty)) => {
                Ok(Some(Ty::adt(AdtTy::bare(ty))))
            }
            DeclarationRef::Item(
                SemanticItemRef::Trait(_)
                | SemanticItemRef::Impl(_)
                | SemanticItemRef::Function(_)
                | SemanticItemRef::TypeAlias(_)
                | SemanticItemRef::Const(_)
                | SemanticItemRef::Static(_),
            ) => Ok(None),
            DeclarationRef::Field(field) => self.ty_for_field(field),
            DeclarationRef::EnumVariant(variant) => self.ty_for_enum_variant(variant),
            DeclarationRef::BodyBinding(binding) => {
                return self.body_view().binding_ty(binding);
            }
        }
        .context("project declaration type")?;
        Ok(ty.map(IndexedType::new))
    }

    /// Resolve a signature type path into an indexed type.
    pub fn ty_for_type_path(
        &self,
        context: TypePathContext,
        path: &Path,
    ) -> anyhow::Result<IndexedType> {
        let resolution = self
            .db
            .resolve_type_path(TypeLoweringAnchor::Context(context), path)
            .context("resolve signature type path")?;
        if matches!(resolution, TypePathResolution::Unknown)
            && let Some(primitive) = path.single_name().and_then(PrimitiveTy::from_name)
        {
            return Ok(IndexedType::new(Ty::Primitive(primitive)));
        }

        Ok(IndexedType::new(
            self.type_path_resolution_to_ty(resolution)?,
        ))
    }

    /// Resolve a type path from a module scope with no declaration-owned generics.
    pub fn ty_for_module_type_path(
        &self,
        module: ModuleRef,
        path: &Path,
    ) -> anyhow::Result<IndexedType> {
        self.ty_for_type_path(TypePathContext::module(module), path)
    }

    /// Resolve a type path from either signature or body source.
    pub fn ty_for_indexed_type_path(
        &self,
        type_path: &IndexedTypePath,
    ) -> anyhow::Result<IndexedType> {
        if let Some(type_ref) = type_path.type_ref() {
            let ty = match type_path.scope() {
                IndexedTypePathScope::Signature(scope) => {
                    self.ty_for_signature_type_ref(scope, type_ref)?
                }
                IndexedTypePathScope::Body(scope) => BodyResolutionView::new(self.db)
                    .type_ref_ty(scope.body_ir(), scope.scope_id(), type_ref)?
                    .unwrap_or(Ty::Unknown),
            };
            return Ok(IndexedType::new(ty));
        }

        match type_path.scope() {
            IndexedTypePathScope::Signature(scope) => {
                self.ty_for_type_path(scope.context(), type_path.path())
            }
            IndexedTypePathScope::Body(scope) => {
                self.ty_for_body_type_path(scope.body_ir(), scope.scope_id(), type_path.path())
            }
        }
    }

    /// Lower the complete type spelling from an item signature in its declaration scope.
    fn ty_for_signature_type_ref(
        &self,
        scope: IndexedSignatureTypeScope,
        type_ref: &rg_item_tree::TypeRef,
    ) -> anyhow::Result<Ty> {
        let item_paths = ItemPathQuery::new(self.db, self.db);
        TypeLoweringQuery::new(&item_paths, self.db)
            .lower(
                type_ref,
                TypeLoweringEnv::new(
                    scope.generic_owner(),
                    TypeLoweringAnchor::Context(scope.context()),
                ),
            )
            .context("lower signature type reference")
    }

    /// Resolve a body type path into an indexed type.
    pub fn ty_for_body_type_path(
        &self,
        body_ref: BodyRef,
        scope: ScopeId,
        path: &Path,
    ) -> anyhow::Result<IndexedType> {
        let resolution = BodyResolutionView::new(self.db)
            .type_path_resolution(body_ref, scope, path)
            .context("resolve body type path")?
            .unwrap_or(TypePathResolution::Unknown);
        if matches!(resolution, TypePathResolution::Unknown)
            && let Some(primitive) = path.single_name().and_then(PrimitiveTy::from_name)
        {
            return Ok(IndexedType::new(Ty::Primitive(primitive)));
        }

        Ok(IndexedType::new(
            self.type_path_resolution_to_ty(resolution)?,
        ))
    }

    /// Resolve a body value path into its expression type.
    pub fn ty_for_body_value_path(
        &self,
        body_ref: BodyRef,
        scope: ScopeId,
        path: &Path,
    ) -> anyhow::Result<IndexedType> {
        // Value-path type queries should use the same Body IR resolver as the main body pass, so
        // enum variants and associated functions agree between snapshots and cursor queries.
        Ok(IndexedType::new(
            BodyResolutionView::new(self.db)
                .nonlocal_value_path_ty(body_ref, scope, path)
                .context("resolve body value path type")?,
        ))
    }

    /// Resolve the declared type of a field.
    fn ty_for_field(&self, field: FieldRef) -> anyhow::Result<Option<Ty>> {
        SemanticSignatureQuery::with_resolver(self.db, self.db, self.db)
            .field_ty(field)
            .map_err(Into::into)
    }

    /// Return the owning enum type for an enum variant constructor.
    fn ty_for_enum_variant(&self, variant: EnumVariantRef) -> anyhow::Result<Option<Ty>> {
        let Some(data) = ItemStoreQuery::new(self.db)
            .enum_variant_data(variant)
            .context("look up enum variant data")?
        else {
            return Ok(None);
        };
        Ok(Some(Ty::adt(AdtTy::bare(data.owner))))
    }

    /// Convert a type-path result to `Ty`, lowering transparent aliases through their declaration.
    fn type_path_resolution_to_ty(&self, resolution: TypePathResolution) -> anyhow::Result<Ty> {
        if let TypePathResolution::TypeAlias(alias) = resolution {
            return Ok(
                SemanticSignatureQuery::with_resolver(self.db, self.db, self.db)
                    .type_alias_ty(alias)
                    .context("lower transparent type alias")?
                    .unwrap_or(Ty::Unknown),
            );
        }

        Ok(Ty::from_type_path_resolution(resolution, Vec::new()).unwrap_or(Ty::Unknown))
    }

    /// Open the body-local type view.
    fn body_view(&self) -> BodyView<'a, 'db> {
        BodyView::new(self.db)
    }
}
