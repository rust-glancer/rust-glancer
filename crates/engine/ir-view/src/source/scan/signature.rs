//! Source scanning over semantic item signatures.
//!
//! Semantic IR owns item signature data, while the indexed view owns the source interpretation of
//! that data. This scanner finds fields, variants, associated functions, and nested type paths
//! without making source-query APIs part of Semantic IR's storage transaction.
//!
//! ```text
//! struct Wrapper<T> {
//!     value: outer::Inner<Vec<T>>,
//!     ^^^^^  ^^^^^  ^^^^^ ^^^ ^ fields and every nested type path remain independently navigable
//! }
//! ```

use rg_ir_model::{
    CrateRef, DefMapRef, EnumVariantRef, FieldRef, FunctionRef, ItemOwner, Path, TypeDefId,
    TypeDefRef,
    items::{
        FieldList, GenericArg, GenericParams, TypeBound, TypePath, TypePathAnchor, TypeRef,
        WherePredicate,
    },
};
use rg_package_store::PackageStoreError;
use rg_parse::{FileId, Span};
use rg_semantic_ir::{ItemStoreQuery, SemanticIrReadTxn, TypePathContext};

/// One semantic signature source node that can become an indexed occurrence.
///
/// Top-level item names already come from DefMap. Semantic signatures add names owned below that
/// boundary—fields, variants, associated functions—and type paths that need their item's generic
/// and impl context for resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SignatureSourceCandidate {
    Field {
        field: FieldRef,
        span: Span,
    },
    Function {
        function: FunctionRef,
        span: Span,
    },
    EnumVariant {
        variant: EnumVariantRef,
        span: Span,
    },
    TypePath {
        context: TypePathContext,
        path: Path,
        file_id: FileId,
        span: Span,
    },
}

impl SignatureSourceCandidate {
    fn span(&self) -> Span {
        match self {
            Self::Field { span, .. }
            | Self::Function { span, .. }
            | Self::EnumVariant { span, .. }
            | Self::TypePath { span, .. } => *span,
        }
    }
}

/// Scans semantic item signatures for declaration names and nested type paths.
///
/// Type references are walked recursively through generic defaults, bounds, where predicates,
/// fields, parameters, return types, and qualified anchors. For `Outer<Inner>`, both paths are
/// emitted rather than treating the annotation as one opaque source span.
pub(crate) struct SignatureSourceScanner<'txn, 'db> {
    semantic_ir: &'txn SemanticIrReadTxn<'db>,
    crate_ref: CrateRef,
    file_id: Option<FileId>,
    offset: Option<u32>,
    candidates: Vec<SignatureSourceCandidate>,
}

