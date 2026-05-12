use crate::{
    AssocItemId, ConstData, ConstId, ConstRef, ConstSignature, EnumData, EnumId, EnumVariantRef,
    FieldRef, FunctionData, FunctionId, FunctionRef, FunctionSignature, ImplData, ImplId, ImplRef,
    ItemId, ItemOwner, ItemStore, PackageIr, SemanticIrDb, SemanticIrPackageBundle,
    SemanticIrStats, SemanticTypePathResolution, StaticData, StaticId, StaticRef, StructData,
    StructId, TargetIr, TraitApplicability, TraitId, TraitImplRef, TraitRef, TypeAliasData,
    TypeAliasId, TypeAliasRef, TypeAliasSignature, TypeDefId, TypeDefRef, TypePathContext,
    UnionData, UnionId, ir::SignatureGenerics,
};
use rg_memsize::{MemoryRecorder, MemorySize};

rg_memsize::impl_memory_size_leaf!(
    StructId,
    UnionId,
    EnumId,
    TraitId,
    ImplId,
    FunctionId,
    TypeAliasId,
    ConstId,
    StaticId,
    TraitApplicability,
);

rg_memsize::impl_memory_size_children! {
    PackageIr => targets;
    TargetIr => local_items, local_impls, items;
    ItemStore => structs, unions, enums, traits, impls, functions, type_aliases, consts, statics;
    StructData => local_def, source, owner, name, visibility, docs, generics, fields;
    UnionData => local_def, source, owner, name, visibility, docs, generics, fields;
    EnumData => local_def, source, owner, name, visibility, docs, generics, variants;
    crate::TraitData => local_def, source, owner, name, visibility, docs, generics,
        super_traits, items, is_unsafe;
    ImplData => local_impl, source, owner, generics, trait_ref, self_ty, resolved_self_tys,
        resolved_trait_refs, items, is_unsafe;
    FunctionData => local_def, source, span, name_span, owner, name, visibility, docs, signature;
    FunctionSignature => generics, params, ret_ty, qualifiers;
    TypeAliasData => local_def, source, span, name_span, owner, name, visibility, docs, signature;
    TypeAliasSignature => generics, bounds, aliased_ty;
    ConstData => local_def, source, span, name_span, owner, name, visibility, docs, signature;
    ConstSignature => ty;
    StaticData => local_def, source, span, name_span, owner, name, visibility, docs, ty,
        mutability;
    TypeDefRef => target, id;
    TraitRef => target, id;
    ImplRef => target, id;
    FunctionRef => target, id;
    TypeAliasRef => target, id;
    ConstRef => target, id;
    StaticRef => target, id;
    FieldRef => owner, index;
    EnumVariantRef => target, enum_id, index;
    TraitImplRef => impl_ref, trait_ref;
    TypePathContext => module, impl_ref;
    SemanticIrStats => target_count, struct_count, union_count, enum_count, trait_count,
        impl_count, function_count, type_alias_count, const_count, static_count;
}

impl MemorySize for SemanticIrDb {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        recorder.scope("packages", |recorder| {
            self.record_packages_memory_children(recorder);
        });
    }
}

impl MemorySize for SemanticIrPackageBundle {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        recorder.scope("package", |recorder| {
            self.package().record_memory_children(recorder);
        });
    }
}

impl MemorySize for SignatureGenerics {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            Self::Empty => {}
            Self::Present(params) => params.record_memory_children(recorder),
        }
    }
}

impl MemorySize for TypeDefId {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            Self::Struct(id) => id.record_memory_children(recorder),
            Self::Enum(id) => id.record_memory_children(recorder),
            Self::Union(id) => id.record_memory_children(recorder),
        }
    }
}

impl MemorySize for ItemId {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            Self::Struct(id) => id.record_memory_children(recorder),
            Self::Union(id) => id.record_memory_children(recorder),
            Self::Enum(id) => id.record_memory_children(recorder),
            Self::Trait(id) => id.record_memory_children(recorder),
            Self::Function(id) => id.record_memory_children(recorder),
            Self::TypeAlias(id) => id.record_memory_children(recorder),
            Self::Const(id) => id.record_memory_children(recorder),
            Self::Static(id) => id.record_memory_children(recorder),
        }
    }
}

impl MemorySize for AssocItemId {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            Self::Function(id) => id.record_memory_children(recorder),
            Self::TypeAlias(id) => id.record_memory_children(recorder),
            Self::Const(id) => id.record_memory_children(recorder),
        }
    }
}

impl MemorySize for ItemOwner {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            Self::Module(module) => module.record_memory_children(recorder),
            Self::Trait(trait_id) => trait_id.record_memory_children(recorder),
            Self::Impl(impl_id) => impl_id.record_memory_children(recorder),
        }
    }
}

impl MemorySize for SemanticTypePathResolution {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            Self::SelfType(types) | Self::TypeDefs(types) => types.record_memory_children(recorder),
            Self::Traits(traits) => traits.record_memory_children(recorder),
            Self::Unknown => {}
        }
    }
}
