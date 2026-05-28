//! Layer-neutral identities composed from the indexed IR storage refs.
//!
//! These refs are the vocabulary views and analysis should exchange. They can point at semantic,
//! body-local, or def-map declarations without making callers invent parallel code paths for each
//! storage layer.

use std::fmt;

use crate::{
    BodyBindingRef, BodyDeclarationRef, BodyEnumVariantRef, BodyFieldRef, BodyFunctionRef,
    BodyImplRef, BodyItemRef, BodyRef as BodyIrBodyRef, BodyValueItemRef, DefId,
    EnumVariantRef as SemanticEnumVariantRef, ExprId, FieldRef as SemanticFieldRef,
    FunctionRef as SemanticFunctionRef, ImplRef as SemanticImplRef, LocalDefRef, ModuleRef,
    ScopeId, SemanticDeclarationRef, SemanticItemRef, TargetRef,
};

/// Stable identity for one lowered function body.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct FunctionBodyRef(BodyIrBodyRef);

impl FunctionBodyRef {
    pub fn body_ir(self) -> BodyIrBodyRef {
        self.0
    }

    pub fn from_body_ir(body: BodyIrBodyRef) -> Self {
        Self(body)
    }

    pub fn target(self) -> TargetRef {
        self.0.target
    }
}

impl fmt::Debug for FunctionBodyRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FunctionBodyRef")
            .field("target", &self.0.target)
            .field("body", &self.0.body)
            .finish()
    }
}

/// Stable identity for one expression inside a lowered body.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ExprRef {
    body: BodyIrBodyRef,
    expr: ExprId,
}

impl ExprRef {
    pub fn new(body: BodyIrBodyRef, expr: ExprId) -> Self {
        Self { body, expr }
    }

    pub fn body_ir(self) -> BodyIrBodyRef {
        self.body
    }

    pub fn expr_id(self) -> ExprId {
        self.expr
    }
}

impl fmt::Debug for ExprRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExprRef")
            .field("body", &FunctionBodyRef::from_body_ir(self.body))
            .field("expr", &self.expr)
            .finish()
    }
}

/// Stable identity for one lexical scope inside a lowered body.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct LexicalScopeRef {
    body: BodyIrBodyRef,
    scope: ScopeId,
}

impl LexicalScopeRef {
    pub fn new(body: BodyIrBodyRef, scope: ScopeId) -> Self {
        Self { body, scope }
    }

    pub fn body_ir(self) -> BodyIrBodyRef {
        self.body
    }

    pub fn scope_id(self) -> ScopeId {
        self.scope
    }
}

impl fmt::Debug for LexicalScopeRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LexicalScopeRef")
            .field("body", &FunctionBodyRef::from_body_ir(self.body))
            .field("scope", &self.scope)
            .finish()
    }
}

/// Stable declaration identity exposed by indexed-data views.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct DeclarationRef(DeclarationRefRepr);

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeclarationRefRepr {
    Module(ModuleRef),
    NameDef(NameDefRef),
    Item(ItemRef),
    Function(FunctionRef),
    Field(FieldRef),
    EnumVariant(EnumVariantRef),
    Binding(BindingRef),
    Impl(ImplRef),
}

impl DeclarationRef {
    pub fn module(module: ModuleRef) -> Self {
        Self(DeclarationRefRepr::Module(module))
    }

    pub fn name_def(name_def: NameDefRef) -> Self {
        Self(DeclarationRefRepr::NameDef(name_def))
    }

    pub fn item(item: ItemRef) -> Self {
        Self(DeclarationRefRepr::Item(item))
    }

    pub fn function(function: FunctionRef) -> Self {
        Self(DeclarationRefRepr::Function(function))
    }

    pub fn field(field: FieldRef) -> Self {
        Self(DeclarationRefRepr::Field(field))
    }

    pub fn enum_variant(variant: EnumVariantRef) -> Self {
        Self(DeclarationRefRepr::EnumVariant(variant))
    }

