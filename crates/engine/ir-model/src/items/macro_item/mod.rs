use rg_cfg_eval::CfgPredicate;
use rg_text::Name;
use rg_tt::TopSubtree;

mod builtin;

pub use self::builtin::{BuiltinMacroItem, CfgSelectArmItem, CfgSelectArmPayload};

#[derive(
    Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite, rg_memsize::MemorySize,
)]
pub enum MacroDefinitionItem {
    MacroRules {
        attrs: MacroDefinitionAttrs,
        body: Option<TopSubtree>,
    },
    MacroDef {
        args: Option<TopSubtree>,
        body: Option<TopSubtree>,
    },
}

impl MacroDefinitionItem {
    pub fn shrink_to_fit(&mut self) {
        match self {
            Self::MacroRules { attrs, .. } => attrs.shrink_to_fit(),
            Self::MacroDef { .. } => {}
        }
    }
}

/// Macro-specific attributes that affect def-map visibility.
#[derive(
    Debug,
    Clone,
    Default,
    PartialEq,
    Eq,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
pub struct MacroDefinitionAttrs {
    #[memsize(skip)]
    pub macro_export: bool,
    pub cfg_attr_macro_export: Vec<CfgPredicate>,
}

impl MacroDefinitionAttrs {
    pub fn shrink_to_fit(&mut self) {
        self.cfg_attr_macro_export.shrink_to_fit();
        for predicate in &mut self.cfg_attr_macro_export {
            predicate.shrink_to_fit();
        }
    }
}

/// Legacy `#[macro_use]` import selector.
#[derive(
    Debug,
    Clone,
    Default,
    PartialEq,
    Eq,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
pub struct MacroUseAttr {
    pub direct: Option<MacroUseSelector>,
    pub cfg_attr_macro_use: Vec<CfgAttrMacroUse>,
}

impl MacroUseAttr {
    pub fn shrink_to_fit(&mut self) {
        if let Some(direct) = &mut self.direct {
            direct.shrink_to_fit();
        }
        self.cfg_attr_macro_use.shrink_to_fit();
        for cfg_attr in &mut self.cfg_attr_macro_use {
            cfg_attr.shrink_to_fit();
        }
    }
}

/// Macro-use selector once cfg_attr gates have been evaluated for one target.
#[derive(
    Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite, rg_memsize::MemorySize,
)]
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
        if let Some(names) = &mut self.names {
            names.shrink_to_fit();
            for name in names {
                name.shrink_to_fit();
            }
        }
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite, rg_memsize::MemorySize,
)]
pub struct CfgAttrMacroUse {
    pub predicate: CfgPredicate,
    pub selector: MacroUseSelector,
}

impl CfgAttrMacroUse {
    pub fn shrink_to_fit(&mut self) {
        self.predicate.shrink_to_fit();
        self.selector.shrink_to_fit();
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite, rg_memsize::MemorySize,
)]
pub struct MacroCallItem {
    pub path: Option<String>,
    pub callee: Option<Name>,
    pub args: Option<TopSubtree>,
    pub builtin: Option<BuiltinMacroItem>,
}

impl MacroCallItem {
    pub fn shrink_to_fit(&mut self) {
        if let Some(path) = &mut self.path {
            path.shrink_to_fit();
        }
        if let Some(callee) = &mut self.callee {
            callee.shrink_to_fit();
        }
        if let Some(builtin) = &mut self.builtin {
            builtin.shrink_to_fit();
        }
    }
}
