//! The package bridge follows contextual source edges before lowering file facts.

use std::collections::{BTreeSet, HashMap, HashSet};

use anyhow::Context as _;
use rg_ir_model::{FileId, PackageSlot};
use rg_macro_expand::CfgSelect;
use rg_parse::{ModuleFileContext, Package, syntax_edition};
use rg_source::{SourceError, SourceInventory};
use rg_syntax::ast::{self, HasModuleItem, HasName};
use rg_tt::syntax_bridge::SpanFactory;

use crate::IncludePathExpression;

pub(super) struct SourceDiscovery<'a> {
    pub(super) files: BTreeSet<FileId>,
    pub(super) includes: HashMap<FileId, HashMap<IncludePathExpression, FileId>>,
    package_slot: PackageSlot,
    package: &'a mut Package,
    sources: &'a SourceInventory,
    active: HashSet<FileId>,
}

impl<'a> SourceDiscovery<'a> {
    pub(super) fn new(
        package_slot: PackageSlot,
        package: &'a mut Package,
        sources: &'a SourceInventory,
    ) -> Self {
        Self {
            files: BTreeSet::new(),
            includes: HashMap::new(),
            package_slot,
            package,
            sources,
            active: HashSet::new(),
        }
    }

    pub(super) fn visit_root(&mut self, file: FileId) -> anyhow::Result<()> {
        let path = self
            .package
            .file_path(file)
            .expect("target root has a parsed path");
        self.visit_file(file, &ModuleFileContext::for_target_root(path))
    }

    pub(super) fn visit_file(
        &mut self,
        file: FileId,
        context: &ModuleFileContext,
    ) -> anyhow::Result<()> {
        // The same source can be included under multiple logical directories. Only a visit on
        // the current recursion path is a cycle regardless of its context.
        if !self.active.insert(file) {
            return Ok(());
        }
        self.files.insert(file);
        let result = (|| {
            self.package.ensure_file_syntax(file)?;
            let syntax = self
                .package
                .parsed_file(file)
                .and_then(|file| file.syntax())
                .context("discovered source should have syntax")?;
            self.visit_items(file, context, syntax.items())
        })();
        self.active.remove(&file);
        result
    }

    fn visit_items(
        &mut self,
        file: FileId,
        context: &ModuleFileContext,
        items: impl Iterator<Item = ast::Item>,
    ) -> anyhow::Result<()> {
        for item in items {
            match item {
                ast::Item::Module(module) => {
                    if let Some(items) = module.item_list() {
                        let inline_context = module.name().map_or_else(
                            || context.clone(),
                            |name| {
                                context.descend_inline(
                                    rg_text::identifier_text(&name.text()),
                                    rg_parse::module_path_override(&module).as_deref(),
                                )
                            },
                        );
                        self.visit_items(file, &inline_context, items.items())?;
                    } else if let Some(resolution) =
                        context.resolve_module_file(self.sources, &module)?
                    {
                        let (path, child_context) = resolution.into_parts();
                        let child = self.package.parse_file(self.sources, &path)?;
                        self.visit_file(child, &child_context)?;
                    }
                }
                ast::Item::MacroCall(call) => self.visit_call(file, context, &call)?,
                ast::Item::Trait(item) => {
                    if let Some(items) = item.assoc_item_list() {
                        self.visit_assoc_calls(file, context, items)?;
                    }
                }
                ast::Item::Impl(item) => {
                    if let Some(items) = item.assoc_item_list() {
                        self.visit_assoc_calls(file, context, items)?;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn visit_assoc_calls(
        &mut self,
        file: FileId,
        context: &ModuleFileContext,
        items: ast::AssocItemList,
    ) -> anyhow::Result<()> {
        for item in items.assoc_items() {
            if let ast::AssocItem::MacroCall(call) = item {
                self.visit_call(file, context, &call)?;
            }
        }
        Ok(())
    }

    fn visit_call(
        &mut self,
        file: FileId,
        context: &ModuleFileContext,
        call: &ast::MacroCall,
    ) -> anyhow::Result<()> {
        let edition = self.package.edition();
        if let Some(include) = IncludePathExpression::from_macro_call(call, edition) {
            let origin = self
                .package
                .parsed_file(file)
                .context("include origin is missing")?;
            if let Some(path) = include.resolve(&origin, self.package.cargo_generated_sources())
                && self.sources.probe_exists(&path)?
            {
                let discovery = self
                    .package
                    .parse_file(self.sources, &path)
                    .and_then(|child| {
                        if self.active.contains(&child) {
                            return Ok(None);
                        }
                        // The include's modules use the logical directory at the call site.
                        self.visit_file(child, context)?;
                        Ok(Some(child))
                    });
                match discovery {
                    Ok(Some(child)) => {
                        self.includes
                            .entry(file)
                            .or_default()
                            .insert(include, child);
                    }
                    // The call may name a user macro. Let the resolved builtin request the file
                    // later if eager discovery could not read it or one of its descendants.
                    Ok(None) => {}
                    Err(error) if Self::is_unreadable_source(&error) => {}
                    Err(error) => return Err(error),
                }
            }
            return Ok(());
        }

        let factory = SpanFactory::new(
            u32::try_from(self.package_slot.0).expect("package slot should fit macro span storage"),
            file.as_raw(),
            syntax_edition(edition),
        );
        if let Some(select) = CfgSelect::from_macro_call(call, &mut |range| factory.span_for(range))
        {
            for arm in select.arms() {
                let Some((items, _)) = arm.parse_items() else {
                    continue;
                };
                if let Err(error) = self.visit_items(file, context, items.items())
                    && !Self::is_unreadable_source(&error)
                {
                    return Err(error);
                }
            }
        }
        Ok(())
    }

    // A speculative macro visit may name files that the resolved macro or selected cfg arm
    // never uses. A source race or an internal failure still invalidates the build.
    fn is_unreadable_source(error: &anyhow::Error) -> bool {
        error
            .downcast_ref::<SourceError>()
            .is_some_and(SourceError::is_unreadable)
    }
}
