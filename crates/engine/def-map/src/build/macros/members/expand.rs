use std::{collections::HashSet, rc::Rc};

use rg_cfg_eval::CfgEvaluator;
use rg_ir_model::{CrateRef, FileId, Path};
use rg_item_tree::{
    BuiltinMacroItem, BuiltinMacroKind, CfgSelectArmPayload, IncludePathExpression, ItemKind,
    ItemNode, ItemTreeId, ItemTreeRef, MacroCallItem, file_declarations,
};
use rg_macro_runtime::{
    CfgSelect, ExpansionParseKind, ExpansionSyntax, MacroExpansionRequest,
    SharedMacroExpansionRuntime,
};
use rg_source::{QueryContext, SourceInput};
use rg_text::RustEdition;
use rg_tt::{Span as TtSpan, TopSubtree};
use rg_workspace::{CrateConfig, PackageInput};

use super::{AssociatedItemInput, AssociatedItemSource};
use crate::{
    CrateResolutionEnv, GeneratedItemStore, GeneratedSourceStore, GeneratedSourceStoreId,
    ItemSource, MacroDefinitionEnv,
    build::macros::{
        ItemOrder, MAX_MACRO_EXPANSION_DEPTH, MacroScopeData,
        generated_tree::GeneratedSourceLowering, resolve::ItemMacroResolver,
    },
};

/// One owner's declarations after replacing member macro calls with their output.
///
/// The list keeps source order, including generated members before the next written declaration.
/// Semantic lowering assigns member IDs from this list. After it copies the declaration meaning,
/// only source maps and file membership need to survive from this expansion result.
pub struct ExpandedAssociatedItems {
    pub items: Vec<(ItemSource, ItemNode)>,
    pub sources: GeneratedSourceStore,
    pub files: Vec<FileId>,
}

/// Share one macro cache across this crate's member queries during one Ajika operation.
/// The crate argument groups callers even though constructing the runtime needs no input reads.
/// Each caller still resolves its macro and observes the definition before using the cache.
///
/// `transient` releases the cache at operation end. Compiled macros are keyed by definition ID,
/// so keeping them across source revisions could reuse a body changed by an edit. `returns(ref)`
/// borrows that shared runtime from the operation, including when owners expand in parallel.
#[ajika::query(transient, returns(ref))]
fn member_macro_runtime(
    db: &QueryContext<'_>,
    krate: CrateRef,
) -> ajika::Result<SharedMacroExpansionRuntime> {
    let _ = (db, krate);
    Ok(SharedMacroExpansionRuntime::new())
}

/// One declaration and the context inherited by calls nested inside its expansion.
/// `source` addresses this node's arena; `origin_source` keeps the written declaration from
/// which the expansion chain started, even after generated children receive their own arenas.
struct PendingMember {
    source: ItemSource,
    item: ItemNode,
    origin_source: ItemTreeRef,
    dollar_crate: Option<CrateRef>,
    order: ItemOrder,
    /// Number of enclosing expansions. Siblings do not consume each other's recursion allowance.
    depth: usize,
    /// Files entered through `include!` on this branch, retained through other macro expansions.
    /// Sibling branches and other owners can still include the same file independently.
    /// Children share this history until another include extends it.
    include_ancestors: Rc<[FileId]>,
}

/// Macro output either reuses source declarations or supplies syntax needing a new source map.
/// Keeping these cases separate preserves the original identities of includes and cfg arms.
enum MemberExpansion<'call> {
    IncludedFile(FileId),
    /// IDs in the parent call's source arena, not newly allocated expansion slots.
    SelectedItems(&'call [ItemTreeId]),
    Generated {
        syntax: ExpansionSyntax,
        call_site: TtSpan,
        dollar_crate: Option<CrateRef>,
    },
}

