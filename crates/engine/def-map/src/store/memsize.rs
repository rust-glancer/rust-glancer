use crate::{
    DefId, DefMap, DefMapDb, DefMapPackageBundle, DefMapStats, ImportBinding, ImportData, ImportId,
    ImportKind, ImportPath, ImportRef, ImportSourcePath, LocalDefData, LocalDefId, LocalDefKind,
    LocalDefRef, LocalImplData, LocalImplId, LocalImplRef, ModuleData, ModuleId, ModuleOrigin,
    ModuleRef, ModuleScope, Package, Path, PathSegment, ScopeBinding, ScopeEntry, TargetRef,
    model::{ImportSourcePathSegment, ScopeNameEntry},
};
use rg_memsize::{MemoryRecorder, MemorySize};

rg_memsize::impl_memory_size_leaf!(
    LocalDefKind,
    ImportKind,
    ModuleId,
    LocalDefId,
    LocalImplId,
    ImportId,
);

rg_memsize::impl_memory_size_children! {
    Package => name, target_names, targets;
    DefMap => root_module, extern_prelude, prelude, modules, local_defs, local_impls, imports;
    ModuleData => name, name_span, docs, parent, children, local_defs, impls, imports,
        unresolved_imports, scope, origin;
    LocalDefData => module, name, kind, visibility, source, file_id, name_span, span;
    LocalImplData => module, source, file_id, span;
    ModuleScope => entries;
    ScopeNameEntry => name, entry;
    ScopeEntry => types, values, macros;
    ScopeBinding => def, visibility, owner;
    ImportData => module, visibility, kind, path, source_path, binding, alias_span, source,
        import_index;
    ImportPath => absolute, segments;
    ImportSourcePath => source_span, absolute, segments;
    ImportSourcePathSegment => segment, span;
    Path => absolute, segments;
    TargetRef => package, target;
    ModuleRef => target, module;
    LocalDefRef => target, local_def;
    LocalImplRef => target, local_impl;
    ImportRef => target, import;
    DefMapStats => target_count, module_count, local_def_count, local_impl_count, import_count,
        unresolved_import_count;
}

impl MemorySize for DefMapDb {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        recorder.scope("packages", |recorder| {
            self.record_packages_memory_children(recorder);
        });
    }
}

impl MemorySize for DefMapPackageBundle {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        recorder.scope("package", |recorder| {
            self.package().record_memory_children(recorder);
        });
    }
}

impl MemorySize for ModuleOrigin {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            Self::Root { file_id } => file_id.record_memory_children(recorder),
            Self::Inline {
                declaration_file,
                declaration_span,
            } => {
                recorder.scope("declaration_file", |recorder| {
                    declaration_file.record_memory_children(recorder);
                });
                recorder.scope("declaration_span", |recorder| {
                    declaration_span.record_memory_children(recorder);
                });
            }
            Self::OutOfLine {
                declaration_file,
                declaration_span,
                definition_file,
            } => {
                recorder.scope("declaration_file", |recorder| {
                    declaration_file.record_memory_children(recorder);
                });
                recorder.scope("declaration_span", |recorder| {
                    declaration_span.record_memory_children(recorder);
                });
                recorder.scope("definition_file", |recorder| {
                    definition_file.record_memory_children(recorder);
                });
            }
        }
    }
}

impl MemorySize for ImportBinding {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            Self::Inferred | Self::Hidden => {}
            Self::Explicit(name) => name.record_memory_children(recorder),
        }
    }
}

impl MemorySize for PathSegment {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            Self::Name(name) => name.record_memory_children(recorder),
            Self::SelfKw | Self::SuperKw | Self::CrateKw => {}
        }
    }
}

impl MemorySize for DefId {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            Self::Module(module) => module.record_memory_children(recorder),
            Self::Local(local) => local.record_memory_children(recorder),
        }
    }
}
