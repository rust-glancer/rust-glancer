use std::{collections::BTreeSet, fmt::Write as _};

use anyhow::Context as _;
use rg_profile::{ProfileFilter, ProfileRegistry, ProfileRun};

struct ProfileAlias {
    name: &'static str,
    description: &'static str,
    selectors: &'static [&'static str],
}

const PROFILE_ALIASES: &[ProfileAlias] = &[
    ProfileAlias {
        name: "default",
        description: "build checkpoints",
        selectors: &[rg_project::BUILD_CHECKPOINTS.scope()],
    },
    ProfileAlias {
        name: "macros",
        description: "def-map macro counters, timings, and by-name expansion tables",
        selectors: &["def_map.finalization", "def_map.macros.by_name"],
    },
    ProfileAlias {
        name: "memory:def-map",
        description: "build checkpoints plus the def-map memory snapshot",
        selectors: &[
            rg_project::BUILD_CHECKPOINTS.scope(),
            "project.build.def_map",
        ],
    },
];

pub(crate) fn profile_groups_help() -> String {
    let alias_name_width = PROFILE_ALIASES
        .iter()
        .map(|alias| alias.name.len())
        .max()
        .unwrap_or_default();

    let mut help = String::from("Profile aliases:\n");
    for alias in PROFILE_ALIASES {
        writeln!(
            help,
            "  {:alias_name_width$}  {}",
            alias.name, alias.description
        )
        .expect("writing to a string should not fail");
    }

    help.push_str("\nProfile selectors:\n");
    help.push_str("  all\n");
    for scope in profile_scopes() {
        writeln!(help, "  {scope}").expect("writing to a string should not fail");
    }

    help
}

pub(crate) fn parse_filter(
    filter: Option<&str>,
    include_memory: bool,
) -> anyhow::Result<Option<ProfileFilter>> {
    let mut filter = expand_filter(filter)?;

    if include_memory {
        filter
            .enable(rg_project::BUILD_CHECKPOINTS.scope())
            .context("while attempting to enable project build profiling for memory report")?;
    }

    Ok((!filter.is_disabled()).then_some(filter))
}

pub(crate) fn start_run(filter: ProfileFilter) -> anyhow::Result<ProfileRun> {
    let registry = ProfileRegistry::new(rg_project::profile_descriptors().iter().copied())
        .context("while attempting to build project profile registry")?;
    ProfileRun::start_with_registry(registry, filter)
        .context("while attempting to activate analyze profile run")
}

fn expand_filter(filter: Option<&str>) -> anyhow::Result<ProfileFilter> {
    let Some(filter) = filter.map(str::trim) else {
        return Ok(ProfileFilter::disabled());
    };
    if filter.is_empty() {
        return Ok(ProfileFilter::disabled());
    }

    let mut expanded = ProfileFilter::disabled();
    for selector in filter.split(',').map(str::trim) {
        if selector == "all" || selector == "*" {
            return Ok(ProfileFilter::all());
        }

        match profile_alias(selector) {
            Some(alias) => {
                for alias_selector in alias.selectors {
                    expanded.enable(alias_selector).with_context(|| {
                        format!(
                            "while attempting to enable `{}` profile alias selector `{alias_selector}`",
                            alias.name,
                        )
                    })?;
                }
            }
            None => expanded
                .enable(selector)
                .context("while attempting to parse analyze profile filter")?,
        }
    }

    Ok(expanded)
}

fn profile_alias(name: &str) -> Option<&'static ProfileAlias> {
    PROFILE_ALIASES.iter().find(|alias| alias.name == name)
}

fn profile_scopes() -> Vec<&'static str> {
    rg_project::profile_descriptors()
        .iter()
        .map(|descriptor| descriptor.scope())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}
