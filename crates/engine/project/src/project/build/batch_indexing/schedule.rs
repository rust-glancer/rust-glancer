//! Chooses dependency-safe package batches.

use rg_def_map::PackageSlot;
use rg_workspace::{SysrootCrate, WorkspaceMetadata};

use crate::{PackageBatchSize, project::package_set::PhasePackageSet};

/// Groups source packages for batch indexing.
///
/// A dependency is placed in the same batch or an earlier batch. Cargo can contain dependency
/// cycles involving development dependencies. When such a cycle prevents further progress, every
/// remaining package is put in one final batch, including ordinary dependents blocked by the cycle.
pub(super) struct PackageBatchSchedule {
    pub(super) batches: Vec<PhasePackageSet>,
    pub(super) cycle_blocked_package_count: usize,
}

impl PackageBatchSchedule {
    pub(super) fn build(
        workspace: &WorkspaceMetadata,
        source_packages: &PhasePackageSet,
        batch_size: PackageBatchSize,
    ) -> Self {
        let mut pending = vec![false; workspace.packages().len()];
        for package in source_packages.iter() {
            if let Some(is_pending) = pending.get_mut(package.0) {
                *is_pending = true;
            }
        }

        let mut batches = Vec::new();
        let mut cycle_blocked_package_count = 0;
        loop {
            let remaining_package_count = pending.iter().filter(|is_pending| **is_pending).count();
            if remaining_package_count == 0 {
                break;
            }
            let mut batch = Vec::with_capacity(batch_size.get().min(remaining_package_count));

            // A dependent may join the same batch after its dependency has been added because
            // DefMap resolves all packages in the batch together. Check dependencies again while
            // filling so a narrow dependency chain does not create mostly empty batches.
            while batch.len() < batch_size.get() {
                let ready = source_packages
                    .iter()
                    .filter(|package| {
                        pending.get(package.0).copied().unwrap_or(false)
                            && Self::dependencies_are_scheduled(workspace, &pending, *package)
                    })
                    .take(batch_size.get() - batch.len())
                    .collect::<Vec<_>>();
                if ready.is_empty() {
                    break;
                }

                for package in ready {
                    pending[package.0] = false;
                    batch.push(package);
                }
            }

            if !batch.is_empty() {
                batches.push(PhasePackageSet::from_packages(batch));
                continue;
            }

            // Building the complete unresolved remainder together keeps the cycle's crate roots
            // and any dependents waiting on them visible, matching the ordinary all-package DefMap
            // session. This final batch may therefore be larger than the configured size.
            let cycle_blocked = source_packages
                .iter()
                .filter(|package| pending.get(package.0).copied().unwrap_or(false))
                .collect::<Vec<_>>();
            cycle_blocked_package_count = cycle_blocked.len();
            batches.push(PhasePackageSet::from_packages(cycle_blocked));
            break;
        }

        Self {
            batches,
            cycle_blocked_package_count,
        }
    }

    pub(super) fn batch_count(&self) -> usize {
        self.batches.len()
    }

    pub(super) fn largest_batch_size(&self) -> usize {
        self.batches
            .iter()
            .map(|batch| batch.as_slice().len())
            .max()
            .unwrap_or(0)
    }

    fn dependencies_are_scheduled(
        workspace: &WorkspaceMetadata,
        pending: &[bool],
        package: PackageSlot,
    ) -> bool {
        let Some(metadata) = workspace.packages().get(package.0) else {
            return true;
        };
        let cargo_dependencies_scheduled = metadata.dependencies.iter().all(|dependency| {
            workspace
                .package_slot(dependency.package_id())
                .and_then(|dependency| pending.get(dependency.0))
                .is_none_or(|is_pending| !*is_pending)
        });
        if !cargo_dependencies_scheduled {
            return false;
        }

        // `proc_macro` is injected into proc-macro targets by DefMap rather than represented as a
        // Cargo edge. Include it here so the implicit dependency is assigned to the same batch or
        // an earlier one.
        if metadata
            .targets
            .iter()
            .any(|target| target.kind.is_proc_macro())
            && let Some(proc_macro) = workspace.sysroot_package(SysrootCrate::ProcMacro)
            && let Some(proc_macro) = workspace.package_slot(&proc_macro.id)
        {
            return pending
                .get(proc_macro.0)
                .is_none_or(|is_pending| !*is_pending);
        }

        true
    }
}

#[cfg(test)]
mod tests {
    use rg_def_map::PackageSlot;
    use rg_workspace::{WorkspaceLoweringConfig, WorkspaceMetadata};
    use test_fixture::fixture_crate;

