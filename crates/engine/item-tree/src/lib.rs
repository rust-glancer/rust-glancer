mod body;
mod db;
mod item;
mod lower;
mod package;

#[doc(hidden)]
pub mod testonly;

#[cfg(test)]
mod tests;

pub use self::{
    body::{RecordExprFieldAst, RecordPatFieldAst},
    db::ItemTreeDb,
    item::{
        BuiltinMacroItem, CfgAttrMacroUse, CfgSelectArmItem, CfgSelectArmPayload, ConstItem,
        Documentation, EnumItem, EnumVariantItem, ExternCrateItem, FieldItem, FieldKey, FieldList,
        FromAst, FunctionItem, FunctionQualifiers, GenericArg, GenericParams, ImplItem,
        ImplItemContext, ImportAlias, InnerDocs, ItemKind, ItemNode, ItemTag, ItemTreeId,
        ItemTreeRef, MacroCallContext, MacroCallItem, MacroDefAst, MacroDefContext,
        MacroDefinitionAttrs, MacroDefinitionItem, MacroRulesAst, MacroRulesContext, MacroUseAttr,
        MacroUseSelector, MaybeFromAst, ModuleItem, ModuleSource, Mutability, OuterDocs, ParamItem,
        ParamKind, StaticItem, StructItem, TraitItem, TraitItemContext, TypeAliasItem, TypeBound,
        TypePath, TypePathSegment, TypeRef, UnionItem, UseImport, UseImportKind, UseItem, UsePath,
        UsePathSegment, UsePathSegmentKind, VisibilityLevel, WherePredicate,
    },
    package::{FileTree, Package, TargetRoot},
};
pub use rg_cfg_eval::{CfgExpr, CfgGate, CfgPredicate};
pub use rg_text::{Name, PackageNameInterners};
