use rg_memsize::{MemoryRecorder, MemorySize};

use crate::{
    ConstItem, Documentation, EnumItem, EnumVariantItem, ExternCrateItem, FieldItem, FieldKey,
    FieldList, FunctionItem, GenericArg, GenericParams, ImplItem, ImportAlias, ItemKind, ItemNode,
    ItemTag, ItemTreeDb, ItemTreeId, ItemTreeRef, ModuleItem, ModuleSource, Mutability, Package,
    ParamItem, ParamKind, StaticItem, StructItem, TargetRoot, TraitItem, TypeAliasItem, TypeBound,
    TypePath, TypePathSegment, TypeRef, UnionItem, UseImport, UseImportKind, UseItem, UsePath,
    UsePathSegment, UsePathSegmentKind, VisibilityLevel, WherePredicate,
    item::{ConstParamData, FunctionQualifiers, LifetimeParamData, TypeParamData},
};

rg_memsize::impl_memory_size_leaf!(ItemTreeId, ParamKind, Mutability, UseImportKind, ItemTag);

rg_memsize::impl_memory_size_children! {
    ItemTreeDb => packages;
    Package => files, target_roots;
    crate::FileTree => file, docs, top_level, items;
    TargetRoot => target, root_file;
    ItemTreeRef => file_id, item;
    ItemNode => kind, name, name_span, visibility, docs, file_id, span;
    Documentation => text;
    GenericParams => lifetimes, types, consts, where_predicates;
    LifetimeParamData => name, bounds;
    TypeParamData => name, bounds, default;
    ConstParamData => name, ty, default;
    FunctionItem => generics, params, ret_ty, qualifiers;
    FunctionQualifiers => is_async, is_const, is_unsafe;
    ParamItem => pat, ty, kind;
    StructItem => generics, fields;
    UnionItem => generics, fields;
    EnumItem => generics, variants;
    EnumVariantItem => name, span, name_span, docs, fields;
    FieldItem => key, visibility, ty, span, docs;
    TraitItem => generics, super_traits, items, is_unsafe;
    ImplItem => generics, trait_ref, self_ty, items, is_unsafe;
    TypeAliasItem => generics, bounds, aliased_ty;
    ConstItem => generics, ty;
    StaticItem => ty, mutability;
    TypePath => source_span, absolute, segments;
    TypePathSegment => name, args, span;
    ExternCrateItem => name, alias;
    UseItem => imports;
    UseImport => kind, path, alias;
    UsePath => source_span, absolute, segments;
    UsePathSegment => kind, span;
    ModuleItem => inner_docs, source;
}

impl MemorySize for WherePredicate {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            Self::Type { ty, bounds } => {
                recorder.scope("ty", |recorder| ty.record_memory_children(recorder));
                recorder.scope("bounds", |recorder| bounds.record_memory_children(recorder));
            }
            Self::Lifetime { lifetime, bounds } => {
                recorder.scope("lifetime", |recorder| {
                    lifetime.record_memory_children(recorder);
                });
                recorder.scope("bounds", |recorder| bounds.record_memory_children(recorder));
            }
            Self::Unsupported(text) => text.record_memory_children(recorder),
        }
    }
}

impl MemorySize for FieldList {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            Self::Named(fields) | Self::Tuple(fields) => fields.record_memory_children(recorder),
            Self::Unit => {}
        }
    }
}

impl MemorySize for FieldKey {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            Self::Named(name) => name.record_memory_children(recorder),
            Self::Tuple(index) => index.record_memory_children(recorder),
        }
    }
}

impl MemorySize for TypeRef {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            Self::Unknown(text) => text.record_memory_children(recorder),
            Self::Never | Self::Unit | Self::Infer => {}
            Self::Path(path) => path.record_memory_children(recorder),
            Self::Tuple(types) => types.record_memory_children(recorder),
            Self::Reference {
                lifetime,
                mutability,
                inner,
            } => {
                recorder.scope("lifetime", |recorder| {
                    lifetime.record_memory_children(recorder);
                });
                recorder.scope("mutability", |recorder| {
                    mutability.record_memory_children(recorder);
                });
                recorder.scope("inner", |recorder| inner.record_memory_children(recorder));
            }
            Self::RawPointer { mutability, inner } => {
                recorder.scope("mutability", |recorder| {
                    mutability.record_memory_children(recorder);
                });
                recorder.scope("inner", |recorder| inner.record_memory_children(recorder));
            }
            Self::Slice(inner) => inner.record_memory_children(recorder),
            Self::Array { inner, len } => {
                recorder.scope("inner", |recorder| inner.record_memory_children(recorder));
                recorder.scope("len", |recorder| len.record_memory_children(recorder));
            }
            Self::FnPointer { params, ret } => {
                recorder.scope("params", |recorder| params.record_memory_children(recorder));
                recorder.scope("ret", |recorder| ret.record_memory_children(recorder));
            }
            Self::ImplTrait(bounds) | Self::DynTrait(bounds) => {
                bounds.record_memory_children(recorder);
            }
        }
    }
}

impl MemorySize for GenericArg {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            Self::Type(ty) => ty.record_memory_children(recorder),
            Self::Lifetime(lifetime) | Self::Const(lifetime) | Self::Unsupported(lifetime) => {
                lifetime.record_memory_children(recorder);
            }
            Self::AssocType { name, ty } => {
                recorder.scope("name", |recorder| name.record_memory_children(recorder));
                recorder.scope("ty", |recorder| ty.record_memory_children(recorder));
            }
        }
    }
}

impl MemorySize for TypeBound {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            Self::Trait(ty) => ty.record_memory_children(recorder),
            Self::Lifetime(lifetime) | Self::Unsupported(lifetime) => {
                lifetime.record_memory_children(recorder);
            }
        }
    }
}

impl MemorySize for ImportAlias {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            Self::Inferred | Self::Hidden => {}
            Self::Explicit { name, span } => {
                recorder.scope("name", |recorder| name.record_memory_children(recorder));
                recorder.scope("span", |recorder| span.record_memory_children(recorder));
            }
        }
    }
}

impl MemorySize for UsePathSegmentKind {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            Self::Name(name) => name.record_memory_children(recorder),
            Self::SelfKw | Self::SuperKw | Self::CrateKw => {}
        }
    }
}

impl MemorySize for ItemKind {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            Self::AsmExpr | Self::ExternBlock | Self::MacroDefinition => {}
            Self::Const(item) => item.record_memory_children(recorder),
            Self::Enum(item) => item.record_memory_children(recorder),
            Self::ExternCrate(item) => item.record_memory_children(recorder),
            Self::Function(item) => item.record_memory_children(recorder),
            Self::Impl(item) => item.record_memory_children(recorder),
            Self::Module(item) => item.record_memory_children(recorder),
            Self::Static(item) => item.record_memory_children(recorder),
            Self::Struct(item) => item.record_memory_children(recorder),
            Self::Trait(item) => item.record_memory_children(recorder),
            Self::TypeAlias(item) => item.record_memory_children(recorder),
            Self::Union(item) => item.record_memory_children(recorder),
            Self::Use(item) => item.record_memory_children(recorder),
        }
    }
}

impl MemorySize for ModuleSource {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            Self::Inline { items } => items.record_memory_children(recorder),
            Self::OutOfLine { definition_file } => definition_file.record_memory_children(recorder),
        }
    }
}

impl MemorySize for VisibilityLevel {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            Self::Private | Self::Public | Self::Crate | Self::Super | Self::Self_ => {}
            Self::Restricted(path) | Self::Unknown(path) => path.record_memory_children(recorder),
        }
    }
}