    pub fn binding(binding: BindingRef) -> Self {
        Self(DeclarationRefRepr::Binding(binding))
    }

    pub fn impl_ref(impl_ref: ImplRef) -> Self {
        Self(DeclarationRefRepr::Impl(impl_ref))
    }

    pub fn semantic(declaration: SemanticDeclarationRef) -> Self {
        match declaration {
            SemanticDeclarationRef::Item(item) => Self::semantic_item(item),
            SemanticDeclarationRef::Field(field) => Self::field(FieldRef::semantic(field)),
            SemanticDeclarationRef::EnumVariant(variant) => {
                Self::enum_variant(EnumVariantRef::semantic(variant))
            }
        }
    }

    pub fn semantic_item(item: SemanticItemRef) -> Self {
        match item {
            SemanticItemRef::Function(function) => Self::function(FunctionRef::semantic(function)),
            SemanticItemRef::Impl(impl_ref) => Self::impl_ref(ImplRef::semantic(impl_ref)),
            SemanticItemRef::TypeDef(_)
            | SemanticItemRef::Trait(_)
            | SemanticItemRef::TypeAlias(_)
            | SemanticItemRef::Const(_)
            | SemanticItemRef::Static(_) => Self::item(ItemRef::semantic(item)),
        }
    }

    pub fn body(declaration: BodyDeclarationRef) -> Self {
        match declaration {
            BodyDeclarationRef::Binding(binding) => Self::binding(BindingRef::body_local(binding)),
            BodyDeclarationRef::Item(item) => Self::body_item(item),
            BodyDeclarationRef::ValueItem(item) => Self::body_value_item(item),
            BodyDeclarationRef::Impl(impl_ref) => Self::impl_ref(ImplRef::body_local(impl_ref)),
            BodyDeclarationRef::Field(field) => Self::field(FieldRef::body_local(field)),
            BodyDeclarationRef::EnumVariant(variant) => {
                Self::enum_variant(EnumVariantRef::body_local(variant))
            }
            BodyDeclarationRef::Function(function) => {
                Self::function(FunctionRef::body_local(function))
            }
        }
    }

    pub fn from_def(def: DefId) -> Self {
        match def {
            DefId::Module(module) => Self::module(module),
            DefId::Local(local_def) => Self::name_def(NameDefRef::def_map_local(local_def)),
        }
    }

    pub fn body_binding(binding: BodyBindingRef) -> Self {
        Self::binding(BindingRef::body_local(binding))
    }

    pub fn body_item(item: BodyItemRef) -> Self {
        Self::item(ItemRef::body_item(item))
    }

    pub fn body_value_item(item: BodyValueItemRef) -> Self {
        Self::item(ItemRef::body_value_item(item))
    }

    pub fn body_field(field: BodyFieldRef) -> Self {
        Self::field(FieldRef::body_local(field))
    }

    pub fn body_enum_variant(variant: BodyEnumVariantRef) -> Self {
        Self::enum_variant(EnumVariantRef::body_local(variant))
    }

    pub fn body_function(function: BodyFunctionRef) -> Self {
        Self::function(FunctionRef::body_local(function))
    }

    pub fn repr(self) -> DeclarationRefRepr {
        self.0
    }
}

