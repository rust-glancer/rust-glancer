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
use rg_ir_model::{
    ConstRef, CrateRef, EnumVariantFieldRef, EnumVariantRef, FieldKey, FieldRef, FunctionRef,
    ItemOwner, TraitApplicability, TypeAliasRef, TypeDefId, TypeDefRef, identity::DeclarationRef,
};
use rg_item_tree::{Documentation, FieldList, ParamItem, ParamKind, UserFacingAttrs};
pub use rg_semantic_ir::AssociatedItemFilter;
use rg_semantic_ir::{EnumVariantData, FieldData, FunctionData, ItemStoreQuery};
use rg_syntax::{Edition, LexedStr, SyntaxKind};
use rg_ty::lookup::{
    AssociatedItemCandidateRef, AssociatedItemRef, MemberMethodCandidateRef,
    MemberMethodOrigin as TyMemberMethodOrigin,
};

use crate::{IndexedViewDb, SymbolKind, item::path::PathView};

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

    pub fn is_suggestible_from(&self, use_crate: CrateRef) -> bool {
        self.data
            .field
            .user_facing_attrs
            .is_suggestible_from(self.field.owner.origin.origin_crate(), use_crate)
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

    pub fn is_suggestible_from(&self, use_crate: CrateRef) -> bool {
        self.data
            .field
            .user_facing_attrs
            .is_suggestible_from(self.field.owner.origin.origin_crate(), use_crate)
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

    pub fn is_suggestible_from(&self, use_crate: CrateRef) -> bool {
        self.data
            .user_facing_attrs
            .is_suggestible_from(self.function.store.origin.origin_crate(), use_crate)
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

    pub fn is_variadic(&self) -> bool {
        self.data
            .signature
            .params()
            .last()
            .is_some_and(|param| param.kind == ParamKind::Variadic)
    }

    fn docs(&self) -> Option<&'a Documentation> {
        self.data.docs.as_ref()
    }
}

/// Borrowed parameter facts needed by editor features.
///
/// Item lowering retains complete patterns and type syntax. Completion and inlay hints can ask
/// this view for the name of a simple binding or whether a parameter is a receiver.
#[derive(Debug, Clone, Copy)]
pub struct FunctionParameterView<'a> {
    param: &'a ParamItem,
}

impl<'a> FunctionParameterView<'a> {
    fn new(param: &'a ParamItem) -> Self {
        Self { param }
    }

    /// Return the written name of a simple parameter binding.
    pub fn simple_binding_name(self) -> Option<&'a str> {
        let mut name = self.param.pat.trim();
        while let Some(binding) = name
            .strip_prefix("mut ")
            .or_else(|| name.strip_prefix("ref "))
        {
            name = binding.trim_start();
        }
        if self.is_receiver() && name == "self" {
            return Some(name);
        }
        // The declaration was already parsed in its own edition. An older lexer edition keeps
        // names such as `gen` valid here even when a newer crate is calling the function.
        matches!(
            LexedStr::single_token(Edition::Edition2015, name),
            Some((SyntaxKind::IDENT, None))
        )
        .then_some(name)
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

    pub fn is_suggestible_from(&self, use_crate: CrateRef) -> bool {
        self.data
            .variant
            .user_facing_attrs
            .is_suggestible_from(self.variant.origin.origin_crate(), use_crate)
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

/// A receiver lookup result together with the method's borrowed declaration.
#[derive(Debug, Clone, Copy)]
pub struct MemberMethodCandidate<'a> {
    candidate: MemberMethodCandidateRef,
    data: &'a FunctionData,
}

impl<'a> MemberMethodCandidate<'a> {
    pub fn function(&self) -> MemberFunction<'a> {
        MemberFunction {
            function: self.candidate.function(),
            data: self.data,
        }
    }

    pub fn origin(&self) -> MemberMethodOrigin {
        self.candidate.origin().into()
    }

    pub fn return_type_matches(&self) -> bool {
        self.candidate.return_type_matches()
    }
}

/// Declaration source for a method candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberMethodOrigin {
    Inherent,
    Trait { applicability: TraitApplicability },
}

impl From<TyMemberMethodOrigin> for MemberMethodOrigin {
    fn from(origin: TyMemberMethodOrigin) -> Self {
        match origin {
            TyMemberMethodOrigin::Inherent => Self::Inherent,
            TyMemberMethodOrigin::Trait { applicability } => Self::Trait { applicability },
        }
    }
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
}

/// One associated declaration and the confidence of the receiver or impl match.
///
/// Different lookup routes can find the same declaration with different trait applicability.
/// Projection keeps one declaration and combines that evidence before analysis ranks or filters
/// the result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MemberAssociatedItemCandidate(AssociatedItemCandidateRef);

impl MemberAssociatedItemCandidate {
    pub fn return_type_matches(self) -> bool {
        self.0.return_type_matches()
    }

    pub fn item(self) -> MemberAssociatedItem {
        match self.0.item() {
            AssociatedItemRef::Function(function) => MemberAssociatedItem::Function(function),
            AssociatedItemRef::TypeAlias(alias) => MemberAssociatedItem::TypeAlias(alias),
            AssociatedItemRef::Const(konst) => MemberAssociatedItem::Const(konst),
            AssociatedItemRef::EnumVariant(variant) => MemberAssociatedItem::EnumVariant(variant),
        }
    }

    pub fn applicability(self) -> TraitApplicability {
        self.0.applicability()
    }
}

/// Presentation facts for one associated declaration after ref-level lookup succeeds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberAssociatedItemDefinition {
    candidate: MemberAssociatedItemCandidate,
    label: String,
    kind: SymbolKind,
    documentation: Option<String>,
    user_facing_attrs: UserFacingAttrs,
}

impl MemberAssociatedItemDefinition {
    pub fn is_suggestible_from(&self, use_crate: CrateRef) -> bool {
        let origin = self.candidate.item().declaration_ref().origin();
        self.user_facing_attrs
            .is_suggestible_from(origin.origin_crate(), use_crate)
    }

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

/// Projects member refs into field, function, and method view data.
pub struct MemberView<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> MemberView<'a, 'db> {
    pub fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self { db }
    }

    /// Return borrowed data for one function.
    #[rg_std::cancelable("member function projection", token = self.db)]
    pub fn function(&self, function: FunctionRef) -> anyhow::Result<Option<MemberFunction<'_>>> {
        Ok(ItemStoreQuery::new(self.db)
            .function_data(function)
            .context("read member function data")?
            .map(|data| MemberFunction { function, data }))
    }

    /// Return borrowed data for one enum variant.
    #[rg_std::cancelable("enum variant projection", token = self.db)]
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

    /// Return the variants of an indexed enum.
    pub fn enum_variant_candidates(
        &self,
        owner: TypeDefRef,
    ) -> anyhow::Result<Vec<EnumVariantRef>> {
        let TypeDefId::Enum(enum_id) = owner.id else {
            return Ok(Vec::new());
        };
        let Some(data) = ItemStoreQuery::new(self.db)
            .enum_data_for_type_def(owner)
            .context("read inferred enum candidate data")?
        else {
            return Ok(Vec::new());
        };
        Ok((0..data.variants.len())
            .map(|index| EnumVariantRef {
                origin: owner.origin,
                enum_id,
                index,
            })
            .collect())
    }
}
