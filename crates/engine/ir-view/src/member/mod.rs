//! Fields, methods, constructors, and associated declarations projected for editor queries.
//!
//! The type layer returns stable refs and applicability evidence. Editor features also need
//! borrowed item data, documentation, constructor shape, and the candidate surface behind
//! `value.field`, `Widget::item`, `<T as Trait>::item`, and `Enum::Variant`. This view performs
//! that cross-layer join, includes body-local impl overlays where needed, and collapses duplicate
//! associated identities before they reach analysis.

mod associated;
mod field;
mod method;

use anyhow::Context as _;
use rg_ir_model::Path;
use rg_ir_model::{
    BodyRef, ConstRef, CrateRef, EnumVariantFieldRef, EnumVariantRef, FieldKey, FieldRef,
    FunctionRef, ItemOwner, ScopeId, TraitApplicability, TypeAliasRef, TypeDefId,
    identity::DeclarationRef,
};
use rg_item_tree::{Documentation, FieldList, ParamItem, ParamKind};
use rg_semantic_ir::{
    EnumVariantData, FieldData, FunctionData, ItemStoreQuery, TypePathResolution,
};
use rg_ty::MemberMethodOrigin as TyMemberMethodOrigin;

use crate::{
    IndexedViewDb, SymbolKind, body::BodyResolutionView, item::path::PathView, ty::IndexedType,
};

/// Borrowed data for one resolved field, independent from the storage layer it came from.
#[derive(Debug, Clone, Copy)]
pub struct MemberField<'a> {
    field: FieldRef,
    data: FieldData<'a>,
}

impl<'a> MemberField<'a> {
    pub fn field_ref(&self) -> FieldRef {
        self.field
    }

    pub fn key(&self) -> Option<&'a FieldKey> {
        self.data.field.key.as_ref()
    }

    pub(crate) fn data(&self) -> FieldData<'a> {
        self.data
    }

    pub fn display_path(&self, paths: &PathView<'_, '_>) -> anyhow::Result<Option<String>> {
        paths.type_def_path(self.field.owner)
    }

    pub fn docs_text(&self) -> Option<String> {
        self.docs().map(Documentation::text)
    }

    fn docs(&self) -> Option<&'a Documentation> {
        self.data.field.docs.as_ref()
    }
}

/// Borrowed data for one field declared directly by an enum variant.
#[derive(Debug, Clone, Copy)]
pub struct MemberEnumVariantField<'a> {
    field: EnumVariantFieldRef,
    data: FieldData<'a>,
}

impl<'a> MemberEnumVariantField<'a> {
    pub fn field_ref(&self) -> EnumVariantFieldRef {
        self.field
    }

    pub fn key(&self) -> Option<&'a FieldKey> {
        self.data.field.key.as_ref()
    }

    pub(crate) fn data(&self) -> FieldData<'a> {
        self.data
    }

    pub fn docs_text(&self) -> Option<String> {
        self.data.field.docs.as_ref().map(Documentation::text)
    }
}

/// Source-independent constructor shape used by pattern completion.
///
/// ```text
/// None               -> Unit
/// Some(value)        -> Tuple { field_count: 1 }
/// User { name, age } -> Record { field_names: ["name", "age"] }
/// ```
///
/// Keeping only this shape lets completion choose `Name`, `Name(...)`, or `Name { ... }` without
/// inspecting item-tree field storage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConstructorShape {
    Unit,
    Tuple { field_count: usize },
    Record { field_names: Vec<String> },
}

impl ConstructorShape {
    fn from_fields(fields: &FieldList) -> Self {
        match fields {
            FieldList::Unit => Self::Unit,
            FieldList::Tuple(fields) => Self::Tuple {
                field_count: fields.len(),
            },
            FieldList::Named(fields) => Self::Record {
                field_names: fields
                    .iter()
                    .filter_map(|field| field.key.as_ref())
                    .map(FieldKey::declaration_label)
                    .collect(),
            },
        }
    }
}

/// Borrowed data for one resolved function, independent from the storage layer it came from.
#[derive(Debug, Clone, Copy)]
pub struct MemberFunction<'a> {
    function: FunctionRef,
    data: &'a FunctionData,
}

impl<'a> MemberFunction<'a> {
    pub fn function_ref(&self) -> FunctionRef {
        self.function
    }

