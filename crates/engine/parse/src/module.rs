//! Module path resolution for ordinary source and macro expansion.

use std::{
    collections::BTreeSet,
    fs as std_fs,
    path::{Path, PathBuf},
};

use anyhow::Context as _;
use rg_ir_model::FileId;
use rg_syntax::{
    AstNode as _, Edition, SourceFile, SyntaxNode,
    ast::{self, HasAttrs, HasModuleItem, HasName},
};
use rg_text::{Name, identifier_text};

use crate::fs;

/// Return the inline modules that contain `node`, from the outermost module inward.
///
/// The syntax spelling is converted to a semantic `Name`, so `mod r#type` contributes `type`.
/// Out-of-line modules are not part of this path because their file already identifies them.
pub fn enclosing_inline_module_path(node: &SyntaxNode) -> Vec<Name> {
    let mut path = node
        .ancestors()
        .filter_map(ast::Module::cast)
        .filter(|module| module.item_list().is_some())
        .filter_map(|module| module.name().map(|name| Name::new(name.text())))
        .collect::<Vec<_>>();
    path.reverse();
    path
}

/// Filesystem bases used to resolve children of one logical Rust module.
///
/// Rust has two related bases. A conventional `mod child;` inside `foo.rs` searches below
/// `foo/`, while `#[path = "child.rs"] mod child;` remains relative to the directory containing
/// `foo.rs`. Keeping both paths makes the context describe how the module was reached instead of
/// trying to reconstruct that information from its `FileId` later.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModuleFileContext {
    child_module_dir: PathBuf,
    path_attr_dir: PathBuf,
}

impl ModuleFileContext {
    /// Starts module traversal at a Cargo target root.
    ///
    /// A custom target such as `src/tool.rs` behaves like `lib.rs`: its children resolve beside
    /// the target file rather than below `src/tool/`.
    pub fn for_target_root(root_file: &Path) -> Self {
        let parent_dir = root_file
            .parent()
            .expect("target root should have a parent directory")
            .to_path_buf();
        Self {
            child_module_dir: parent_dir.clone(),
            path_attr_dir: parent_dir,
        }
    }

    /// Reconstructs the conventional context of a standalone module file.
    ///
    /// Semantic module traversal should use the context returned by `resolve_module_name`. This
    /// constructor is for source-local queries that have only a file and no module provenance.
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

