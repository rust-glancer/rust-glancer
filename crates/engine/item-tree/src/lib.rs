mod db;
mod item;
mod lower;
mod package;

#[doc(hidden)]
pub mod testonly;

#[cfg(test)]
mod tests;

pub use self::{
    db::{IncrementalItemTreeLowering, ItemTreeDb},
    item::{
        BuiltinMacroItem, BuiltinMacroKind, CfgAttrMacroUse, CfgSelectArmItem, CfgSelectArmPayload,
        ConstExpr, ConstItem, ConstParamData, Documentation, EnumItem, EnumVariantItem,
        ExternBlockItem, ExternCrateItem, FieldItem, FieldKey, FieldList, FromAst, FunctionItem,
        FunctionQualifiers, GenericArg, GenericParams, ImplItem, ImplItemContext, ImportAlias,
        IncludePathExpression, InnerDocs, ItemKind, ItemNode, ItemTag, ItemTreeId, ItemTreeRef,
        LangItem, LifetimeParamData, MacroCallContext, MacroCallItem, MacroDefAst, MacroDefContext,
        MacroDefinitionAttrs, MacroDefinitionItem, MacroRulesAst, MacroRulesContext, MacroUseAttr,
        MacroUseSelector, MaybeFromAst, ModuleItem, ModuleSource, OuterDocs, ParamItem, ParamKind,
        ProcMacroDefinition, ProcMacroKind, SelfParamKind, StaticItem, StructItem,
        TraitBoundModifier, TraitItem, TraitItemContext, TypeAliasItem, TypeBound,
        TypeBoundListDisplay, TypeNameFormatter, TypeOrConstParamData, TypeParamData, TypePath,
        TypePathAnchor, TypePathSegment, TypeRef, TypeRefDisplay, UnionItem, UseImport,
        UseImportKind, UseItem, UsePath, UsePathSegment, UsePathSegmentKind, UserFacingAttrs,
        VisibilityLevel, WherePredicate,
    },
    package::{FileTree, Package, TargetRoot},
};
pub use rg_cfg_eval::{CfgExpr, CfgGate, CfgPredicate};
pub use rg_ir_model::Mutability;
pub use rg_text::{Name, PackageNameInterners};