    use crate::{PackageBatchSize, project::package_set::PhasePackageSet};

    use super::PackageBatchSchedule;

    #[test]
    fn caps_batch_reservation_to_the_source_package_count() {
        let fixture = fixture_crate(
            r#"
//- /Cargo.toml
[package]
name = "one_package"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct OnePackage;
"#,
        );
        let workspace =
            WorkspaceMetadata::for_tests(fixture.metadata(), WorkspaceLoweringConfig::default())
                .expect("fixture workspace metadata should normalize");
        let packages = PhasePackageSet::from_packages(vec![PackageSlot(0)]);

        let schedule = PackageBatchSchedule::build(
            &workspace,
            &packages,
            PackageBatchSize::new(usize::MAX)
                .expect("maximum usize should remain a positive batch size"),
        );

        assert_eq!(schedule.batches.len(), 1);
        assert_eq!(schedule.batches[0].as_slice(), [PackageSlot(0)]);
    }

    #[test]
    fn continues_filling_a_batch_after_adding_a_dependency() {
        let fixture = fixture_crate(
            r#"
//- /Cargo.toml
[workspace]
members = ["app", "middle", "leaf"]
resolver = "2"

//- /app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
middle = { path = "../middle" }

//- /app/src/lib.rs
pub fn app() -> middle::Middle { middle::Middle }

//- /middle/Cargo.toml
[package]
name = "middle"
version = "0.1.0"
edition = "2024"

[dependencies]
leaf = { path = "../leaf" }

//- /middle/src/lib.rs
pub struct Middle;

//- /leaf/Cargo.toml
[package]
name = "leaf"
version = "0.1.0"
edition = "2024"

//- /leaf/src/lib.rs
pub struct Leaf;
"#,
        );
        let workspace =
            WorkspaceMetadata::for_tests(fixture.metadata(), WorkspaceLoweringConfig::default())
                .expect("fixture workspace metadata should normalize");
        let packages = PhasePackageSet::from_packages(
            (0..workspace.packages().len()).map(PackageSlot).collect(),
        );

        let schedule = PackageBatchSchedule::build(
            &workspace,
            &packages,
            PackageBatchSize::new(2).expect("fixture batch size should be non-zero"),
        );

        assert_eq!(schedule.cycle_blocked_package_count, 0);
        assert_eq!(schedule.batches.len(), 2);
        assert_eq!(schedule.batches[0].as_slice().len(), 2);
        assert_eq!(schedule.batches[1].as_slice().len(), 1);

        let mut package_batches = vec![usize::MAX; workspace.packages().len()];
        for (batch_idx, batch) in schedule.batches.iter().enumerate() {
            for package in batch.iter() {
                package_batches[package.0] = batch_idx;
            }
        }
        for (package_idx, package) in workspace.packages().iter().enumerate() {
            for dependency in &package.dependencies {
                let dependency = workspace
                    .package_slot(dependency.package_id())
                    .expect("normalized dependency should have a package slot");
                assert!(
                    package_batches[dependency.0] <= package_batches[package_idx],
                    "dependencies should be built before or with their dependents",
                );
            }
        }
    }

    #[test]
    fn keeps_a_cycle_and_its_blocked_dependents_together() {
        let fixture = fixture_crate(
            r#"
//- /Cargo.toml
[workspace]
members = ["app", "a", "b"]
resolver = "2"

//- /app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
a = { path = "../a" }

//- /app/src/lib.rs
pub fn app() -> a::A { a::A }

//- /a/Cargo.toml
[package]
name = "a"
version = "0.1.0"
edition = "2024"

[dependencies]
b = { path = "../b" }

//- /a/src/lib.rs
pub struct A;

pub fn b() -> b::B { b::B }

//- /b/Cargo.toml
[package]
name = "b"
version = "0.1.0"
edition = "2024"

[dev-dependencies]
a = { path = "../a" }

//- /b/src/lib.rs
pub struct B;
"#,
        );
        let workspace =
            WorkspaceMetadata::for_tests(fixture.metadata(), WorkspaceLoweringConfig::default())
                .expect("fixture workspace metadata should normalize");
        let packages = PhasePackageSet::from_packages(
            (0..workspace.packages().len()).map(PackageSlot).collect(),
        );

        let schedule = PackageBatchSchedule::build(
            &workspace,
            &packages,
            PackageBatchSize::new(1).expect("fixture package batch size should be non-zero"),
        );

        assert_eq!(schedule.batches.len(), 1);
        assert_eq!(schedule.batches[0].as_slice().len(), 3);
        assert_eq!(schedule.cycle_blocked_package_count, 3);
    }
}
