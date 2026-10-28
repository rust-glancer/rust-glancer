//! Import scheduling and updates to the scopes lent by builders.
//!
//! Named imports wait for path lookup to expose answers; resolved globs forward changes from
//! their source modules. Both update the live scopes immediately, so later lookups in the same
//! run can use those answers. Builders take the updated scopes back between collection runs.

use std::{collections::VecDeque, mem};

use rg_ir_model::{
    CrateRef, DefId, ImportId, ImportRef, LocalDefRef, LocalEnumVariantRef, ModuleRef,
};
use rg_package_store::PackageStoreError;
use rg_text::Name;
use rustc_hash::FxHashMap;

use super::{ImportApplicationStats, ModuleSetShape};
use crate::{
    CrateResolutionEnv, GlobImportSource, ImportBinding, ImportData, ImportKind, ItemScopeBuilder,
    LocalDefData, LocalDefKind, LocalEnumVariantData, LocalEnumVariantEntry, ModuleResolutionView,
    NameBinding, NameBindingProvenance, NameBindingsView, NameResolutionEnv, NameResolver,
    Namespace, NamespaceSet,
    profile::{ImportResolutionMetrics, metric},
};

// Even with stable conflict winners, a chain of updates through cyclic globs needs a bound.
// Count glob-to-glob hops, matching rust-analyzer's recursion limit. The queue avoids stack growth;
// reaching the limit keeps the answers already found and stops forwarding that branch.
const GLOB_PROPAGATION_LIMIT: usize = 100;

/// Borrowed import IDs for one mutable module, in collection order.
///
/// Repeated calls must keep existing IDs in place and append newly collected imports. The collector
/// remembers how many imports it has registered for each module and reads only the appended tail.
pub struct ModuleImportInput<'a> {
    pub module: ModuleRef,
    pub imports: &'a [ImportId],
}

