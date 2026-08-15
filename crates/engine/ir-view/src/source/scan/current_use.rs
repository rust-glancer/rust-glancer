//! Module-level import paths read directly from captured source.
//!
//! Saved DefMap owns the namespace in which a path resolves, but its import spans belong to saved
//! text. When the editor source differs, this scanner reads the path and span from current syntax
//! and uses only the enclosing module path to select the matching saved module.

use rg_def_map::{DefMapReadTxn, ImportPath};
use rg_ir_model::CrateRef;
use rg_item_tree::{FromAst as _, ImportAlias, UseItem};
use rg_package_store::PackageStoreError;
use rg_parse::{FileId, enclosing_inline_module_path};
use rg_syntax::{AstNode as _, SourceFile, TextSize, ast};
use rg_text::NameInterner;

use super::{DefinitionSourceCandidate, ModuleSourceSiteScanner};

/// Finds the module-level `use` path segment under one current-source cursor.
pub(crate) struct CurrentUsePathScanner<'source, 'txn, 'db> {
    def_map: &'txn DefMapReadTxn<'db>,
    crate_ref: CrateRef,
    file_id: FileId,
    source: &'source SourceFile,
    offset: u32,
}

impl<'source, 'txn, 'db> CurrentUsePathScanner<'source, 'txn, 'db> {
    pub(crate) fn new(
        def_map: &'txn DefMapReadTxn<'db>,
        crate_ref: CrateRef,
        file_id: FileId,
        source: &'source SourceFile,
        offset: u32,
    ) -> Self {
        Self {
            def_map,
            crate_ref,
            file_id,
            source,
            offset,
        }
    }

    /// Attach a current import spelling to the saved module that owns its namespace.
    pub(crate) fn scan(&self) -> Result<Vec<DefinitionSourceCandidate>, PackageStoreError> {
        let Some(use_item) = self.use_item_at_offset() else {
            return Ok(Vec::new());
        };
        let inline_module_path = enclosing_inline_module_path(use_item.syntax())
            .into_iter()
            .map(|name| name.to_string())
            .collect::<Vec<_>>();
        let Some(module) = ModuleSourceSiteScanner::module_for_inline_path(
            self.def_map,
            self.crate_ref,
            self.file_id,
            &inline_module_path,
        )?
        else {
            return Ok(Vec::new());
        };

        // Reuse ordinary item-tree import lowering so roots, nested use trees, `self`, and aliases
        // have exactly the same semantic path shape as saved imports. Only the source tree and
        // selected module differ here.
        let mut interner = NameInterner::new();
        let item = UseItem::from_ast(&use_item, &mut interner);
        let mut candidates = Vec::new();
        for import in item.imports {
            let Some(path) = ImportPath::from_use_path(&import.path, None) else {
                continue;
            };

            if let ImportAlias::Explicit { span, .. } = import.alias
                && span.touches(self.offset)
            {
                let candidate = DefinitionSourceCandidate::ImportAlias {
                    module,
                    path: path.semantic().clone(),
                    file_id: self.file_id,
                    span,
                };
                if !candidates.contains(&candidate) {
                    candidates.push(candidate);
                }
            }

            let Some(prefixes) = path.prefixes_with_spans() else {
                continue;
            };
            for (path, span) in prefixes {
                if !span.touches(self.offset) {
                    continue;
                }
                let candidate = DefinitionSourceCandidate::UsePath {
                    module,
                    path,
                    file_id: self.file_id,
                    span,
                };
                if !candidates.contains(&candidate) {
                    candidates.push(candidate);
                }
            }
        }

        Ok(candidates)
    }

    /// Select only an item owned by a source file or inline module.
    ///
    /// A `use` statement inside a function has a lexical body scope and must keep using current
    /// Body IR. Treating it as module syntax would resolve the same text in the wrong namespace.
    fn use_item_at_offset(&self) -> Option<ast::Use> {
        self.source
            .syntax()
            .token_at_offset(TextSize::from(self.offset))
            .filter(|token| !token.kind().is_trivia())
            .filter_map(|token| token.parent_ancestors().find_map(ast::Use::cast))
            .filter(|use_item| {
                use_item.syntax().parent().is_some_and(|parent| {
                    SourceFile::can_cast(parent.kind()) || ast::ItemList::can_cast(parent.kind())
                })
            })
            .min_by_key(|use_item| use_item.syntax().text_range().len())
    }
}