impl fmt::Debug for DeclarationRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            DeclarationRefRepr::Module(module) => f
                .debug_struct("DeclarationRef")
                .field("kind", &"module")
                .field("module", &module)
                .finish(),
            DeclarationRefRepr::NameDef(name_def) => f
                .debug_struct("DeclarationRef")
                .field("kind", &"name_def")
                .field("name_def", &name_def)
                .finish(),
            DeclarationRefRepr::Item(item) => f
                .debug_struct("DeclarationRef")
                .field("kind", &"item")
                .field("item", &item)
                .finish(),
            DeclarationRefRepr::Function(function) => f
                .debug_struct("DeclarationRef")
                .field("kind", &"function")
                .field("function", &function)
                .finish(),
            DeclarationRefRepr::Field(field) => f
                .debug_struct("DeclarationRef")
                .field("kind", &"field")
                .field("field", &field)
                .finish(),
            DeclarationRefRepr::EnumVariant(variant) => f
                .debug_struct("DeclarationRef")
                .field("kind", &"enum_variant")
                .field("variant", &variant)
                .finish(),
            DeclarationRefRepr::Binding(binding) => f
                .debug_struct("DeclarationRef")
                .field("kind", &"binding")
                .field("binding", &binding)
                .finish(),
            DeclarationRefRepr::Impl(impl_ref) => f
                .debug_struct("DeclarationRef")
                .field("kind", &"impl")
                .field("impl_ref", &impl_ref)
                .finish(),
        }
    }
}

/// Stable identity for a namespace definition that has not been promoted into an item model.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct NameDefRef(NameDefRefRepr);

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum NameDefRefRepr {
    DefMapLocal(LocalDefRef),
}

impl NameDefRef {
    pub fn def_map_local(local_def: LocalDefRef) -> Self {
        Self(NameDefRefRepr::DefMapLocal(local_def))
    }

    pub fn repr(self) -> NameDefRefRepr {
        self.0
    }
}

impl fmt::Debug for NameDefRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            NameDefRefRepr::DefMapLocal(local_def) => f
                .debug_struct("NameDefRef")
                .field("kind", &"def_map")
                .field("local_def", &local_def)
                .finish(),
        }
    }
}

/// Stable item identity exposed by indexed-data views.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ItemRef(ItemRefRepr);

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum ItemRefRepr {
    Semantic(SemanticItemRef),
    BodyLocal(BodyItemRef),
    BodyLocalValue(BodyValueItemRef),
}

impl ItemRef {
    pub fn semantic(item: SemanticItemRef) -> Self {
        Self(ItemRefRepr::Semantic(item))
    }

    pub fn body_item(item: BodyItemRef) -> Self {
        Self(ItemRefRepr::BodyLocal(item))
    }

    pub fn body_value_item(item: BodyValueItemRef) -> Self {
        Self(ItemRefRepr::BodyLocalValue(item))
    }

    pub fn repr(self) -> ItemRefRepr {
        self.0
    }
}

impl fmt::Debug for ItemRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            ItemRefRepr::Semantic(item) => f
                .debug_struct("ItemRef")
                .field("kind", &"signature")
                .field("item", &item)
                .finish(),
            ItemRefRepr::BodyLocal(item) => f
                .debug_struct("ItemRef")
                .field("kind", &"body_local")
                .field("item", &item)
                .finish(),
            ItemRefRepr::BodyLocalValue(item) => f
                .debug_struct("ItemRef")
                .field("kind", &"body_local_value")
                .field("item", &item)
                .finish(),
        }
    }
}

/// Stable binding identity exposed by indexed-data views.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct BindingRef(BodyBindingRef);

impl BindingRef {
    pub fn body_local(binding: BodyBindingRef) -> Self {
        Self(binding)
    }

    pub fn body_ir(self) -> BodyBindingRef {
        self.0
    }
}

impl fmt::Debug for BindingRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BindingRef")
            .field("body", &FunctionBodyRef::from_body_ir(self.0.body))
            .field("binding", &self.0.binding)
            .finish()
    }
}

/// Stable impl-block identity exposed by indexed-data views.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ImplRef(ImplRefRepr);

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImplRefRepr {
    Semantic(SemanticImplRef),
    BodyLocal(BodyImplRef),
}

impl ImplRef {
    pub fn semantic(impl_ref: SemanticImplRef) -> Self {
        Self(ImplRefRepr::Semantic(impl_ref))
    }

