mod db;
mod item;
mod lower;
mod package;

#[doc(hidden)]
pub mod testonly;

#[cfg(test)]
mod tests;

pub use self::{
    db::ItemTreeDb,
    item::{
        BuiltinMacroItem, BuiltinMacroKind, CfgAttrMacroUse, CfgSelectArmItem, CfgSelectArmPayload,
        ConstItem, ConstParamData, Documentation, EnumItem, EnumVariantItem, ExternCrateItem,
        FieldItem, FieldKey, FieldList, FromAst, FunctionItem, FunctionQualifiers, GenericArg,
        GenericParams, ImplItem, ImplItemContext, ImportAlias, InnerDocs, ItemKind, ItemNode,
        ItemTag, ItemTreeId, ItemTreeRef, LangItem, LifetimeParamData, MacroCallContext,
        MacroCallItem, MacroDefAst, MacroDefContext, MacroDefinitionAttrs, MacroDefinitionItem,
        MacroRulesAst, MacroRulesContext, MacroUseAttr, MacroUseSelector, MaybeFromAst, ModuleItem,
        ModuleSource, OuterDocs, ParamItem, ParamKind, SelfParamKind, StaticItem, StructItem,
        TraitItem, TraitItemContext, TypeAliasItem, TypeBound, TypeBoundListDisplay,
        TypeNameFormatter, TypeOrConstParamData, TypeParamData, TypePath, TypePathAnchor,
        TypePathSegment, TypeRef, TypeRefDisplay, UnionItem, UseImport, UseImportKind, UseItem,
        UsePath, UsePathSegment, UsePathSegmentKind, VisibilityLevel, WherePredicate,
    },
    package::{FileTree, Package, TargetRoot},
};
pub use rg_cfg_eval::{CfgExpr, CfgGate, CfgPredicate};
pub use rg_ir_model::Mutability;
pub use rg_text::{Name, PackageNameInterners};