    pub fn name(&self) -> &'a str {
        self.data.name.as_str()
    }

    /// Iterate parameters without exposing the item-tree storage shape.
    pub fn parameters(&self) -> impl ExactSizeIterator<Item = FunctionParameterView<'a>> + 'a {
        self.data
            .signature
            .params()
            .iter()
            .map(FunctionParameterView::new)
    }

    pub fn parameter(&self, index: usize) -> Option<FunctionParameterView<'a>> {
        self.data
            .signature
            .params()
            .get(index)
            .map(FunctionParameterView::new)
    }

    pub(crate) fn data(&self) -> &'a FunctionData {
        self.data
    }

    pub fn display_path(&self, paths: &PathView<'_, '_>) -> anyhow::Result<Option<String>> {
        paths.function_path(self.function)
    }

    pub fn symbol_kind(&self) -> SymbolKind {
        match self.data.owner {
            ItemOwner::Module(_) => SymbolKind::Function,
            ItemOwner::Trait(_) | ItemOwner::Impl(_) => SymbolKind::Method,
        }
    }

    pub fn docs_text(&self) -> Option<String> {
        self.docs().map(Documentation::text)
    }

    pub fn has_self_receiver(&self) -> bool {
        self.data.has_self_receiver()
    }

    fn docs(&self) -> Option<&'a Documentation> {
        self.data.docs.as_ref()
    }
}

/// Borrowed parameter facts needed by editor features.
///
/// Item lowering retains complete patterns and type syntax. Completion and inlay hints only need
/// the written pattern plus whether the parameter is a receiver, so the full item-tree node stays
/// behind this projection.
#[derive(Debug, Clone, Copy)]
pub struct FunctionParameterView<'a> {
    param: &'a ParamItem,
}

impl<'a> FunctionParameterView<'a> {
    fn new(param: &'a ParamItem) -> Self {
        Self { param }
    }

    pub fn pattern(self) -> &'a str {
        self.param.pat.as_str()
    }

    pub fn is_receiver(self) -> bool {
        matches!(self.param.kind, ParamKind::SelfParam(_))
    }
}

/// Borrowed data for one resolved enum variant constructor.
#[derive(Debug, Clone, Copy)]
pub struct MemberEnumVariant<'a> {
    variant: EnumVariantRef,
    data: EnumVariantData<'a>,
    owner_name: &'a str,
}

impl<'a> MemberEnumVariant<'a> {
    pub fn variant_ref(&self) -> EnumVariantRef {
        self.variant
    }

    pub fn owner(&self) -> rg_ir_model::TypeDefRef {
        self.data.owner
    }

    pub fn label(&self) -> &'a str {
        self.data.variant.name.as_str()
    }

    pub fn owner_name(&self) -> &'a str {
        self.owner_name
    }

    pub fn constructor_shape(&self) -> ConstructorShape {
        ConstructorShape::from_fields(&self.data.variant.fields)
    }

    pub fn docs_text(&self) -> Option<String> {
        self.data.variant.docs.as_ref().map(Documentation::text)
    }
}

/// One method candidate with enough origin information for UI ranking and labels.
#[derive(Debug, Clone, Copy)]
pub struct MemberMethodCandidate<'a> {
    function: MemberFunction<'a>,
    origin: MemberMethodOrigin,
}

/// Declaration source for a method candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberMethodOrigin {
    Inherent,
    Trait { applicability: TraitApplicability },
}

/// Stable identity for a declaration that may be offered after `::`.
///
/// Functions, associated types, and associated consts come from impl or trait lookup. Enum
/// variants use the same completion surface (`Action::Sto$0`), so they join that vocabulary even
/// though they are not trait associated items.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MemberAssociatedItem {
    Function(FunctionRef),
    TypeAlias(TypeAliasRef),
    Const(ConstRef),
    EnumVariant(EnumVariantRef),
}

impl MemberAssociatedItem {
    pub fn declaration_ref(self) -> DeclarationRef {
        match self {
            Self::Function(function) => DeclarationRef::from(function),
            Self::TypeAlias(alias) => DeclarationRef::from(alias),
            Self::Const(konst) => DeclarationRef::from(konst),
            Self::EnumVariant(variant) => DeclarationRef::from(variant),
        }
    }

    pub fn function_ref(self) -> Option<FunctionRef> {
        match self {
            Self::Function(function) => Some(function),
            Self::TypeAlias(_) | Self::Const(_) | Self::EnumVariant(_) => None,
        }
    }

    pub fn enum_variant_ref(self) -> Option<EnumVariantRef> {
        match self {
            Self::EnumVariant(variant) => Some(variant),
            Self::Function(_) | Self::TypeAlias(_) | Self::Const(_) => None,
        }
    }
}

/// One associated declaration and the confidence of the receiver or impl match.
///
/// Different lookup routes can find the same declaration with different trait applicability.
/// Projection keeps one declaration and combines that evidence before analysis ranks or filters
/// the result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MemberAssociatedItemCandidate {
    item: MemberAssociatedItem,
    applicability: TraitApplicability,
}