/// Supplies the declarations and lookup data needed to collect imports.
///
/// The caller owns module metadata, item data, and access to frozen dependencies. The collector
/// temporarily owns the mutable scope buffers and overlays them on these reads. Keeping that
/// split lets the same algorithm serve an active crate or one body-local DefMap.
pub trait ImportCollectionEnv: CrateResolutionEnv<Error = PackageStoreError> {
    /// All modules with imports in this build, including those registered by an earlier call.
    fn import_modules(&self) -> Vec<ModuleImportInput<'_>>;
    fn import_data(&self, import: ImportRef) -> &ImportData;
    /// Return false to keep a public binding local to its module, as proc-macro exports require.
    fn allow_public_binding(
        &self,
        _module: ModuleRef,
        _namespace: Namespace,
        _binding: &NameBinding,
    ) -> bool {
        true
    }
    fn check_cancelled(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

/// A scope buffer and its matching unresolved-import report, ready to return to its builder.
pub struct ResolvedModuleImports {
    pub module: ModuleRef,
    pub scope: ItemScopeBuilder,
    /// Imports for which no namespace answer or glob source was found. Partial answers count
    /// as usable imports, and an empty but resolved glob does not appear here.
    pub unresolved: Vec<ImportId>,
}

/// Updated scope buffers to put back into the builders after one collection run.
pub struct ImportCollectionOutput {
    pub modules: Vec<ResolvedModuleImports>,
    pub(crate) metrics: ImportResolutionMetrics,
}

/// How far one import has progressed within a call to `collect`.
///
/// A name can have separate type, value, and macro answers. Finding one is already useful, but
/// another import may expose an answer in a different namespace on the next pass.
#[derive(Clone, Copy)]
enum ImportState {
    Unresolved,
    /// The namespaces found on the first successful lookup. Retry until this set changes, then
    /// record the new answers and retire the import. Changing only a target does not count as
    /// progress: conflicting imports must not keep each other alive by exchanging definitions.
    Partial(NamespaceSet),
    /// A glob has a source, or a named import needs no more retries in this run.
    Resolved,
}

/// One registered import path. Its vector index is also the ID used by resolved glob links.
struct ImportDirective {
    reference: ImportRef,
    module: usize,
    state: ImportState,
}

struct WorkingModule {
    module: ModuleRef,
    scope: ItemScopeBuilder,
    /// Length of the append-only import list already registered in `directives`.
    import_count: usize,
    /// Per-run profiling flags; neither controls whether imports are retried.
    evaluated: bool,
    changed: bool,
}

/// A resolved `use source::*`. Keep the source so later changes can be forwarded without
/// resolving the path again; enum sources are scanned for variants instead of module names.
struct ResolvedGlob {
    directive: usize,
    source: GlobImportSource,
}

/// Collects import answers without rebuilding scopes from declarations on every pass.
///
/// There are two kinds of work:
/// - `use source::Item` waits until lookup finds an answer. It can stay pending for missing
///   namespaces, but an already occupied namespace does not keep chasing competing definitions.
/// - `use source::*` resolves its source once, copies its visible names, and remembers a link.
///   Later changes in that source travel through the link as individual name/namespace updates.
///
/// Keep this value across macro batches. Builders lend their scope buffers to `collect` and take
/// them back before expanding macros. The import list and glob links survive between calls;
/// named imports are reconsidered against the new declarations. No scheduling state is frozen.
///
/// This follows rust-analyzer's best-effort policy: unrelated equal-priority conflicts keep the
/// first winner, while the winning import may update its own target. Earlier answers remain if
/// their source loses a namespace, and a resolved glob keeps its selected source. TODO: Handle
/// removed namespaces and changed glob prefixes if these retained answers prove unhelpful.
#[derive(Default)]
pub struct ImportCollector {
    modules: Vec<WorkingModule>,
    module_indices: FxHashMap<ModuleRef, usize>,
    directives: Vec<ImportDirective>,
    globs: Vec<ResolvedGlob>,
    /// Source module -> indices in `globs` that copy names out of that module.
    globs_by_source: FxHashMap<ModuleRef, Vec<usize>>,
    /// Changed slot and number of glob hops from the insertion that started its propagation.
    /// The binding is read when processing the event, so it reflects the latest selected answer.
    changes: VecDeque<(ModuleRef, Name, Namespace, usize)>,
    stats: ImportApplicationStats,
}

impl ImportCollector {
    /// Extend the supplied scopes until pending imports make no more progress.
    ///
    /// Supply every mutable module with imports, including modules handled by earlier calls.
    /// These are the same buffers returned last time, possibly with macro-generated declarations
    /// added. Modules without imports can stay in `env`: they only supply names, never receive
    /// imported ones. All supplied buffers return in the output; builders must restore them before
    /// macro lookup or freezing.
    pub fn collect<E: ImportCollectionEnv>(
        &mut self,
        env: &E,
        mut scopes: Vec<(ModuleRef, ItemScopeBuilder)>,
    ) -> anyhow::Result<ImportCollectionOutput> {
        self.stats = ImportApplicationStats::default();
        // 1. Take the scope buffers back from the builders and register newly appended imports.
        // Stable package/crate/module order also makes first-winner conflict choices repeatable.
        scopes.sort_unstable_by_key(|(module, _)| Self::module_key(module));
        for (module, scope) in scopes {
            if let Some(&index) = self.module_indices.get(&module) {
                self.modules[index].scope = scope;
            } else {
                self.module_indices.insert(module, self.modules.len());
                self.modules.push(WorkingModule {
                    module,
                    scope,
                    import_count: 0,
                    evaluated: false,
                    changed: false,
                });
            }
        }
        let mut inputs = env.import_modules();
        inputs.sort_unstable_by_key(|input| Self::module_key(&input.module));
        for input in inputs {
            let owner = self.module_indices[&input.module];
            for &import in input.imports.iter().skip(self.modules[owner].import_count) {
                self.directives.push(ImportDirective {
                    reference: ImportRef {
                        origin: input.module.origin,
                        import,
                    },
                    module: owner,
                    state: ImportState::Unresolved,
                });
            }
            self.modules[owner].import_count = input.imports.len();
        }

        // 2. Reconsider named imports after collection has had a chance to add declarations.
        // For example, `use bridge::Item` may first see a glob import in `bridge`; a macro can
        // then declare `Item` there and shadow that glob. Crate construction completes dependencies
        // first, but names inside the active crate can still change between macro batches.
        // Keep the old scope answers and winner IDs: the same import can refresh its own target.
        let mut pending = Vec::new();
        for (id, directive) in self.directives.iter_mut().enumerate() {
            if env.import_data(directive.reference).kind != ImportKind::Glob {
                directive.state = ImportState::Unresolved;
            }
            if !matches!(directive.state, ImportState::Resolved) {
                pending.push(id);
            }
        }

        // 3. Copy new declarations through the glob links kept from earlier calls. Rescanning
        // each source once avoids recording every direct insertion made during macro expansion.
        // A registered source stays selected even if later lookup would choose another prefix.
        for glob in 0..self.globs.len() {
            self.scan_glob(env, glob)?;
            self.propagate(env)?;
        }

        // 4. Retry only imports that still need answers. Insertions are visible immediately, so
        // `use foo::Item; use crate::source as foo;` can resolve on the next pass. Each insertion
        // also drains glob forwarding before the next directive looks at the scopes.
        loop {
            env.check_cancelled()?;
            let candidates = mem::take(&mut pending);
            let mut progress = false;
            for id in candidates {
                env.check_cancelled()?;
                self.stats.imports_evaluated += 1;
                let directive = &self.directives[id];
                let owner = directive.module;
                self.modules[owner].evaluated = true;
                let module = self.modules[owner].module;
                let data = env.import_data(directive.reference);
                if data.kind == ImportKind::Glob {
                    self.stats.glob_imports_evaluated += 1;
                    let source = NameResolver::new(&ImportQueryEnv { state: self, env })
                        .import_glob_source(module, data.path.semantic())?;
                    if let Some(source) = source {
                        // Finding an empty module still resolves the glob. The link will supply
                        // names if the module grows later; the `use` itself needs no more retries.
                        let glob = self.globs.len();
                        self.globs.push(ResolvedGlob {
                            directive: id,
                            source,
                        });
                        if let GlobImportSource::Module(source) = source {
                            self.globs_by_source.entry(source).or_default().push(glob);
                        }
                        self.directives[id].state = ImportState::Resolved;
                        self.scan_glob(env, glob)?;
                        self.propagate(env)?;
                        progress = true;
                    } else {
                        pending.push(id);
                    }
                    continue;
                }
                let bindings = NameResolver::new(&ImportQueryEnv { state: self, env })
                    .import_bindings(module, data.path.semantic(), NamespaceSet::ALL)?;
                // Track occupied namespaces, not the identities selected inside them. A type
                // alias may appear before a value of the same name; competing types alone must
                // not trigger another round of recording and replacing answers.
                let available = bindings
                    .iter()
                    .map(|(namespace, _)| *namespace)
                    .collect::<NamespaceSet>();
                if available == NamespaceSet::EMPTY {
                    pending.push(id);
                    continue;
                }
                let previous = self.directives[id].state;
                let record = match previous {
                    ImportState::Unresolved => true,
                    ImportState::Partial(old) => old != available,
                    ImportState::Resolved => false,
                };
                if record {
                    let reference = self.directives[id].reference;
                    for (namespace, source) in bindings {
                        // `use Trait as _` adds method-lookup access without creating a name.
                        // Other definitions imported as `_` resolve but add no named binding.
                        if matches!(data.binding, ImportBinding::Hidden)
                            && !(namespace == Namespace::Types
                                && matches!(source.def,
                                DefId::Local(def) if env.local_def_kind(def)? == Some(LocalDefKind::Trait)))
                        {
                            continue;
                        }
                        if let Some(binding) = NameResolver::new(env).imported_binding(
                            module,
                            &source,
                            data,
                            NameBindingProvenance::NamedImport(reference),
                        )? {
                            if matches!(data.binding, ImportBinding::Hidden) {
                                let old = self.modules[owner]
                                    .scope
                                    .unnamed_trait_binding(binding.def)
                                    .copied();
                                let binding = self.merge(env, old, binding)?;
                                self.modules[owner].scope.set_unnamed_trait_binding(binding);
                                self.modules[owner].changed |= old != Some(binding);
                            } else if let Some(name) = data.binding_name() {
                                self.insert(
                                    env,
                                    owner,
                                    &name,
                                    namespace,
                                    binding,
                                    reference.import,
                                    0,
                                )?;
                                self.propagate(env)?;
                            }
                        }
                    }
                    progress = true;
                }
                // A full answer retires immediately. Otherwise keep the first namespace set
                // pending, record one later change to that set, and retire. This is the same
                // bounded partial-answer policy used by rust-analyzer, not a claim that every
                // missing namespace has been proved absent.
                self.directives[id].state = if available == NamespaceSet::ALL
                    || matches!(previous, ImportState::Partial(old) if old != available)
                {
                    ImportState::Resolved
                } else {
                    pending.push(id);
                    ImportState::Partial(available)
                };
            }
            // Existing partial answers can remain pending here. If no import found a first
            // answer, a new namespace set, or a glob source, another pass has nothing new to use.
            if !progress {
                break;
            }
        }
        // 5. Finish per-run counters and return the buffers. Directive state and glob links stay
        // here, but path lookup outside this call must read the scopes restored to the builders.
        let mut evaluated = self
            .modules
            .iter_mut()
            .filter_map(|module| mem::take(&mut module.evaluated).then_some(module.module))
            .collect::<Vec<_>>();
        let mut changed = self
            .modules
            .iter_mut()
            .filter_map(|module| mem::take(&mut module.changed).then_some(module.module))
            .collect::<Vec<_>>();
        evaluated.sort_unstable_by_key(Self::module_key);
        changed.sort_unstable_by_key(Self::module_key);
        let evaluated = ModuleSetShape::from_sorted_modules(&evaluated);
        let changed = ModuleSetShape::from_sorted_modules(&changed);
        let metrics = ImportResolutionMetrics {
            evaluated_packages: evaluated.packages,
            evaluated_crates: evaluated.crates,
            evaluated_modules: evaluated.modules,
            changed_packages: changed.packages,
            changed_crates: changed.crates,
            changed_modules: changed.modules,
            imports_evaluated: self.stats.imports_evaluated,
            glob_imports_evaluated: self.stats.glob_imports_evaluated,
            glob_bindings_emitted: self.stats.glob_bindings_emitted,
        };
        let mut modules = self
            .modules
            .iter_mut()
            .map(|module| ResolvedModuleImports {
                module: module.module,
                scope: mem::take(&mut module.scope),
                unresolved: Vec::new(),
            })
            .collect::<Vec<_>>();
        for directive in &self.directives {
            if matches!(directive.state, ImportState::Unresolved) {
                modules[directive.module]
                    .unresolved
                    .push(directive.reference.import);
            }
        }
        self.changes.shrink_to_fit();
        Ok(ImportCollectionOutput { modules, metrics })
    }

    fn module_key(module: &ModuleRef) -> (usize, usize, usize) {
        let krate = module.origin.origin_crate();
        (krate.package.0, krate.crate_id.0, module.module.0)
    }

    /// Apply ordinary precedence and combine visibility/attributes for routes to the same item.
    fn merge<E: ImportCollectionEnv>(
        &self,
        env: &E,
        old: Option<NameBinding>,
        incoming: NameBinding,
    ) -> Result<NameBinding, PackageStoreError> {
        old.map_or(Ok(incoming), |old| {
            old.select(incoming, |a, b| {
                NameResolver::new(env).broadest_visibility(a, b)
            })
        })
    }

    /// Select one import's answer for a slot, then enqueue any change for downstream globs.
    #[allow(clippy::too_many_arguments)]
    fn insert<E: ImportCollectionEnv>(
        &mut self,
        env: &E,
        owner: usize,
        name: &Name,
        namespace: Namespace,
        mut incoming: NameBinding,
        import: ImportId,
        depth: usize,
    ) -> anyhow::Result<()> {
        let module = self.modules[owner].module;
        // Proc-macro implementation functions remain usable locally, but their value identity
        // cannot become a public export just because it arrived through an import.
        if !env.allow_public_binding(module, namespace, &incoming) {
            incoming.restrict_public_visibility_to(module);
        }
        let scope = &self.modules[owner].scope;
        let old = scope.binding(name, namespace).copied();
        let old_import = scope.binding_import(name, namespace);
        // Normally an equal-priority conflict keeps the old definition. There is one exception:
        // if this import supplied the winning answer, let it refresh that answer. For example,
        // a glob's source can acquire a direct declaration that shadows its old imported name.
        let binding = match old {
            Some(old)
                if old.def != incoming.def
                    && old_import == Some(import)
                    && old.is_explicit() == incoming.is_explicit() =>
            {
                incoming
            }
            _ => self.merge(env, old, incoming)?,
        };
        if old == Some(binding) {
            return Ok(());
        }
        // Another route to the same definition can improve visibility or supply better display
        // attributes without becoming the route allowed to replace this target later. Preserve
        // that distinction separately from NameBinding::attribute_import.
        let winner = match old {
            Some(old) if old.def != binding.def || !old.is_explicit() && binding.is_explicit() => {
                Some(import)
            }
            None => Some(import),
            _ => old_import,
        };
        self.modules[owner]
            .scope
            .set_import_binding(name, namespace, binding, winner);
        self.modules[owner].changed = true;
        self.changes
            .push_back((module, name.clone(), namespace, depth));
        Ok(())
    }

    /// Seed a new glob, or refresh it after builders have added macro-generated declarations.
    fn scan_glob<E: ImportCollectionEnv>(&mut self, env: &E, glob: usize) -> anyhow::Result<()> {
        let directive = &self.directives[self.globs[glob].directive];
        let owner = directive.module;
        self.modules[owner].evaluated = true;
        let module = self.modules[owner].module;
        let reference = directive.reference;
        let data = env.import_data(reference);
        match self.globs[glob].source {
            GlobImportSource::Module(source) => {
                // Copy one source's bindings before mutating scopes: the destination can also
                // be the source, and insertion may change its storage. Do not retain snapshots
                // of every module just to scan this glob.
                let names = ImportQueryEnv { state: self, env }
                    .module_scope_entries(source)?
                    .into_iter()
                    .flat_map(|(name, entry)| {
                        Namespace::ALL.into_iter().filter_map(move |ns| {
                            entry
                                .binding(ns)
                                .copied()
                                .map(|binding| (name.clone(), ns, binding))
                        })
                    })
                    .collect::<Vec<_>>();
                for (name, namespace, source) in names {
                    if let Some(binding) = NameResolver::new(env).imported_binding(
                        module,
                        &source,
                        data,
                        NameBindingProvenance::GlobImport(reference),
                    )? {
                        self.stats.glob_bindings_emitted += 1;
                        self.insert(env, owner, &name, namespace, binding, reference.import, 0)?;
                    }
                }
            }
            GlobImportSource::Enum(def) => {
                // `use Enum::*` imports variant constructors rather than a module scope.
                for entry in env.local_enum_variant_entries_for_enum(def)? {
                    let source = NameBinding::new(
                        DefId::EnumVariant(entry.variant_ref),
                        entry.data.visibility,
                        NameBindingProvenance::Direct,
                    );
                    for namespace in entry.data.namespaces.iter() {
                        if let Some(binding) = NameResolver::new(env).imported_binding(
                            module,
                            &source,
                            data,
                            NameBindingProvenance::GlobImport(reference),
                        )? {
                            self.stats.glob_bindings_emitted += 1;
                            self.insert(
                                env,
                                owner,
                                &entry.data.name,
                                namespace,
                                binding,
                                reference.import,
                                0,
                            )?;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Forward changed slots through all registered globs that read their source module.
    ///
    /// With `b: use a::*` and `c: use b::*`, inserting `a::Item` updates `b::Item`, queues
    /// that changed slot, then updates `c::Item`. An unchanged destination queues nothing, so
    /// ordinary cyclic globs stop once they already contain the same selected answers.
    fn propagate<E: ImportCollectionEnv>(&mut self, env: &E) -> anyhow::Result<()> {
        let mut processed = 0usize;
        while let Some((source, name, namespace, depth)) = self.changes.pop_front() {
            if processed.is_multiple_of(1024) {
                env.check_cancelled()?;
            }
            processed += 1;
            if depth >= GLOB_PROPAGATION_LIMIT {
                metric::IMPORT_GLOB_LIMIT_HITS.inc();
                continue;
            }
            // Events carry the slot address, not a saved binding: an earlier event may already
            // have changed this name again. Forward whatever is selected now.
            let binding = ImportQueryEnv { state: self, env }
                .module_scope_entry(source, &name)?
                .and_then(|entry| entry.binding(namespace))
                .copied();
            let Some(binding) = binding else {
                continue;
            };
            let globs = self
                .globs_by_source
                .get(&source)
                .cloned()
                .unwrap_or_default();
            for glob in globs {
                let directive = &self.directives[self.globs[glob].directive];
                let owner = directive.module;
                let module = self.modules[owner].module;
                let reference = directive.reference;
                // Reapply visibility at each hop. `pub use` cannot expose a source that this
                // importing module cannot see, or widen a private source into a public binding.
                if let Some(binding) = NameResolver::new(env).imported_binding(
                    module,
                    &binding,
                    env.import_data(reference),
                    NameBindingProvenance::GlobImport(reference),
                )? {
                    self.stats.glob_bindings_emitted += 1;
                    self.modules[owner].evaluated = true;
                    self.insert(
                        env,
                        owner,
                        &name,
                        namespace,
                        binding,
                        reference.import,
                        depth + 1,
                    )?;
                }
            }
        }
        Ok(())
    }
}

/// Makes ordinary path lookup read the live scopes owned by the import collector.
///
/// Builders have empty placeholders while their buffers are lent out. Read those modules here;
/// delegate modules without imports, frozen dependencies, and all declaration metadata to `env`.
struct ImportQueryEnv<'a, E> {
    state: &'a ImportCollector,
    env: &'a E,
}
impl<E: ImportCollectionEnv> NameResolutionEnv for ImportQueryEnv<'_, E> {
    type Error = PackageStoreError;
    fn module_metadata(
        &self,
        module: ModuleRef,
    ) -> Result<Option<ModuleResolutionView<'_>>, Self::Error> {
        self.env.module_metadata(module)
    }
    fn module_scope_entry<'a>(
        &'a self,
        module: ModuleRef,
        name: &str,
    ) -> Result<Option<NameBindingsView<'a>>, Self::Error> {
        match self.state.module_indices.get(&module) {
            Some(&index) => Ok(self.state.modules[index].scope.entry(name)),
            None => self.env.module_scope_entry(module, name),
        }
    }
    fn module_scope_entries<'a>(
        &'a self,
        module: ModuleRef,
    ) -> Result<Vec<(&'a Name, NameBindingsView<'a>)>, Self::Error> {
        match self.state.module_indices.get(&module) {
            Some(&index) => Ok(self.state.modules[index].scope.entries().collect()),
            None => self.env.module_scope_entries(module),
        }
    }
    fn local_def_data(&self, def: LocalDefRef) -> Result<Option<&LocalDefData>, Self::Error> {
        self.env.local_def_data(def)
    }
    fn local_enum_variant_data(
        &self,
        def: LocalEnumVariantRef,
    ) -> Result<Option<&LocalEnumVariantData>, Self::Error> {
        self.env.local_enum_variant_data(def)
    }
    fn local_enum_variant_entries_for_enum<'a>(
        &'a self,
        def: LocalDefRef,
    ) -> Result<Vec<LocalEnumVariantEntry<'a>>, Self::Error> {
        self.env.local_enum_variant_entries_for_enum(def)
    }
}
impl<E: ImportCollectionEnv> CrateResolutionEnv for ImportQueryEnv<'_, E> {
    fn extern_root(&self, krate: CrateRef, name: &str) -> Result<Option<ModuleRef>, Self::Error> {
        self.env.extern_root(krate, name)
    }
    fn prelude_module(&self, krate: CrateRef) -> Result<Option<ModuleRef>, Self::Error> {
        self.env.prelude_module(krate)
    }
    fn root_module(&self, krate: CrateRef) -> Result<Option<ModuleRef>, Self::Error> {
        self.env.root_module(krate)
    }
}
