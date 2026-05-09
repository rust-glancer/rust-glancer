//! Cursor-oriented queries over semantic item signatures.
//!
//! Analysis owns the user-facing `SymbolAt` enum, but semantic IR owns the shape of item
//! signatures. Keeping this scan here prevents analysis from knowing how every semantic item stores
//! generic params, field types, enum variants, impl headers, and associated function declarations.

use rg_def_map::{Path, TargetRef};
use rg_item_tree::{
    FieldList, GenericArg, GenericParams, TypeBound, TypePath, TypeRef, WherePredicate,
};
use rg_package_store::PackageStoreError;
use rg_parse::{FileId, Span};

use crate::{
    EnumVariantRef, FieldRef, FunctionRef, ItemOwner, SemanticIrReadTxn, TypeDefId, TypeDefRef,
    TypePathContext,
};

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
            file_id,
            offset,
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
    file_id: FileId,
    offset: u32,
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
        for (ty, data) in self.semantic_ir.structs(self.target)? {
            if data.source.file_id != self.file_id {
                continue;
            }
            let context = TypePathContext::module(data.owner);
            self.scan_generic_params(context, &data.generics);
            self.scan_field_list(ty, context, &data.fields);
        }

        Ok(())
    }

    fn scan_unions(&mut self) -> Result<(), PackageStoreError> {
        for (ty, data) in self.semantic_ir.unions(self.target)? {
            if data.source.file_id != self.file_id {
                continue;
            }
            let context = TypePathContext::module(data.owner);
            self.scan_generic_params(context, &data.generics);
            for (field_idx, field) in data.fields.iter().enumerate() {
                self.push_field(
                    FieldRef {
                        owner: ty,
                        index: field_idx,
                    },
                    field.span,
                );
                self.push_type_ref(context, &field.ty);
            }
        }

        Ok(())
    }

    fn scan_enums(&mut self) -> Result<(), PackageStoreError> {
        for (ty, data) in self.semantic_ir.enums(self.target)? {
            if data.source.file_id != self.file_id {
                continue;
            }
            let context = TypePathContext::module(data.owner);
            self.scan_generic_params(context, &data.generics);
            let TypeDefId::Enum(enum_id) = ty.id else {
                continue;
            };
            for (variant_idx, variant) in data.variants.iter().enumerate() {
                self.push_enum_variant(
                    EnumVariantRef {
                        target: self.target,
                        enum_id,
                        index: variant_idx,
                    },
                    variant.name_span,
                );
                self.scan_field_list_for_owner(context, &variant.fields);
            }
        }

        Ok(())
    }

    fn scan_traits(&mut self) -> Result<(), PackageStoreError> {
        for (_, data) in self.semantic_ir.traits(self.target)? {
            if data.source.file_id != self.file_id {
                continue;
            }
            let context = TypePathContext::module(data.owner);
            self.scan_generic_params(context, &data.generics);
            self.scan_type_bounds(context, &data.super_traits);
        }

        Ok(())
    }

    fn scan_impls(&mut self) -> Result<(), PackageStoreError> {
        for (impl_ref, data) in self.semantic_ir.impls(self.target)? {
            if data.source.file_id != self.file_id {
                continue;
            }
            let Some(context) = self.owner_context(ItemOwner::Impl(impl_ref.id))? else {
                continue;
            };
            self.scan_generic_params(context, &data.generics);
            if let Some(trait_ref) = &data.trait_ref {
                self.push_type_ref(context, trait_ref);
            }
            self.push_type_ref(context, &data.self_ty);
        }

        Ok(())
    }

    fn scan_functions(&mut self) -> Result<(), PackageStoreError> {
        for (function_ref, data) in self.semantic_ir.functions(self.target)? {
            if data.source.file_id != self.file_id {
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
                self.scan_generic_params(context, generics);
            }
            for param in data.signature.params() {
                if let Some(ty) = &param.ty {
                    self.push_type_ref(context, ty);
                }
            }
            if let Some(ret_ty) = data.signature.ret_ty() {
                self.push_type_ref(context, ret_ty);
            }
        }

        Ok(())
    }

    fn scan_type_aliases(&mut self) -> Result<(), PackageStoreError> {
        for (_, data) in self.semantic_ir.type_aliases(self.target)? {
            if data.source.file_id != self.file_id {
                continue;
            }
            let Some(context) = self.owner_context(data.owner)? else {
                continue;
            };
            if let Some(generics) = data.signature.generics() {
                self.scan_generic_params(context, generics);
            }
            self.scan_type_bounds(context, data.signature.bounds());
            if let Some(ty) = data.signature.aliased_ty() {
                self.push_type_ref(context, ty);
            }
        }

        Ok(())
    }

    fn scan_consts(&mut self) -> Result<(), PackageStoreError> {
        for (_, data) in self.semantic_ir.consts(self.target)? {
            if data.source.file_id != self.file_id {
                continue;
            }
            let Some(context) = self.owner_context(data.owner)? else {
                continue;
            };
            if let Some(ty) = data.signature.ty() {
                self.push_type_ref(context, ty);
            }
        }

        Ok(())
    }

    fn scan_statics(&mut self) -> Result<(), PackageStoreError> {
        for (_, data) in self.semantic_ir.statics(self.target)? {
            if data.source.file_id != self.file_id {
                continue;
            }
            if let Some(ty) = &data.ty {
                self.push_type_ref(TypePathContext::module(data.owner), ty);
            }
        }

        Ok(())
    }

    fn scan_field_list(&mut self, owner: TypeDefRef, context: TypePathContext, fields: &FieldList) {
        for (idx, field) in fields.fields().iter().enumerate() {
            self.push_field(FieldRef { owner, index: idx }, field.span);
            self.push_type_ref(context, &field.ty);
        }
    }

    fn scan_field_list_for_owner(&mut self, context: TypePathContext, fields: &FieldList) {
        for field in fields.fields() {
            self.push_type_ref(context, &field.ty);
        }
    }

    fn scan_generic_params(&mut self, context: TypePathContext, generics: &GenericParams) {
        for param in &generics.types {
            self.scan_type_bounds(context, &param.bounds);
            if let Some(default) = &param.default {
                self.push_type_ref(context, default);
            }
        }
        for param in &generics.consts {
            if let Some(ty) = &param.ty {
                self.push_type_ref(context, ty);
            }
        }
        for predicate in &generics.where_predicates {
            match predicate {
                WherePredicate::Type { ty, bounds } => {
                    self.push_type_ref(context, ty);
                    self.scan_type_bounds(context, bounds);
                }
                WherePredicate::Lifetime { .. } | WherePredicate::Unsupported(_) => {}
            }
        }
    }

    fn scan_type_bounds(&mut self, context: TypePathContext, bounds: &[TypeBound]) {
        for bound in bounds {
            match bound {
                TypeBound::Trait(ty) => self.push_type_ref(context, ty),
                TypeBound::Lifetime(_) | TypeBound::Unsupported(_) => {}
            }
        }
    }

    fn push_type_ref(&mut self, context: TypePathContext, ty: &TypeRef) {
        match ty {
            TypeRef::Path(path) => self.push_type_path(context, path),
            TypeRef::Tuple(types) => {
                for ty in types {
                    self.push_type_ref(context, ty);
                }
            }
            TypeRef::Reference { inner, .. }
            | TypeRef::RawPointer { inner, .. }
            | TypeRef::Slice(inner) => self.push_type_ref(context, inner),
            TypeRef::Array { inner, .. } => self.push_type_ref(context, inner),
            TypeRef::FnPointer { params, ret } => {
                for param in params {
                    self.push_type_ref(context, param);
                }
                self.push_type_ref(context, ret);
            }
            TypeRef::ImplTrait(bounds) | TypeRef::DynTrait(bounds) => {
                self.scan_type_bounds(context, bounds);
            }
            TypeRef::Unknown(_) | TypeRef::Never | TypeRef::Unit | TypeRef::Infer => {}
        }
    }

    fn push_type_path(&mut self, context: TypePathContext, path: &TypePath) {
        for (idx, segment) in path.segments.iter().enumerate() {
            if segment.span.touches(self.offset) {
                self.push_candidate(SemanticCursorCandidate::TypePath {
                    context,
                    path: Path::from_type_path_prefix(path, idx),
                    span: segment.span,
                });
            }

            for arg in &segment.args {
                self.push_generic_arg(context, arg);
            }
        }
    }

    fn push_generic_arg(&mut self, context: TypePathContext, arg: &GenericArg) {
        match arg {
            GenericArg::Type(ty) => self.push_type_ref(context, ty),
            GenericArg::AssocType { ty: Some(ty), .. } => self.push_type_ref(context, ty),
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
        if candidate.span().touches(self.offset) {
            self.candidates.push(candidate);
        }
    }

    fn owner_context(
        &self,
        owner: ItemOwner,
    ) -> Result<Option<TypePathContext>, PackageStoreError> {
        self.semantic_ir
            .type_path_context_for_owner(self.target, owner)
    }
}