impl MemberExpansion<'_> {
    fn from_syntax(
        syntax: ExpansionSyntax,
        call_site: TtSpan,
        dollar_crate: Option<CrateRef>,
    ) -> Option<Self> {
        // Malformed item syntax contributes no declarations or generated source slots.
        syntax
            .parse
            .parse_errors()
            .is_empty()
            .then_some(Self::Generated {
                syntax,
                call_site,
                dollar_crate,
            })
    }
}

impl AssociatedItemInput {
    /// Expand one owner's member list against the settled module scopes. Members cannot add
    /// module bindings, so this work has no import-resolution fixed point and no mutable DefMap.
    /// New expansions belong to `source_store`; existing sources retain their original identity,
    /// including members whose entire impl was emitted by a module-level macro.
    pub fn expand(
        &self,
        db: &QueryContext<'_>,
        krate: CrateRef,
        source_store: GeneratedSourceStoreId,
        scopes: &ajika::Output<MacroScopeData>,
        env: &(impl CrateResolutionEnv + MacroDefinitionEnv),
    ) -> anyhow::Result<ExpandedAssociatedItems> {
        let Self::Source(input) = self else {
            return Ok(ExpandedAssociatedItems {
                items: Vec::new(),
                sources: GeneratedSourceStore::default(),
                files: Vec::new(),
            });
        };
        let config = CrateConfig::read(db, krate)?;
        MemberExpansionBuilder {
            input,
            db,
            krate,
            scopes,
            env,
            cfg: config.cfg(),
            edition: config.edition,
            pending: Vec::new(),
            items: Vec::new(),
            generated: GeneratedItemStore::new(source_store),
            files: HashSet::from([input.source.file_id]),
        }
        .build()
    }
}

/// Owns the traversal of one member list. Macro dispatch describes what to insert; enqueuing
/// applies the source, order, and depth rules without changing how the macro was resolved.
struct MemberExpansionBuilder<'a, 'db, E> {
    input: &'a AssociatedItemSource,
    db: &'a QueryContext<'db>,
    krate: CrateRef,
    scopes: &'a ajika::Output<MacroScopeData>,
    env: &'a E,
    cfg: CfgEvaluator<'a>,
    edition: RustEdition,
    pending: Vec<PendingMember>,
    items: Vec<(ItemSource, ItemNode)>,
    generated: GeneratedItemStore,
    files: HashSet<FileId>,
}

impl<E: CrateResolutionEnv + MacroDefinitionEnv> MemberExpansionBuilder<'_, '_, E> {
    fn build(mut self) -> anyhow::Result<ExpandedAssociatedItems> {
        // This is a stack: push siblings in reverse order so an expansion is fully visited before
        // the following written member. For `methods!(); fn last() {}`, generated functions must
        // precede `last` when semantic lowering allocates their IDs.
        let include_ancestors = Rc::<[FileId]>::default();
        for (index, &id) in self.input.items.iter().enumerate().rev() {
            let source = self.input.source.with_item(id);
            self.pending.push(PendingMember {
                source,
                item: self.input.item(self.db, self.krate, source)?.clone(),
                origin_source: self.input.origin_source,
                dollar_crate: self.input.dollar_crate,
                order: self.input.order.generated_child(index),
                depth: 0,
                include_ancestors: Rc::clone(&include_ancestors),
            });
        }
        while let Some(member) = self.pending.pop() {
            self.db.check_cancelled()?;
            if !self.cfg.is_enabled(&member.item.cfg) {
                continue;
            }
            match &member.item.kind {
                ItemKind::Function(_) | ItemKind::TypeAlias(_) | ItemKind::Const(_) => {
                    self.items.push((member.source, member.item));
                }
                ItemKind::MacroCall(call) => {
                    if let Some(expansion) = self.expand_macro(&member, call)? {
                        self.enqueue_expansion(&member, expansion)?;
                    }
                }
                _ => {}
            }
        }
        // This becomes a persisted output: hash iteration order must not look like a file change.
        let mut files: Vec<_> = self.files.into_iter().collect();
        files.sort_unstable();
        Ok(ExpandedAssociatedItems {
            items: self.items,
            sources: self.generated.into_sources(self.db)?,
            files,
        })
    }

