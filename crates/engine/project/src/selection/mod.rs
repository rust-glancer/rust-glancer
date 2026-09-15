//! Selects packages for work and for the dependency reads that work is allowed to make.
//!
//! A phase package set lists the packages being built or offloaded. A read subset can be larger:
//! rebuilding one package also needs access to its visible dependencies. Keep both selections
//! available to indexing and queries without treating them as residency decisions.

mod package_set;
pub(crate) mod subset;

pub(crate) use package_set::PhasePackageSet;