impl<'txn, 'db> SignatureSourceScanner<'txn, 'db> {
    pub(crate) fn at(
        semantic_ir: &'txn SemanticIrReadTxn<'db>,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> Self {
        Self {
            semantic_ir,
            crate_ref,
            file_id: Some(file_id),
            offset: Some(offset),
            candidates: Vec::new(),
        }
    }

    pub(crate) fn in_crate(
        semantic_ir: &'txn SemanticIrReadTxn<'db>,
        crate_ref: CrateRef,
        file_id: Option<FileId>,
    ) -> Self {
        Self {
            semantic_ir,
            crate_ref,
            file_id,
            offset: None,
            candidates: Vec::new(),
        }
    }

    /// Collects the source facts owned by each semantic item family in the crate.
    pub(crate) fn scan(mut self) -> Result<Vec<SignatureSourceCandidate>, PackageStoreError> {
        self.scan_structs()?;
        self.scan_unions()?;
        self.scan_enums()?;
        self.scan_traits()?;
        self.scan_impls()?;
        self.scan_functions()?;
        self.scan_type_aliases()?;
        self.scan_consts()?;
        self.scan_statics()?;
        Ok(self.candidates)
    }

    fn scan_structs(&mut self) -> Result<(), PackageStoreError> {
        let crate_ref = self.crate_ref;
        let origin = DefMapRef::Crate(crate_ref);
        for (ty, data) in self
            .semantic_ir
            .items(crate_ref)?
            .into_iter()
            .flat_map(move |items| {
                items.structs().iter_with_ids().map(move |(id, data)| {
                    (
                        TypeDefRef {
                            origin,
                            id: TypeDefId::Struct(id),
                        },
                        data,
                    )
                })
            })
        {
            if !self.file_matches(data.source.file_id) {
                continue;
            }
            let context = TypePathContext::module(data.owner);
            self.scan_generic_params(context, &data.generics, data.source.file_id);
            self.scan_field_list(ty, context, &data.fields, data.source.file_id);
        }

        Ok(())
    }

    fn scan_unions(&mut self) -> Result<(), PackageStoreError> {
        let crate_ref = self.crate_ref;
        let origin = DefMapRef::Crate(crate_ref);
        for (ty, data) in self
            .semantic_ir
            .items(crate_ref)?
            .into_iter()
            .flat_map(move |items| {
                items.unions().iter_with_ids().map(move |(id, data)| {
                    (
                        TypeDefRef {
                            origin,
                            id: TypeDefId::Union(id),
                        },
                        data,
                    )
                })
            })
        {
            if !self.file_matches(data.source.file_id) {
                continue;
            }
            let context = TypePathContext::module(data.owner);
            self.scan_generic_params(context, &data.generics, data.source.file_id);
            for (field_idx, field) in data.fields.iter().enumerate() {
                self.push_field(
                    FieldRef {
                        owner: ty,
                        index: field_idx,
                    },
                    field.span,
                );
                self.push_type_ref(context, &field.ty, data.source.file_id);
            }
        }

        Ok(())
    }

    fn scan_enums(&mut self) -> Result<(), PackageStoreError> {
        let crate_ref = self.crate_ref;
        let origin = DefMapRef::Crate(crate_ref);
        for (ty, data) in self
            .semantic_ir
            .items(crate_ref)?
            .into_iter()
            .flat_map(move |items| {
                items.enums().iter_with_ids().map(move |(id, data)| {
                    (
                        TypeDefRef {
                            origin,
                            id: TypeDefId::Enum(id),
                        },
                        data,
                    )
                })
            })
        {
            if !self.file_matches(data.source.file_id) {
                continue;
            }
            let context = TypePathContext::module(data.owner);
            self.scan_generic_params(context, &data.generics, data.source.file_id);
            let TypeDefId::Enum(enum_id) = ty.id else {
                continue;
            };
            for (variant_idx, variant) in data.variants.iter().enumerate() {
                self.push_enum_variant(
                    EnumVariantRef {
                        origin: DefMapRef::Crate(self.crate_ref),
                        enum_id,
                        index: variant_idx,
                    },
                    variant.name_span,
                );
                self.scan_field_list_for_owner(context, &variant.fields, data.source.file_id);
            }
        }

        Ok(())
    }

    fn scan_traits(&mut self) -> Result<(), PackageStoreError> {
        let Some(items) = self.semantic_ir.items(self.crate_ref)? else {
            return Ok(());
        };

        for data in items.traits() {
            if !self.file_matches(data.source.file_id) {
                continue;
            }
            let context = TypePathContext::module(data.owner);
            self.scan_generic_params(context, &data.generics, data.source.file_id);
            self.scan_type_bounds(context, &data.super_traits, data.source.file_id);
        }

        Ok(())
    }

    fn scan_impls(&mut self) -> Result<(), PackageStoreError> {
        let Some(items) = self.semantic_ir.items(self.crate_ref)? else {
            return Ok(());
        };

        for (impl_ref, data) in items.impls_with_refs() {
            if !self.file_matches(data.source.file_id) {
                continue;
            }
            let Some(context) = self.owner_context(ItemOwner::Impl(impl_ref.id))? else {
                continue;
            };
            self.scan_generic_params(context, &data.generics, data.source.file_id);
            if let Some(trait_ref) = &data.trait_ref {
                self.push_type_ref(context, trait_ref, data.source.file_id);
            }
            self.push_type_ref(context, &data.self_ty, data.source.file_id);
        }

        Ok(())
    }

    fn scan_functions(&mut self) -> Result<(), PackageStoreError> {
        let Some(items) = self.semantic_ir.items(self.crate_ref)? else {
            return Ok(());
        };

        for (function_ref, data) in items.functions_with_refs() {
            if !self.file_matches(data.source.file_id) {
                continue;
            }
            if data.local_def.is_none() {
                let span = data.name_span.unwrap_or(data.span);
                self.push_function(function_ref, span);
            }
            let Some(context) = self.owner_context(data.owner)? else {
                continue;
            };
            if let Some(generics) = data.signature.generics() {
                self.scan_generic_params(context, generics, data.source.file_id);
            }
            for param in data.signature.params() {
                if let Some(ty) = &param.ty {
                    self.push_type_ref(context, ty, data.source.file_id);
                }
            }
            if let Some(ret_ty) = data.signature.ret_ty() {
                self.push_type_ref(context, ret_ty, data.source.file_id);
            }
        }

        Ok(())
    }

    fn scan_type_aliases(&mut self) -> Result<(), PackageStoreError> {
        for data in self
            .semantic_ir
            .items(self.crate_ref)?
            .into_iter()
            .flat_map(move |items| items.type_aliases().iter())
        {
            if !self.file_matches(data.source.file_id) {
                continue;
            }
            let Some(context) = self.owner_context(data.owner)? else {
                continue;
            };
            if let Some(generics) = data.signature.generics() {
                self.scan_generic_params(context, generics, data.source.file_id);
            }
            self.scan_type_bounds(context, data.signature.bounds(), data.source.file_id);
            if let Some(ty) = data.signature.aliased_ty() {
                self.push_type_ref(context, ty, data.source.file_id);
            }
        }

        Ok(())
    }

    fn scan_consts(&mut self) -> Result<(), PackageStoreError> {
        for data in self
            .semantic_ir
            .items(self.crate_ref)?
            .into_iter()
            .flat_map(move |items| items.consts().iter())
        {
            if !self.file_matches(data.source.file_id) {
                continue;
            }
            let Some(context) = self.owner_context(data.owner)? else {
                continue;
            };
            if let Some(ty) = data.signature.ty() {
                self.push_type_ref(context, ty, data.source.file_id);
            }
        }

        Ok(())
    }

    fn scan_statics(&mut self) -> Result<(), PackageStoreError> {
        for data in self
            .semantic_ir
            .items(self.crate_ref)?
            .into_iter()
            .flat_map(move |items| items.statics().iter())
        {
            if !self.file_matches(data.source.file_id) {
                continue;
            }
            if let Some(ty) = &data.ty {
                self.push_type_ref(TypePathContext::module(data.owner), ty, data.source.file_id);
            }
        }

        Ok(())
    }

    fn scan_field_list(
        &mut self,
        owner: TypeDefRef,
        context: TypePathContext,
        fields: &FieldList,
        file_id: FileId,
    ) {
        for (idx, field) in fields.fields().iter().enumerate() {
            self.push_field(FieldRef { owner, index: idx }, field.span);
            self.push_type_ref(context, &field.ty, file_id);
        }
    }

    fn scan_field_list_for_owner(
        &mut self,
        context: TypePathContext,
        fields: &FieldList,
        file_id: FileId,
    ) {
        for field in fields.fields() {
            self.push_type_ref(context, &field.ty, file_id);
        }
    }

    fn scan_generic_params(
        &mut self,
        context: TypePathContext,
        generics: &GenericParams,
        file_id: FileId,
    ) {
        for param in generics.types() {
            self.scan_type_bounds(context, &param.bounds, file_id);
            if let Some(default) = &param.default {
                self.push_type_ref(context, default, file_id);
            }
        }
        for param in generics.consts() {
            if let Some(ty) = &param.ty {
                self.push_type_ref(context, ty, file_id);
            }
        }
        for predicate in &generics.where_predicates {
            match predicate {
                WherePredicate::Type { ty, bounds } => {
                    self.push_type_ref(context, ty, file_id);
                    self.scan_type_bounds(context, bounds, file_id);
                }
                WherePredicate::Lifetime { .. } | WherePredicate::Unsupported(_) => {}
            }
        }
    }

    fn scan_type_bounds(
        &mut self,
        context: TypePathContext,
        bounds: &[TypeBound],
        file_id: FileId,
    ) {
        for bound in bounds {
            match bound {
                TypeBound::Trait(ty) => self.push_type_ref(context, ty, file_id),
                TypeBound::Lifetime(_) | TypeBound::Unsupported(_) => {}
            }
        }
    }

    fn push_type_ref(&mut self, context: TypePathContext, ty: &TypeRef, file_id: FileId) {
        match ty {
            TypeRef::Path(path) => self.push_type_path(context, path, file_id),
            TypeRef::Tuple(types) => {
                for ty in types {
                    self.push_type_ref(context, ty, file_id);
                }
            }
            TypeRef::Reference { inner, .. }
            | TypeRef::RawPointer { inner, .. }
            | TypeRef::Slice(inner) => self.push_type_ref(context, inner, file_id),
            TypeRef::Array { inner, .. } => self.push_type_ref(context, inner, file_id),
            TypeRef::FnPointer { params, ret } => {
                for param in params {
                    self.push_type_ref(context, param, file_id);
                }
                self.push_type_ref(context, ret, file_id);
            }
            TypeRef::ImplTrait(bounds) | TypeRef::DynTrait(bounds) => {
                self.scan_type_bounds(context, bounds, file_id);
            }
            TypeRef::Unknown(_) | TypeRef::Never | TypeRef::Unit | TypeRef::Infer => {}
        }
    }

    /// Emits each unanchored path prefix and then descends into the segment's generic arguments.
    ///
    /// For `outer::Inner<Vec<T>>`, segment occurrences resolve as `outer` and `outer::Inner`; `Vec`
    /// and `T` are visited recursively as their own paths.
    fn push_type_path(&mut self, context: TypePathContext, path: &TypePath, file_id: FileId) {
        if let Some(anchor) = &path.anchor {
            self.push_type_path_anchor(context, anchor, file_id);
        }

        for (idx, segment) in path.segments.iter().enumerate() {
            if path.anchor.is_none()
                && self.offset_matches(segment.span)
                && let Some(def_map_path) = Path::from_type_path_prefix(path, idx)
            {
                self.push_candidate(SignatureSourceCandidate::TypePath {
                    context,
                    path: def_map_path,
                    file_id,
                    span: segment.span,
                });
            }

            for arg in &segment.args {
                self.push_generic_arg(context, arg, file_id);
            }
        }
    }

    fn push_type_path_anchor(
        &mut self,
        context: TypePathContext,
        anchor: &TypePathAnchor,
        file_id: FileId,
    ) {
        match anchor {
            TypePathAnchor::Type(ty) => self.push_type_ref(context, ty, file_id),
            TypePathAnchor::QualifiedTrait { self_ty, trait_ty } => {
                self.push_type_ref(context, self_ty, file_id);
                self.push_type_ref(context, trait_ty, file_id);
            }
        }
    }

    fn push_generic_arg(&mut self, context: TypePathContext, arg: &GenericArg, file_id: FileId) {
        match arg {
            GenericArg::Type(ty) => self.push_type_ref(context, ty, file_id),
            GenericArg::FnTraitArgs { params, ret } => {
                for param in params {
                    self.push_type_ref(context, param, file_id);
                }
                self.push_type_ref(context, ret, file_id);
            }
            GenericArg::AssocType { ty: Some(ty), .. } => {
                self.push_type_ref(context, ty, file_id);
            }
            GenericArg::Lifetime(_)
            | GenericArg::Const(_)
            | GenericArg::AssocType { ty: None, .. }
            | GenericArg::Unsupported(_) => {}
        }
    }

    fn push_field(&mut self, field: FieldRef, span: Span) {
        self.push_candidate(SignatureSourceCandidate::Field { field, span });
    }

    fn push_function(&mut self, function: FunctionRef, span: Span) {
        self.push_candidate(SignatureSourceCandidate::Function { function, span });
    }

    fn push_enum_variant(&mut self, variant: EnumVariantRef, span: Span) {
        self.push_candidate(SignatureSourceCandidate::EnumVariant { variant, span });
    }

    fn push_candidate(&mut self, candidate: SignatureSourceCandidate) {
        if self.offset_matches(candidate.span()) {
            self.candidates.push(candidate);
        }
    }

    fn owner_context(
        &self,
        owner: ItemOwner,
    ) -> Result<Option<TypePathContext>, PackageStoreError> {
        ItemStoreQuery::new(self.semantic_ir)
            .type_path_context_for_owner(DefMapRef::Crate(self.crate_ref), owner)
    }

    fn file_matches(&self, file_id: FileId) -> bool {
        self.file_id.is_none_or(|selected| selected == file_id)
    }

    fn offset_matches(&self, span: Span) -> bool {
        self.offset.is_none_or(|offset| span.touches(offset))
    }
}