    /// Resolve and expand one call without changing the traversal. `None` means there is no usable
    /// expansion; query failures still propagate to the caller.
    fn expand_macro<'call>(
        &self,
        member: &PendingMember,
        call: &'call MacroCallItem,
    ) -> anyhow::Result<Option<MemberExpansion<'call>>> {
        let Some(input) = &call.input else {
            return Ok(None);
        };
        let Some(path_text) = call.path.as_deref().or(call.callee.as_deref()) else {
            return Ok(None);
        };
        let Some(args) = input.args.as_ref() else {
            return Ok(None);
        };
        let Some(path) = Path::from_text(path_text, input.path_crate.or(member.dollar_crate))
        else {
            return Ok(None);
        };
        // Ordinary written members do not observe macro scopes. Acquire them only after
        // this owner actually reaches a call that needs name resolution.
        let resolver =
            ItemMacroResolver::new(self.env, self.krate, self.scopes.value_ref(self.db)?);
        let Some(resolved) = resolver.resolve(self.input.module, &member.order, &path)? else {
            return Ok(None);
        };
        if resolved.definition.is_proc_macro() {
            return Ok(None);
        }
        if member.depth >= MAX_MACRO_EXPANSION_DEPTH {
            tracing::warn!(%path_text, "associated macro expansion reached recursion limit");
            return Ok(None);
        }

        // Keep the token-tree call site. A selected cfg_select arm can give the call a new
        // declaration owner while its argument tokens still belong to the original invocation.
        match resolved.definition.data.builtin {
            Some(BuiltinMacroKind::Include) => Ok(self
                .include_file(member.source.file_id, call, args)?
                .map(MemberExpansion::IncludedFile)),
            Some(BuiltinMacroKind::CfgSelect) => {
                Ok(self.cfg_selection(call, args, input.call_site, member.dollar_crate))
            }
            Some(_) => Ok(None),
            None => {
                let Some(definition) = resolved.definition.declarative_definition() else {
                    return Ok(None);
                };
                let syntax =
                    member_macro_runtime(self.db, self.krate)?.expand(MacroExpansionRequest {
                        def_ref: resolved.definition.def_ref,
                        definition,
                        path_text,
                        args,
                        call_site: input.call_site,
                        parse_kind: ExpansionParseKind::Items,
                    });
                Ok(syntax.and_then(|syntax| {
                    MemberExpansion::from_syntax(
                        syntax,
                        input.call_site,
                        Some(resolved.definition.data.dollar_crate),
                    )
                }))
            }
        }
    }

    /// Resolve an include relative to its invocation. The readability probe also records failed
    /// reads, so a later source update can invalidate an include that contributed no members.
    fn include_file(
        &self,
        origin_file: FileId,
        call: &MacroCallItem,
        args: &TopSubtree,
    ) -> anyhow::Result<Option<FileId>> {
        let path = match &call.builtin {
            Some(BuiltinMacroItem::Include { path }) => Some(path.clone()),
            _ => IncludePathExpression::from_macro_arguments(args, self.edition),
        };
        let Some(path) = path else { return Ok(None) };
        let origin = origin_file.path(&self.db.interner())?;
        let generated_paths = PackageInput::at(self.krate.package).generated_sources(self.db)?;
        let Some(path) = path.resolve(&origin, generated_paths.as_ref()) else {
            return Ok(None);
        };
        let Some(file) = SourceInput::file_at(self.db, &path)? else {
            return Ok(None);
        };
        Ok(SourceInput::is_readable(self.db, file)?.then_some(file))
    }

    /// Prefer already-lowered cfg arms so their declarations keep their original source IDs.
    /// Calls without a prelowered arm list parse the selected payload as new item syntax.
    fn cfg_selection<'call>(
        &self,
        call: &'call MacroCallItem,
        args: &TopSubtree,
        call_site: TtSpan,
        dollar_crate: Option<CrateRef>,
    ) -> Option<MemberExpansion<'call>> {
        if let Some(BuiltinMacroItem::CfgSelect { arms }) = &call.builtin {
            let arm = arms
                .iter()
                .find(|arm| self.cfg.is_predicate_enabled(&arm.predicate))?;
            return match &arm.payload {
                CfgSelectArmPayload::Items(ids) => Some(MemberExpansion::SelectedItems(ids)),
                CfgSelectArmPayload::LoweringFailed => None,
            };
        }
        let selection = CfgSelect::parse(args)?;
        let arm = selection
            .arms()
            .iter()
            .find(|arm| self.cfg.is_predicate_enabled(&arm.predicate))?;
        MemberExpansion::from_syntax(
            ExpansionSyntax::from_token_tree(arm.payload.clone(), ExpansionParseKind::Items),
            call_site,
            dollar_crate,
        )
    }

    /// Splice children ahead of the next sibling. Includes start their own written provenance;
    /// selected cfg items keep their arena, while generated syntax gets an owner-local source map.
    fn enqueue_expansion(
        &mut self,
        parent: &PendingMember,
        expansion: MemberExpansion<'_>,
    ) -> anyhow::Result<()> {
        match expansion {
            MemberExpansion::IncludedFile(file) => {
                // Stop a recursive include before it contributes another copy of its members.
                if parent.include_ancestors.contains(&file) {
                    return Ok(());
                }
                let include_ancestors: Rc<[FileId]> = parent
                    .include_ancestors
                    .iter()
                    .copied()
                    .chain([file])
                    .collect();

                // Included text starts a written source chain. It does not inherit the definition
                // crate used to interpret `$crate` in the macro that emitted the include call.
                self.files.insert(file);
                let tree = file_declarations(self.db, self.krate.package, file)?
                    .items
                    .value_ref(self.db)?;
                for (index, &id) in tree.top_level.iter().enumerate().rev() {
                    let written = ItemTreeRef {
                        file_id: file,
                        item: id,
                    };
                    self.pending.push(PendingMember {
                        source: written.into(),
                        item: tree.items[id].clone(),
                        origin_source: written,
                        dollar_crate: None,
                        order: parent.order.generated_child(index),
                        depth: parent.depth + 1,
                        include_ancestors: Rc::clone(&include_ancestors),
                    });
                }
            }
            MemberExpansion::SelectedItems(ids) => {
                for (index, &id) in ids.iter().enumerate().rev() {
                    let source = parent.source.with_item(id);
                    self.pending.push(PendingMember {
                        source,
                        item: self.input.item(self.db, self.krate, source)?.clone(),
                        origin_source: parent.origin_source,
                        dollar_crate: parent.dollar_crate,
                        order: parent.order.generated_child(index),
                        depth: parent.depth + 1,
                        include_ancestors: Rc::clone(&parent.include_ancestors),
                    });
                }
            }
            MemberExpansion::Generated {
                syntax,
                call_site,
                dollar_crate,
            } => {
                let data = GeneratedSourceLowering::lower(
                    parent.origin_source,
                    parent.source.file_id,
                    call_site,
                    dollar_crate,
                    syntax,
                )?;
                let source_id = self.generated.alloc_source(data);
                let data = self
                    .generated
                    .source(source_id)
                    .expect("new expansion source exists");
                for (index, &id) in data.top_level.iter().enumerate().rev() {
                    self.pending.push(PendingMember {
                        source: ItemSource::generated(
                            parent.source.file_id,
                            self.generated.item_ref(source_id, id),
                        ),
                        item: data.items[id].clone(),
                        origin_source: parent.origin_source,
                        dollar_crate,
                        order: parent.order.generated_child(index),
                        depth: parent.depth + 1,
                        include_ancestors: Rc::clone(&parent.include_ancestors),
                    });
                }
            }
        }
        Ok(())
    }
}
