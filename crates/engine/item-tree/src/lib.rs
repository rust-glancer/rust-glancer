mod discovery;
mod file_tree;
mod item;
mod lower;
mod query;
mod source;

#[doc(hidden)]
pub mod testonly;

#[cfg(test)]
mod tests;

pub use rg_cfg_eval::{CfgExpr, CfgGate, CfgPredicate};
pub use rg_ir_model::Mutability;
pub use rg_text::Name;

pub use self::{
    discovery::{TargetSources, target_sources},
    file_tree::{FileTree, ItemTrees},
    item::{
        BuiltinDeriveKind, BuiltinMacroItem, BuiltinMacroKind, CfgAttrMacroUse, CfgSelectArmItem,
        CfgSelectArmPayload, ConstExpr, ConstExprData, ConstItem, ConstParamData, DeriveAttrs,
        DeriveMacroCall, Documentation, DocumentationPlacement, DocumentationSource, EnumItem,
        EnumVariantItem, ExternBlockItem, ExternCrateItem, FieldItem, FieldKey, FieldList, FromAst,
        FunctionItem, FunctionQualifiers, GenericArg, GenericParams, ImplItem, ImplPolarity,
        ImportAlias, IncludePathExpression, InnerDocs, ItemKind, ItemNode, ItemTag, ItemTreeId,
        ItemTreeRef, LangItem, LifetimeParamData, MacroCallContext, MacroCallItem, MacroDefAst,
        MacroDefContext, MacroDefinitionAttrs, MacroDefinitionItem, MacroRulesAst,
        MacroRulesContext, MacroUseAttr, MacroUseSelector, MaybeFromAst, ModuleItem, ModuleSource,
        OuterDocs, ParamItem, ParamKind, ProcMacroDefinition, ProcMacroKind, SelfParamKind,
        StaticItem, StructItem, TraitBoundModifier, TraitItem, TypeAliasItem, TypeBound,
        TypeBoundListDisplay, TypeNameFormatter, TypeOrConstParamData, TypeParamData, TypePath,
        TypePathAnchor, TypePathSegment, TypeRef, TypeRefDisplay, UnionItem, UseImport,
        UseImportKind, UseItem, UsePath, UsePathSegment, UsePathSegmentKind, UserFacingAttrs,
        VisibilityLevel, WherePredicate,
    },
    query::{FileDeclarations, file_declaration_sources, file_declarations},
    source::{
        DeclarationSourceBuilder, DeclarationSourceStore, NoSource, RecordSource, SourceLocations,
        SourceRole, TypeSource,
    },
};