    pub fn body_local(impl_ref: BodyImplRef) -> Self {
        Self(ImplRefRepr::BodyLocal(impl_ref))
    }

    pub fn repr(self) -> ImplRefRepr {
        self.0
    }
}

impl fmt::Debug for ImplRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            ImplRefRepr::Semantic(impl_ref) => f
                .debug_struct("ImplRef")
                .field("target", &impl_ref.target)
                .field("impl", &impl_ref.id)
                .finish(),
            ImplRefRepr::BodyLocal(impl_ref) => f
                .debug_struct("ImplRef")
                .field("body", &FunctionBodyRef::from_body_ir(impl_ref.body))
                .field("impl", &impl_ref.impl_id)
                .finish(),
        }
    }
}

/// Stable field identity exposed by indexed-data views.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct FieldRef(FieldRefRepr);

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum FieldRefRepr {
    Semantic(SemanticFieldRef),
    BodyLocal(BodyFieldRef),
}

impl FieldRef {
    pub fn semantic(field: SemanticFieldRef) -> Self {
        Self(FieldRefRepr::Semantic(field))
    }

    pub fn body_local(field: BodyFieldRef) -> Self {
        Self(FieldRefRepr::BodyLocal(field))
    }

    pub fn repr(self) -> FieldRefRepr {
        self.0
    }
}

impl fmt::Debug for FieldRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            FieldRefRepr::Semantic(field) => f
                .debug_struct("FieldRef")
                .field("owner", &field.owner)
                .field("index", &field.index)
                .finish(),
            FieldRefRepr::BodyLocal(field) => f
                .debug_struct("FieldRef")
                .field("owner", &field.item)
                .field("index", &field.index)
                .finish(),
        }
    }
}

/// Stable function identity exposed by indexed-data views.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct FunctionRef(FunctionRefRepr);

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum FunctionRefRepr {
    Semantic(SemanticFunctionRef),
    BodyLocal(BodyFunctionRef),
}

impl FunctionRef {
    pub fn semantic(function: SemanticFunctionRef) -> Self {
        Self(FunctionRefRepr::Semantic(function))
    }

    pub fn body_local(function: BodyFunctionRef) -> Self {
        Self(FunctionRefRepr::BodyLocal(function))
    }

    pub fn repr(self) -> FunctionRefRepr {
        self.0
    }
}

impl fmt::Debug for FunctionRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            FunctionRefRepr::Semantic(function) => f
                .debug_struct("FunctionRef")
                .field("target", &function.target)
                .field("function", &function.id)
                .finish(),
            FunctionRefRepr::BodyLocal(function) => f
                .debug_struct("FunctionRef")
                .field("body", &FunctionBodyRef::from_body_ir(function.body))
                .field("function", &function.function)
                .finish(),
        }
    }
}

/// Stable enum variant identity exposed by indexed-data views.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct EnumVariantRef(EnumVariantRefRepr);

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum EnumVariantRefRepr {
    Semantic(SemanticEnumVariantRef),
    BodyLocal(BodyEnumVariantRef),
}

impl EnumVariantRef {
    pub fn semantic(variant: SemanticEnumVariantRef) -> Self {
        Self(EnumVariantRefRepr::Semantic(variant))
    }

    pub fn body_local(variant: BodyEnumVariantRef) -> Self {
        Self(EnumVariantRefRepr::BodyLocal(variant))
    }

    pub fn repr(self) -> EnumVariantRefRepr {
        self.0
    }
}

impl fmt::Debug for EnumVariantRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            EnumVariantRefRepr::Semantic(variant) => f
                .debug_struct("EnumVariantRef")
                .field("target", &variant.target)
                .field("enum", &variant.enum_id)
                .field("index", &variant.index)
                .finish(),
            EnumVariantRefRepr::BodyLocal(variant) => f
                .debug_struct("EnumVariantRef")
                .field("owner", &variant.item)
                .field("index", &variant.index)
                .finish(),
        }
    }
}
