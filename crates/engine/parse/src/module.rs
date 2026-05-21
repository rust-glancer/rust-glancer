//! Package-local module graph discovery.
//!
//! Parsing starts with Cargo target roots, but later lowering phases need every reachable
//! out-of-line module file to already be present in the package file cache. This pass walks the
//! Rust module declarations without allocating item-tree payloads, so syntax retention and
//! item-tree lowering stay in separate lifetime windows.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use anyhow::Context as _;
use rg_syntax::ast::{self, HasAttrs, HasModuleItem, HasName};

use crate::{FileId, Package, fs};

impl Package {
    /// Discovers reachable out-of-line module files before AST-consuming lowering allocates.
    pub fn discover_modules(&mut self) -> anyhow::Result<()> {
        ModuleDiscovery::new(self).discover()
    }
}

/// Filesystem context for resolving out-of-line child modules of the current logical module.
#[derive(Debug, Clone)]
pub struct ModuleFileContext {
    child_module_dir: PathBuf,
}

impl ModuleFileContext {
    /// Builds the child-module directory for a file-backed module.
    pub fn from_definition_file(definition_file: &Path) -> Self {
        let parent_dir = definition_file
            .parent()
            .expect("definition file should have a parent directory");
        let file_name = definition_file
            .file_name()
            .and_then(|name| name.to_str())
            .expect("definition file name should be UTF-8");
        let file_stem = definition_file
            .file_stem()
            .and_then(|stem| stem.to_str())
            .expect("definition file stem should be UTF-8");

        let child_module_dir = match file_name {
            "lib.rs" | "main.rs" | "mod.rs" => parent_dir.to_path_buf(),
            _ => parent_dir.join(file_stem),
        };

        Self { child_module_dir }
    }

    /// Builds the child-module directory for an inline child module.
    pub fn descend(&self, module_name: &str) -> Self {
        Self {
            child_module_dir: self.child_module_dir.join(module_name),
        }
    }

    /// Resolves one out-of-line module declaration according to the supported Rust file rules.
    ///
    /// The resolver intentionally handles only the module forms that lowering already supports.
    /// More advanced attribute expansion belongs with a broader module-system implementation.
    pub fn resolve_module_file(&self, module: &ast::Module) -> Option<PathBuf> {
        let module_name = module.name().map(|name| name.text().to_string())?;
        if let Some(path_attr) = module_path_attr(module) {
            return self.resolve_path_attr_file(&path_attr);
        }

        self.resolve_child_file(&module_name)
    }

    /// Resolves `mod name;` according to conventional Rust module file rules.
    fn resolve_child_file(&self, module_name: &str) -> Option<PathBuf> {
        let flat_file = self.child_module_dir.join(format!("{module_name}.rs"));
        if flat_file.exists() {
            return Some(flat_file);
        }

        let nested_file = self.child_module_dir.join(module_name).join("mod.rs");
        if nested_file.exists() {
            return Some(nested_file);
        }

        None
    }

    /// Resolves the basic literal form of `#[path = "..."]` relative to the current module.
    fn resolve_path_attr_file(&self, path_attr: &str) -> Option<PathBuf> {
        fs::resolve_relative_path_literal(&self.child_module_dir, path_attr)
    }
}

struct ModuleDiscovery<'db> {
    package: &'db mut Package,
    visited: HashSet<FileId>,
    active_stack: HashSet<FileId>,
}

impl<'db> ModuleDiscovery<'db> {
    fn new(package: &'db mut Package) -> Self {
        Self {
            package,
            visited: HashSet::default(),
            active_stack: HashSet::default(),
        }
    }

    fn discover(mut self) -> anyhow::Result<()> {
        let roots = self
            .package
            .targets()
            .iter()
            .map(|target| (target.name.clone(), target.root_file))
            .collect::<Vec<_>>();

        for (target_name, root_file) in roots {
            self.discover_file(root_file).with_context(|| {
                format!("while attempting to discover modules for target {target_name}")
            })?;
        }

        Ok(())
    }

    fn discover_file(&mut self, current_file_id: FileId) -> anyhow::Result<()> {
        if self.visited.contains(&current_file_id) {
            return Ok(());
        }

        // Recursive module graphs can revisit a file before the first traversal finishes.
        if !self.active_stack.insert(current_file_id) {
            return Ok(());
        }

        self.package
            .ensure_file_syntax(current_file_id)
            .with_context(|| {
                format!("while attempting to load syntax for {:?}", current_file_id)
            })?;

        let (items, module_file_context) = {
            let parsed_file = self.package.parsed_file(current_file_id).with_context(|| {
                format!(
                    "while attempting to fetch parsed file {:?}",
                    current_file_id
                )
            })?;
            let syntax = parsed_file.syntax().with_context(|| {
                format!(
                    "while attempting to access retained syntax for {:?}",
                    current_file_id
                )
            })?;
            (
                syntax.items().collect::<Vec<_>>(),
                ModuleFileContext::from_definition_file(parsed_file.path()),
            )
        };

        self.discover_items(items, &module_file_context)
            .with_context(|| {
                format!(
                    "while attempting to discover module items for {:?}",
                    current_file_id
                )
            })?;

        self.active_stack.remove(&current_file_id);
        self.visited.insert(current_file_id);
        Ok(())
    }

    fn discover_items(
        &mut self,
        items: Vec<ast::Item>,
        module_file_context: &ModuleFileContext,
    ) -> anyhow::Result<()> {
        for item in items {
            let ast::Item::Module(module) = item else {
                continue;
            };

            self.discover_module(&module, module_file_context)
                .context("while attempting to discover module declaration")?;
        }

        Ok(())
    }

    fn discover_module(
        &mut self,
        module: &ast::Module,
        module_file_context: &ModuleFileContext,
    ) -> anyhow::Result<()> {
        if let Some(item_list) = module.item_list() {
            // Inline modules do not introduce a file, but their out-of-line children resolve under
            // a directory named after the inline module path.
            let inline_module_context = module
                .name()
                .map(|name| module_file_context.descend(name.text().as_str()))
                .unwrap_or_else(|| module_file_context.clone());
            let inline_items = item_list.items().collect::<Vec<_>>();
            return self
                .discover_items(inline_items, &inline_module_context)
                .context("while attempting to discover inline module items");
        }

        let Some(module_file_path) = module_file_context.resolve_module_file(module) else {
            return Ok(());
        };
        let module_file_id = self
            .package
            .parse_file(&module_file_path)
            .with_context(|| {
                format!(
                    "while attempting to parse module file {}",
                    module_file_path.display()
                )
            })?;

        self.discover_file(module_file_id).with_context(|| {
            format!(
                "while attempting to discover modules from {}",
                module_file_path.display()
            )
        })
    }
}

/// Extracts the basic `#[path = "..."]` module override.
///
/// This intentionally handles only direct string-literal attributes. More advanced forms such as
/// `cfg_attr` can be added later when the rest of the module system needs them.
fn module_path_attr(item: &ast::Module) -> Option<String> {
    for attr in item.attrs() {
        if !attr.kind().is_outer() || attr.simple_name().as_deref() != Some("path") {
            continue;
        }

        let Some(ast::Meta::KeyValueMeta(meta)) = attr.meta() else {
            continue;
        };
        let Some(ast::Expr::Literal(literal)) = meta.expr() else {
            continue;
        };
        let ast::LiteralKind::String(path) = literal.kind() else {
            continue;
        };

        return path.value().ok().map(|path| path.into_owned());
    }

    None
}