impl MemberAssociatedItemCandidate {
    pub fn item(self) -> MemberAssociatedItem {
        self.item
    }

    pub fn applicability(self) -> TraitApplicability {
        self.applicability
    }
}

/// Presentation facts for one associated declaration after ref-level lookup succeeds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberAssociatedItemDefinition {
    candidate: MemberAssociatedItemCandidate,
    label: String,
    kind: SymbolKind,
    documentation: Option<String>,
}

impl MemberAssociatedItemDefinition {
    pub fn candidate(&self) -> MemberAssociatedItemCandidate {
        self.candidate
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn kind(&self) -> SymbolKind {
        self.kind
    }

    pub fn documentation(&self) -> Option<&str> {
        self.documentation.as_deref()
    }
}

impl From<TyMemberMethodOrigin> for MemberMethodOrigin {
    fn from(origin: TyMemberMethodOrigin) -> Self {
        match origin {
            TyMemberMethodOrigin::Inherent => Self::Inherent,
            TyMemberMethodOrigin::Trait { applicability } => Self::Trait { applicability },
        }
    }
}

impl<'a> MemberMethodCandidate<'a> {
    pub fn function(&self) -> MemberFunction<'a> {
        self.function
    }

    pub fn origin(&self) -> MemberMethodOrigin {
        self.origin
    }
}

/// Place where member lookup is requested.
#[derive(Debug, Clone, Copy)]
pub enum MemberUseSite {
    Crate(CrateRef),
    Body(BodyRef),
}

impl MemberUseSite {
    pub fn krate(crate_ref: CrateRef) -> Self {
        Self::Crate(crate_ref)
    }

    pub fn body(body: BodyRef) -> Self {
        Self::Body(body)
    }
}

/// Projects member refs into field, function, and method view data.
pub struct MemberView<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> MemberView<'a, 'db> {
    pub fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self { db }
    }

    /// Return borrowed data for one function.
    pub fn function(&self, function: FunctionRef) -> anyhow::Result<Option<MemberFunction<'_>>> {
        Ok(ItemStoreQuery::new(self.db)
            .function_data(function)
            .context("read member function data")?
            .map(|data| MemberFunction { function, data }))
    }

    /// Return borrowed data for one enum variant.
    pub fn enum_variant(
        &self,
        variant: EnumVariantRef,
    ) -> anyhow::Result<Option<MemberEnumVariant<'_>>> {
        let items = ItemStoreQuery::new(self.db);
        let Some(data) = items
            .enum_variant_data(variant)
            .context("read member enum variant data")?
        else {
            return Ok(None);
        };
        let Some(owner_name) = items
            .type_def_name(data.owner)
            .context("read member enum variant owner name")?
        else {
            return Ok(None);
        };
        Ok(Some(MemberEnumVariant {
            variant,
            data,
            owner_name,
        }))
    }

    /// Return variants of an enum represented by an already-inferred type.
    pub fn enum_variant_candidates_for_ty(
        &self,
        ty: &IndexedType,
    ) -> anyhow::Result<Vec<EnumVariantRef>> {
        let mut variants = Vec::new();
        for owner in ty.nominal_type_defs() {
            let TypeDefId::Enum(enum_id) = owner.id else {
                continue;
            };
            let Some(data) = ItemStoreQuery::new(self.db)
                .enum_data_for_type_def(owner)
                .context("read inferred enum candidate data")?
            else {
                continue;
            };
            variants.extend((0..data.variants.len()).map(|index| EnumVariantRef {
                origin: owner.origin,
                enum_id,
                index,
            }));
        }
        Ok(variants)
    }

    /// Resolve a body type path and return its enum variants.
    pub fn enum_variant_candidates_for_body_type_path(
        &self,
        body: BodyRef,
        scope: ScopeId,
        path: &Path,
    ) -> anyhow::Result<Vec<EnumVariantRef>> {
        let Some(resolution) = BodyResolutionView::new(self.db)
            .type_path_resolution(body, scope, path)
            .context("resolve enum candidate body type path")?
        else {
            return Ok(Vec::new());
        };
        let (TypePathResolution::SelfType(ty) | TypePathResolution::TypeDef(ty)) = resolution
        else {
            return Ok(Vec::new());
        };

        let item_query = ItemStoreQuery::new(self.db);
        let mut variants = Vec::new();
        let TypeDefId::Enum(enum_id) = ty.id else {
            return Ok(Vec::new());
        };
        let Some(data) = item_query
            .enum_data_for_type_def(ty)
            .context("read body enum candidate data")?
        else {
            return Ok(Vec::new());
        };
        variants.extend((0..data.variants.len()).map(|index| EnumVariantRef {
            origin: ty.origin,
            enum_id,
            index,
        }));
        Ok(variants)
    }
}
