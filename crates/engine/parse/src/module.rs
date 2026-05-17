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
use ra_syntax::ast::{self, HasAttrs, HasModuleItem, HasName};

use crate::{FileId, Package};

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
    /// Compute the directory used to resolve child modules for a given source file.
    ///
    /// Uses Rust's file conventions: if the file name is `lib.rs`, `main.rs`, or `mod.rs`,
    /// the child-module directory is the file's parent directory; otherwise it is
    /// the parent directory joined with the file stem (e.g., `src/foo.rs` -> `src/foo`).
    ///
    /// Panics if the provided path has no parent, or if the file name or file stem are not UTF‑8.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::path::Path;
    /// let _ctx = ModuleFileContext::from_definition_file(Path::new("src/lib.rs"));
    /// ```
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

    /// Creates a new module-file context for a named child by appending `module_name` to the current child-module directory.
    ///
    /// # Examples
    ///
    /// ```
    /// let ctx = ModuleFileContext { child_module_dir: std::path::PathBuf::from("src") };
    /// let child = ctx.descend("foo");
    /// assert_eq!(child.child_module_dir, std::path::PathBuf::from("src").join("foo"));
    /// ```
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

    /// Resolve a child module declared with `mod name;` using Rust's file-layout conventions.
    ///
    /// Checks for a sibling file named `<child_module_dir>/<module_name>.rs` first, then for a nested
    /// module file `<child_module_dir>/<module_name>/mod.rs`. Returns the first existing path found,
    /// or `None` if neither exists.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::fs;
    /// use std::path::PathBuf;
    /// use tempfile::tempdir;
    ///
    /// // create temporary directory structure
    /// let dir = tempdir().unwrap();
    /// let child_dir = dir.path().join("foo");
    /// fs::create_dir_all(&child_dir).unwrap();
    /// // create nested mod.rs
    /// fs::write(child_dir.join("mod.rs"), "// mod foo").unwrap();
    ///
    /// let ctx = ModuleFileContext { child_module_dir: PathBuf::from(dir.path()) };
    /// let resolved = ctx.resolve_child_file("foo").unwrap();
    /// assert!(resolved.ends_with("foo/mod.rs"));
    /// ```
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

    /// Resolve a `#[path = "..."]` attribute value to a filesystem path relative to the current module context.
    ///
    /// This accepts only a non-empty, relative path string and returns the resolved path joined against
    /// the context's child module directory only if that path exists on disk. Absolute paths or an
    /// empty string are rejected and yield `None`.
    ///
    /// # Examples
    ///
    /// ```
    /// // Given a ModuleFileContext with child_module_dir = "/project/src/lib"
    /// // resolve_path_attr_file("sub/mod.rs") -> Some("/project/src/lib/sub/mod.rs")
    /// // resolve_path_attr_file("/etc/passwd") -> None
    /// ```
    fn resolve_path_attr_file(&self, path_attr: &str) -> Option<PathBuf> {
        let path_attr = Path::new(path_attr);
        if path_attr.as_os_str().is_empty() || path_attr.is_absolute() {
            return None;
        }

        let file = self.child_module_dir.join(path_attr);
        file.exists().then_some(file)
    }
}

struct ModuleDiscovery<'db> {
    package: &'db mut Package,
    visited: HashSet<FileId>,
    active_stack: HashSet<FileId>,
}

impl<'db> ModuleDiscovery<'db> {
    /// Create a new module discovery context bound to the given package.
    ///
    /// The returned `ModuleDiscovery` is ready to traverse the package's crate root files
    /// and populate the package's module-file cache.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// // Prepare a mutable Package instance (omitted).
    /// let mut package = /* ... */ ;
    /// let discovery = ModuleDiscovery::new(&mut package);
    /// ```
    fn new(package: &'db mut Package) -> Self {
        Self {
            package,
            visited: HashSet::default(),
            active_stack: HashSet::default(),
        }
    }

    /// Discover all out-of-line module files reachable from the package's Cargo targets and
    /// populate the package's file cache accordingly.
    ///
    /// The method iterates each target's root file and walks its module graph, delegating
    /// per-file discovery to `discover_file`. Errors produced while processing a target are
    /// annotated with that target's name for easier diagnosis.
    ///
    /// # Returns
    ///
    /// `Ok(())` if discovery completes for all targets, `Err` with context otherwise.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// // given `package: &mut Package`
    /// ModuleDiscovery::new(package).discover().unwrap();
    /// ```
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

    /// Discover and register all out-of-line modules reachable from a single source file.
    ///
    /// Traverses module declarations in the file identified by `current_file_id`, recursively
    /// discovering any out-of-line module files referenced by `mod` declarations or `#[path = "..."]`
    /// overrides and adding them to the package cache. This method is idempotent for files already
    /// visited and guards against infinite recursion using an active traversal stack.
    ///
    /// # Returns
    ///
    /// `Ok(())` on success. Returns an `Err` if loading retained syntax, fetching the parsed file,
    /// or discovering module items fails; errors carry contextual messages describing the failing step.
    ///
    /// # Examples
    ///
    /// ```
    /// // Given a mutable `discovery: ModuleDiscovery` and a `file_id: FileId`,
    /// // call `discovery.discover_file(file_id)` to populate the module cache for that file.
    /// // discovery.discover_file(file_id).unwrap();
    /// ```
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

    /// Traverse the provided AST items and discover any module declarations using the given module file context.
    ///
    /// For each `mod` item in `items`, delegates discovery to `discover_module`, skipping non-module items. Errors from
    /// discovering an individual module are propagated.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// // Construct a ModuleDiscovery and ModuleFileContext according to your test setup.
    /// // Call discover_items with a list of parsed `ast::Item`s to process module declarations.
    /// let mut discovery = /* ModuleDiscovery::new(&mut package) */ todo!();
    /// let items: Vec<ast::Item> = Vec::new();
    /// let ctx: ModuleFileContext = /* context built from a file path */ todo!();
    /// discovery.discover_items(items, &ctx).unwrap();
    /// ```
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

    /// Discovers submodules declared by a given `mod` AST node, recursing into inline item lists or resolving and processing out-of-line module files.
    ///
    /// If `module` has an inline item list, the function descends the module file context (using the module's name when present) and discovers items within that inline list. Otherwise it resolves the module's source file (honoring a `#[path = "..."]` string-literal override when present), parses that file, and continues discovery from the parsed file.
    ///
    /// # Returns
    ///
    /// `Ok(())` on success; an error containing contextual information if parsing or file discovery fails.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// // Illustrative usage (context-dependent): obtain a ModuleDiscovery and call discover_module
    /// # use anyhow::Result;
    /// # fn _example() -> Result<()> {
    /// // let mut discovery = ModuleDiscovery::new(&mut package);
    /// // let module: ast::Module = /* obtained from parsed syntax */ unimplemented!();
    /// // let ctx = ModuleFileContext::from_definition_file(path);
    /// // discovery.discover_module(&module, &ctx)?;
    /// # Ok(())
    /// # }
    /// ```
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

/// Extracts a direct outer `#[path = "..."]` attribute value from a module.

///

/// This only recognizes an outer attribute named exactly `path` whose meta is a key-value

/// with a string literal (e.g. `#[path = "foo.rs"]`). Other forms (including `cfg_attr` or

/// non-literal/indirect expressions) are ignored and will return `None`.

///

/// # Examples

///

/// ```no_run

/// // Given an `ast::Module` `m` that has `#[path = "sub/mod.rs"]`, this returns that string.

/// let _path_opt = module_path_attr(&m);

/// ```
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
