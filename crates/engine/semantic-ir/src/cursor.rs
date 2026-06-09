//! Cursor-oriented queries over semantic item signatures.
//!
//! Analysis owns the user-facing `SymbolAt` enum, but semantic IR owns the shape of item
//! signatures. Keeping this scan here prevents analysis from knowing how every semantic item stores
//! generic params, field types, enum variants, impl headers, and associated function declarations.

use rg_ir_model::Path;
use rg_ir_model::{DefMapRef, TargetRef};
use rg_ir_model::{EnumVariantRef, FieldRef, FunctionRef, ItemOwner, TypeDefId, TypeDefRef};
use rg_ir_storage::{ItemStoreQuery, TypePathContext};
use rg_item_tree::{
    FieldList, GenericArg, GenericParams, TypeBound, TypePath, TypeRef, WherePredicate,
};
use rg_package_store::PackageStoreError;
use rg_parse::{FileId, Span};

use crate::SemanticIrReadTxn;

/// One semantic signature source node that can participate in cursor queries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SemanticCursorCandidate {
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

impl SemanticCursorCandidate {
    fn span(&self) -> Span {
        match self {
            Self::Field { span, .. }
            | Self::Function { span, .. }
            | Self::EnumVariant { span, .. }
            | Self::TypePath { span, .. } => *span,
        }
    }
}

impl SemanticIrReadTxn<'_> {
    /// Returns cursor candidates inside semantic item signatures.
    pub fn signature_cursor_candidates(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> Result<Vec<SemanticCursorCandidate>, PackageStoreError> {
        let mut candidates = Vec::new();
        SignatureCursorScanner {
            semantic_ir: self,
            target,
            file_id: Some(file_id),
            offset: Some(offset),
            candidates: &mut candidates,
        }
        .scan()?;

        Ok(candidates)
    }

    /// Returns source candidates inside semantic item signatures for one target.
    pub fn signature_source_candidates(
        &self,
        target: TargetRef,
        file_id: Option<FileId>,
    ) -> Result<Vec<SemanticCursorCandidate>, PackageStoreError> {
        let mut candidates = Vec::new();
        SignatureCursorScanner {
            semantic_ir: self,
            target,
            file_id,
            offset: None,
            candidates: &mut candidates,
        }
        .scan()?;

        Ok(candidates)
    }
}

/// Scans semantic item signatures for names and type paths under the cursor.
struct SignatureCursorScanner<'txn, 'db> {
    semantic_ir: &'txn SemanticIrReadTxn<'db>,
    target: TargetRef,
    file_id: Option<FileId>,
    offset: Option<u32>,
    candidates: &'txn mut Vec<SemanticCursorCandidate>,
}

impl SignatureCursorScanner<'_, '_> {
    fn scan(&mut self) -> Result<(), PackageStoreError> {
        self.scan_structs()?;
        self.scan_unions()?;
        self.scan_enums()?;
        self.scan_traits()?;
        self.scan_impls()?;
        self.scan_functions()?;
        self.scan_type_aliases()?;
        self.scan_consts()?;
        self.scan_statics()?;
        Ok(())
    }

    fn scan_structs(&mut self) -> Result<(), PackageStoreError> {
        let target = self.target;
        let origin = DefMapRef::Target(target);
        for (ty, data) in self
            .semantic_ir
            .items(target)?
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
        let target = self.target;
        let origin = DefMapRef::Target(target);
        for (ty, data) in self
            .semantic_ir
            .items(target)?
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
        let target = self.target;
        let origin = DefMapRef::Target(target);
        for (ty, data) in self
            .semantic_ir
            .items(target)?
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
                        origin: DefMapRef::Target(self.target),
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
        let Some(items) = self.semantic_ir.items(self.target)? else {
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
        let Some(items) = self.semantic_ir.items(self.target)? else {
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
        let Some(items) = self.semantic_ir.items(self.target)? else {
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
            .items(self.target)?
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
            .items(self.target)?
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
            .items(self.target)?
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
        for param in &generics.types {
            self.scan_type_bounds(context, &param.bounds, file_id);
            if let Some(default) = &param.default {
                self.push_type_ref(context, default, file_id);
            }
        }
        for param in &generics.consts {
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

    fn push_type_path(&mut self, context: TypePathContext, path: &TypePath, file_id: FileId) {
        for (idx, segment) in path.segments.iter().enumerate() {
            if self.offset_matches(segment.span) {
                self.push_candidate(SemanticCursorCandidate::TypePath {
                    context,
                    path: Path::from_type_path_prefix(path, idx),
                    file_id,
                    span: segment.span,
                });
            }

            for arg in &segment.args {
                self.push_generic_arg(context, arg, file_id);
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
        self.push_candidate(SemanticCursorCandidate::Field { field, span });
    }

    fn push_function(&mut self, function: FunctionRef, span: Span) {
        self.push_candidate(SemanticCursorCandidate::Function { function, span });
    }

    fn push_enum_variant(&mut self, variant: EnumVariantRef, span: Span) {
        self.push_candidate(SemanticCursorCandidate::EnumVariant { variant, span });
    }

    fn push_candidate(&mut self, candidate: SemanticCursorCandidate) {
        if self.offset_matches(candidate.span()) {
            self.candidates.push(candidate);
        }
    }

    fn owner_context(
        &self,
        owner: ItemOwner,
    ) -> Result<Option<TypePathContext>, PackageStoreError> {
        ItemStoreQuery::new(self.semantic_ir)
            .type_path_context_for_owner(DefMapRef::Target(self.target), owner)
    }

    fn file_matches(&self, file_id: FileId) -> bool {
        self.file_id.is_none_or(|selected| selected == file_id)
    }

    fn offset_matches(&self, span: Span) -> bool {
        self.offset.is_none_or(|offset| span.touches(offset))
    }
}