        Self {
            child_module_dir,
            path_attr_dir: parent_dir.to_path_buf(),
        }
    }

    /// Recreates the child context of a file selected by `#[path = "..."]`.
    ///
    /// Unlike a conventional flat module, the file's children stay beside the selected file.
    pub fn for_path_attribute_file(definition_file: &Path) -> Self {
        let parent_dir = definition_file
            .parent()
            .expect("definition file should have a parent directory")
            .to_path_buf();
        Self {
            child_module_dir: parent_dir.clone(),
            path_attr_dir: parent_dir,
        }
    }

    /// Enters an inline module, including the directory override introduced by `#[path]`.
    pub fn descend_inline(&self, module_name: &str, path_override: Option<&str>) -> Self {
        let child_module_dir = path_override
            .and_then(|path| self.resolve_inline_path_attr(path))
            .unwrap_or_else(|| self.child_module_dir.join(module_name));
        Self {
            path_attr_dir: child_module_dir.clone(),
            child_module_dir,
        }
    }

    /// Lists conventional child-module names next to this logical module.
    ///
    /// Both `name.rs` and `name/mod.rs` represent the same declaration candidate. The query is
    /// deliberately request-scoped filesystem work: retaining every undeclared sibling in the
    /// project index would impose a permanent memory cost for an occasional completion site.
    pub fn candidate_module_names(&self) -> anyhow::Result<Vec<String>> {
        let mut candidates = BTreeSet::new();
        let entries = match std_fs::read_dir(&self.child_module_dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("read module directory {}", self.child_module_dir.display())
                });
            }
        };

        for entry in entries {
            let entry = entry.with_context(|| {
                format!(
                    "inspect module directory {}",
                    self.child_module_dir.display()
                )
            })?;
            let file_type = entry
                .file_type()
                .with_context(|| format!("inspect module candidate {}", entry.path().display()))?;
            let path = entry.path();
            let candidate = if file_type.is_file()
                && path.extension().and_then(|extension| extension.to_str()) == Some("rs")
            {
                path.file_stem().and_then(|stem| stem.to_str())
            } else if file_type.is_dir() && path.join("mod.rs").is_file() {
                path.file_name().and_then(|name| name.to_str())
            } else {
                None
            };
            let Some(candidate) = candidate else {
                continue;
            };
            if matches!(candidate, "lib" | "main" | "mod")
                || !Self::is_valid_module_identifier(candidate)
            {
                continue;
            }
            candidates.insert(candidate.to_string());
        }

        Ok(candidates.into_iter().collect())
    }

    /// Validate the filename as a raw identifier so keyword-named modules remain candidates.
    fn is_valid_module_identifier(candidate: &str) -> bool {
        let parsed = SourceFile::parse(&format!("mod r#{candidate};"), Edition::CURRENT);
        if !parsed.errors().is_empty() {
            return false;
        }
        parsed
            .tree()
            .items()
            .find_map(|item| match item {
                ast::Item::Module(module) => module.name(),
                _ => None,
            })
            .is_some_and(|name| identifier_text(&name.text()) == candidate)
    }

    /// Resolves one out-of-line module declaration according to the supported Rust file rules.
    ///
    /// The resolver intentionally handles only the module forms that lowering already supports.
    /// More advanced attribute expansion belongs with a broader module-system implementation.
    pub fn resolve_module_file<E>(
        &self,
        module: &ast::Module,
        exists: impl FnMut(&Path) -> Result<bool, E>,
    ) -> Result<Option<ModuleFileResolution>, E> {
        let Some(module_name) = module.name().map(|name| {
            let text = name.text();
            identifier_text(&text).to_string()
        }) else {
            return Ok(None);
        };
        self.resolve_module_name(
            &module_name,
            module_path_override(module).as_deref(),
            exists,
        )
    }

    /// Resolves a module spelling retained after its source AST is no longer available.
    ///
    /// Declarative macro expansion happens after ItemTree lowering. Keeping this entry point next
    /// to the ordinary AST resolver makes generated `mod child;` requests use exactly the same
    /// conventional and direct-literal `#[path]` rules as source declarations.
    pub fn resolve_module_name<E>(
        &self,
        module_name: &str,
        path_override: Option<&str>,
        mut exists: impl FnMut(&Path) -> Result<bool, E>,
    ) -> Result<Option<ModuleFileResolution>, E> {
        for candidate in self.module_file_candidates(module_name, path_override) {
            if exists(candidate.path())? {
                return Ok(Some(candidate));
            }
        }
        Ok(None)
    }

    /// Resolves against files that an earlier source-discovery pass already captured.
    ///
    /// DefMap uses this after the source inventory is sealed. The callback only maps candidate
    /// paths to IDs for files already known to the package.
    pub fn resolve_known_module_name(
        &self,
        module_name: &str,
        path_override: Option<&str>,
        mut file_id_for_path: impl FnMut(&Path) -> Option<FileId>,
    ) -> Option<(FileId, ModuleFileContext)> {
        self.module_file_candidates(module_name, path_override)
            .into_iter()
            .find_map(|candidate| {
                let file_id = file_id_for_path(candidate.path())?;
                let (_, context) = candidate.into_parts();
                Some((file_id, context))
            })
    }

    /// Builds candidates in Rust's precedence order and attaches each candidate's next context.
    fn module_file_candidates(
        &self,
        module_name: &str,
        path_override: Option<&str>,
    ) -> Vec<ModuleFileResolution> {
        if let Some(path_override) = path_override {
            let Some(path) = fs::resolve_relative_path_literal(&self.path_attr_dir, path_override)
            else {
                return Vec::new();
            };
            let child_module_dir = path
                .parent()
                .expect("relative module path should have a parent directory")
                .to_path_buf();
            return vec![ModuleFileResolution {
                path,
                context: Self {
                    path_attr_dir: child_module_dir.clone(),
                    child_module_dir,
                },
            }];
        }

        let flat_file = self.child_module_dir.join(format!("{module_name}.rs"));
        let nested_module_dir = self.child_module_dir.join(module_name);
        vec![
            ModuleFileResolution {
                path: flat_file,
                context: Self {
                    child_module_dir: nested_module_dir.clone(),
                    path_attr_dir: self.child_module_dir.clone(),
                },
            },
            ModuleFileResolution {
                path: nested_module_dir.join("mod.rs"),
                context: Self {
                    path_attr_dir: nested_module_dir.clone(),
                    child_module_dir: nested_module_dir,
                },
            },
        ]
    }

    /// Resolves an inline `#[path]` as a directory. Empty paths intentionally keep the current
    /// attribute base, matching `#[path = ""] mod inline { ... }`.
    fn resolve_inline_path_attr(&self, path_attr: &str) -> Option<PathBuf> {
        let path = Path::new(path_attr);
        if path.is_absolute() {
            return None;
        }
        Some(self.path_attr_dir.join(path))
    }
}

/// A selected module source together with the context its contents must inherit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleFileResolution {
    path: PathBuf,
    context: ModuleFileContext,
}

impl ModuleFileResolution {
    fn path(&self) -> &Path {
        &self.path
    }

    pub fn into_parts(self) -> (PathBuf, ModuleFileContext) {
        (self.path, self.context)
    }
}

/// Extracts the basic `#[path = "..."]` module override.
///
/// This intentionally handles only direct string-literal attributes. More advanced forms such as
/// `cfg_attr` can be added later when the rest of the module system needs them.
pub fn module_path_override(item: &ast::Module) -> Option<String> {
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

#[cfg(test)]
mod tests {
    use rg_syntax::{AstNode as _, Edition, SourceFile, ast};

    use super::enclosing_inline_module_path;

    #[test]
    fn inline_module_path_uses_semantic_names() {
        let syntax = SourceFile::parse(
            "mod outer { mod r#type { fn target() {} } }",
            Edition::Edition2021,
        )
        .tree();
        let target = syntax
            .syntax()
            .descendants()
            .find_map(ast::Fn::cast)
            .expect("fixture should contain a function");

        let path = enclosing_inline_module_path(target.syntax())
            .into_iter()
            .map(|name| name.to_string())
            .collect::<Vec<_>>();

        assert_eq!(path, ["outer", "type"]);
    }
}
