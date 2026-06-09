use rg_cfg_eval::CfgPredicate;
use rg_std::{MemorySize, Shrink};
use rg_text::Name;
use rg_tt::TopSubtree;
use wincode::{SchemaRead, SchemaWrite};

mod builtin;

pub use self::builtin::{BuiltinMacroItem, CfgSelectArmItem, CfgSelectArmPayload};

#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub enum MacroDefinitionItem {
    MacroRules {
        attrs: MacroDefinitionAttrs,
        #[shrink(skip)]
        body: Option<TopSubtree>,
    },
    MacroDef {
        #[shrink(skip)]
        args: Option<TopSubtree>,
        #[shrink(skip)]
        body: Option<TopSubtree>,
    },
}

impl MacroDefinitionItem {
    pub fn shrink_to_fit(&mut self) {
        Shrink::shrink_to_fit(self);
    }
}

/// Macro-specific attributes that affect def-map visibility.
#[derive(Debug, Clone, Default, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct MacroDefinitionAttrs {
    #[memsize(skip)]
    pub macro_export: bool,
    pub cfg_attr_macro_export: Vec<CfgPredicate>,
}

impl MacroDefinitionAttrs {
    pub fn shrink_to_fit(&mut self) {
        Shrink::shrink_to_fit(self);
    }
}

/// Legacy `#[macro_use]` import selector.
#[derive(Debug, Clone, Default, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct MacroUseAttr {
    pub direct: Option<MacroUseSelector>,
    pub cfg_attr_macro_use: Vec<CfgAttrMacroUse>,
}

impl MacroUseAttr {
    pub fn shrink_to_fit(&mut self) {
        Shrink::shrink_to_fit(self);
    }
}

/// Macro-use selector once cfg_attr gates have been evaluated for one target.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct MacroUseSelector {
    /// `None` means all exported macros; `Some` keeps the explicit `#[macro_use(foo, bar)]` list.
    pub names: Option<Vec<Name>>,
}

impl MacroUseSelector {
    pub fn allows(&self, name: &Name) -> bool {
        match &self.names {
            Some(names) => names.iter().any(|allowed| allowed == name),
            None => true,
        }
    }

    pub fn merge(&mut self, other: &Self) {
        let (Some(names), Some(other_names)) = (&mut self.names, &other.names) else {
            self.names = None;
            return;
        };

        for name in other_names {
            if !names.iter().any(|existing| existing == name) {
                names.push(name.clone());
            }
        }
    }

    pub fn shrink_to_fit(&mut self) {
        Shrink::shrink_to_fit(self);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct CfgAttrMacroUse {
    pub predicate: CfgPredicate,
    pub selector: MacroUseSelector,
}

impl CfgAttrMacroUse {
    pub fn shrink_to_fit(&mut self) {
        Shrink::shrink_to_fit(self);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct MacroCallItem {
    pub path: Option<String>,
    pub callee: Option<Name>,
    #[shrink(skip)]
    pub args: Option<TopSubtree>,
    pub builtin: Option<BuiltinMacroItem>,
}

impl MacroCallItem {
    pub fn shrink_to_fit(&mut self) {
        Shrink::shrink_to_fit(self);
    }
}
