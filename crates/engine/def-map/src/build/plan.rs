//! Prepares crate dependencies and the matching external roots before construction starts.
//!
//! A dependency has both a name (`serde` in `serde::Serialize`) and a target crate. Cargo target
//! kinds decide which roots exist: build scripts see build dependencies, tests also see dev
//! dependencies, and ordinary targets can refer to their sibling library.
//!
//! Those same roots form the scheduling graph. Preparation removes cyclic edges from that graph
//! and from name lookup together, then returns crate inputs in dependency-first order.
//! Once a plan is returned, neither its order nor its visible roots need further normalization.

use std::{collections::HashMap, mem};

use rg_ir_model::{CrateId, CrateRef, DefMapRef, ModuleId, ModuleRef, PackageSlot};
use rg_text::Name;
use rg_workspace::{SysrootCrate, WorkspaceMetadata};

type CrateImplicitRoots = HashMap<Name, ModuleRef>;
type PackageCrateImplicitRoots = Vec<CrateImplicitRoots>;

/// One crate and its normalized roots, moved into declaration collection together.
pub(crate) struct CrateBuildInput {
    pub(crate) crate_ref: CrateRef,
    pub(crate) implicit_roots: CrateImplicitRoots,
}

/// Inputs for all crates in dependency-first order. Original crate slots stay unchanged.
pub(crate) struct CrateBuildPlan {
    pub(crate) crates: Vec<CrateBuildInput>,
}

impl CrateBuildPlan {
    pub(crate) fn prepare(workspace: &WorkspaceMetadata) -> Self {
        let mut roots = Self::implicit_roots(workspace);
        let mut visits = roots
            .iter()
            .map(|roots| vec![Visit::Pending; roots.len()])
            .collect::<Vec<_>>();
        let mut order = Vec::new();
        // A package cycle may still have a usable target order:
        // `a[test] -> b[lib] -> a[lib]`. Follow target roots rather than package edges.
        for package in 0..roots.len() {
            for crate_id in 0..roots[package].len() {
                Self::visit(
                    &mut roots,
                    CrateRef {
                        package: PackageSlot(package),
                        crate_id: CrateId(crate_id),
                    },
                    &mut visits,
                    &mut order,
                );
            }
        }
        let crates = order
            .into_iter()
            .map(|crate_ref| CrateBuildInput {
                crate_ref,
                implicit_roots: mem::take(&mut roots[crate_ref.package.0][crate_ref.crate_id.0]),
            })
            .collect();
        Self { crates }
    }

    fn visit(
        roots: &mut [PackageCrateImplicitRoots],
        krate: CrateRef,
        visits: &mut [Vec<Visit>],
        order: &mut Vec<CrateRef>,
    ) {
        if visits[krate.package.0][krate.crate_id.0] == Visit::Complete {
            return;
        }
        visits[krate.package.0][krate.crate_id.0] = Visit::Active;
        let mut dependencies = roots[krate.package.0][krate.crate_id.0]
            .iter()
            .map(|(name, module)| (name.clone(), module.origin.origin_crate()))
            .collect::<Vec<_>>();
        dependencies.sort_unstable_by(|(left_name, left), (right_name, right)| {
            (left.package.0, left.crate_id.0, left_name.as_str()).cmp(&(
                right.package.0,
                right.crate_id.0,
                right_name.as_str(),
            ))
        });
        for (name, dependency) in dependencies {
            if visits[dependency.package.0][dependency.crate_id.0] == Visit::Active {
                // Like rust-analyzer, keep a usable acyclic graph when metadata describes a cycle.
                // Removing the root as well as the scheduling edge prevents lookup from reading a
                // dependency that has not been built, or falling back to its stale saved DefMap.
                roots[krate.package.0][krate.crate_id.0].remove(&name);
                tracing::warn!(?krate, ?dependency, %name, "omitting cyclic DefMap dependency");
                continue;
            }
            Self::visit(roots, dependency, visits, order);
        }
        visits[krate.package.0][krate.crate_id.0] = Visit::Complete;
        order.push(krate);
    }

    /// Derive the names and target roots that can start a path outside a crate's module tree.
    fn implicit_roots(workspace: &WorkspaceMetadata) -> Vec<PackageCrateImplicitRoots> {
        let packages = workspace.packages();
        let lib_targets = packages
            .iter()
            .enumerate()
            .filter_map(|(package_slot, package)| {
                package
                    .analyzed_targets()
                    .enumerate()
                    .find(|(_, target)| target.kind.is_lib())
                    .map(|(index, target)| {
                        (
                            &package.id,
                            (
                                CrateRef {
                                    package: PackageSlot(package_slot),
                                    crate_id: CrateId(index),
                                },
                                target.name.as_str(),
                            ),
                        )
                    })
            })
            .collect::<HashMap<_, _>>();
        let proc_macro_root = workspace
            .sysroot_package(SysrootCrate::ProcMacro)
            .and_then(|package| lib_targets.get(&package.id))
            .map(|(crate_ref, _)| *crate_ref);
        let mut roots = Vec::with_capacity(packages.len());

        for package in packages {
            let mut package_roots = Vec::new();

            for (index, target) in package.analyzed_targets().enumerate() {
                let mut crate_roots = HashMap::new();

                // Cargo lets package targets refer to their sibling library by crate name, but build
                // scripts are separate crates and only see explicit build-dependencies.
                if let Some(&(lib_crate, lib_name)) = lib_targets.get(&package.id)
                    && lib_crate.crate_id != CrateId(index)
                    && !target.kind.is_custom_build()
                {
                    crate_roots.insert(
                        Name::new(lib_name),
                        ModuleRef {
                            origin: DefMapRef::Crate(lib_crate),
                            module: ModuleId(0),
                        },
                    );
                }

                // Rustc supplies `proc_macro` only to proc-macro targets. This is not a Cargo
                // dependency: a normal library or binary sibling in the same package must not see the
                // root just because another target exports procedural macros.
                if target.kind.is_proc_macro()
                    && let Some(proc_macro_root) = proc_macro_root
                {
                    crate_roots.insert(
                        Name::new(SysrootCrate::ProcMacro.name()),
                        ModuleRef {
                            origin: DefMapRef::Crate(proc_macro_root),
                            module: ModuleId(0),
                        },
                    );
                }

                for dependency in &package.dependencies {
                    if dependency.name().is_empty() || !dependency.applies_to_target(&target.kind) {
                        continue;
                    }

                    let Some(&(lib_crate, _)) = lib_targets.get(dependency.package_id()) else {
                        continue;
                    };

                    crate_roots.insert(
                        Name::new(dependency.name()),
                        ModuleRef {
                            origin: DefMapRef::Crate(lib_crate),
                            module: ModuleId(0),
                        },
                    );
                }

                package_roots.push(crate_roots);
            }

            roots.push(package_roots);
        }

        roots
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Visit {
    Pending,
    Active,
    Complete,
}
