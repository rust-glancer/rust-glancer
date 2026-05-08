// TODO: This file is unreasonably messy, to be cleaned up

use rg_memsize::{MemoryRecord, MemoryRecorder, MemorySize};
use rg_project::{BuildProfile, CacheProbeProfile, Project};

pub(super) const TOP_MEMORY_ROWS: usize = 12;

pub(super) fn print_project_summary(project: &Project) {
    let stats = project.stats();
    let residency = project.package_residency_plan();
    let resident_package_count = residency
        .packages()
        .iter()
        .filter(|decision| matches!(decision, rg_project::PackageResidency::Resident))
        .count();
    let offloaded_package_count = residency.packages().len() - resident_package_count;

    println!("rust-glancer analysis built");
    println!(
        "packages: {} ({} workspace)",
        stats.package_count, stats.workspace_package_count
    );
    println!(
        "package residency: {} ({resident_package_count} resident, {offloaded_package_count} offloaded)",
        residency.policy().config_name(),
    );
    println!(
        "def maps: {} targets, {} modules, {} unresolved imports",
        stats.def_map.target_count,
        stats.def_map.module_count,
        stats.def_map.unresolved_import_count
    );
    println!(
        "semantic IR: {} targets, {} type defs, {} traits, {} impls, {} functions",
        stats.semantic_ir.target_count,
        stats.semantic_ir.struct_count
            + stats.semantic_ir.enum_count
            + stats.semantic_ir.union_count,
        stats.semantic_ir.trait_count,
        stats.semantic_ir.impl_count,
        stats.semantic_ir.function_count
    );
    println!(
        "body IR: {} targets ({} built, {} skipped), {} bodies, {} expressions",
        stats.body_ir.target_count,
        stats.body_ir.built_target_count,
        stats.body_ir.skipped_target_count,
        stats.body_ir.body_count,
        stats.body_ir.expression_count
    );
}

pub(super) fn print_analysis_setup_profile(
    metadata_elapsed: std::time::Duration,
    workspace_elapsed: std::time::Duration,
    sysroot_elapsed: std::time::Duration,
) {
    let mut elapsed = std::time::Duration::default();

    println!();
    println!("analysis setup profile:");
    println!("  {:>10}  {:>10}  checkpoint", "phase", "elapsed");

    for (label, phase_elapsed) in [
        ("cargo metadata", metadata_elapsed),
        ("workspace metadata", workspace_elapsed),
        ("sysroot discovery", sysroot_elapsed),
    ] {
        elapsed += phase_elapsed;
        print_build_profile_timing_row(
            format_duration(phase_elapsed),
            format_duration(elapsed),
            label,
        );
    }
}

pub(super) fn print_build_profile(profile: &BuildProfile, purge: Option<&AllocatorPurgeReport>) {
    println!();
    println!("build profile:");
    let includes_memory = purge.is_some()
        || profile.checkpoints().iter().any(|checkpoint| {
            checkpoint.retained_bytes.is_some()
                || checkpoint.active_retained_bytes.is_some()
                || checkpoint.allocated_bytes.is_some()
                || checkpoint.active_bytes.is_some()
                || checkpoint.resident_bytes.is_some()
        });

    if includes_memory {
        println!(
            "  {:>10}  {:>10}  {:>12}  {:>12}  {:>12}  {:>12}  {:>12}  checkpoint",
            "phase", "elapsed", "rg_sampled", "rg_total", "j_allocated", "j_active", "j_resident"
        );
    } else {
        println!("  {:>10}  {:>10}  checkpoint", "phase", "elapsed");
    }

    for checkpoint in profile.checkpoints() {
        if includes_memory {
            print_build_profile_memory_row(
                format_duration(checkpoint.phase_elapsed),
                format_duration(checkpoint.elapsed),
                checkpoint.retained_bytes,
                checkpoint.active_retained_bytes,
                checkpoint.allocated_bytes,
                checkpoint.active_bytes,
                checkpoint.resident_bytes,
                checkpoint.label,
            );
        } else {
            print_build_profile_timing_row(
                format_duration(checkpoint.phase_elapsed),
                format_duration(checkpoint.elapsed),
                checkpoint.label,
            );
        }
    }

    if let Some(purge) = purge {
        let project_checkpoint = profile.checkpoints().last();
        print_build_profile_memory_row(
            "-".to_string(),
            "-".to_string(),
            project_checkpoint.and_then(|checkpoint| checkpoint.retained_bytes),
            project_checkpoint.and_then(|checkpoint| checkpoint.active_retained_bytes),
            purge.after.map(|stats| stats.allocated_bytes),
            purge.after.map(|stats| stats.active_bytes),
            purge.after.map(|stats| stats.resident_bytes),
            "after allocator purge",
        );
    }

    if let Some(cache_probe) = profile.cache_probe() {
        print_cache_probe_profile(cache_probe);
    }
}

fn print_cache_probe_profile(profile: &CacheProbeProfile) {
    println!();
    println!("cache probe:");
    println!(
        "  packages: {} total, {} resident, {} offloadable",
        profile.package_count, profile.resident_count, profile.offloadable_count,
    );
    println!(
        "  result: {} hits, {} misses",
        profile.hit_count,
        profile.miss_count(),
    );

    if profile.miss_count() > 0 {
        println!("  miss reasons:");
        for (label, count) in [
            ("missing artifact", profile.missing_artifact_count),
            ("artifact read error", profile.artifact_read_error_count),
            ("source mismatch", profile.source_mismatch_count),
            ("source fingerprint error", profile.source_error_count),
            (
                "body IR policy mismatch",
                profile.body_ir_policy_mismatch_count,
            ),
            ("parse restore error", profile.restore_error_count),
            ("unplanned package", profile.unplanned_package_count),
        ] {
            if count > 0 {
                println!("    {count:>6}  {label}");
            }
        }
    }

    println!("  timings:");
    println!(
        "    {:>10}  artifact read",
        format_duration(profile.artifact_read_elapsed),
    );
    println!(
        "    {:>10}  source fingerprint",
        format_duration(profile.source_fingerprint_elapsed),
    );
    println!(
        "    {:>10}  parse restore",
        format_duration(profile.parse_restore_elapsed),
    );
}

pub(super) fn print_allocator_stats(stats: rg_lsp::AllocatorStats) {
    println!(
        "allocator stats: allocated {}, active {}, resident {}, mapped {}, retained {}",
        format_bytes(stats.allocated_bytes),
        format_bytes(stats.active_bytes),
        format_bytes(stats.resident_bytes),
        format_bytes(stats.mapped_bytes),
        format_bytes(stats.retained_bytes),
    );
}

#[derive(Debug, Clone, Copy)]
pub(super) struct AllocatorPurgeReport {
    result: rg_lsp::AllocatorPurgeResult,
    before: Option<rg_lsp::AllocatorStats>,
    after: Option<rg_lsp::AllocatorStats>,
}

pub(super) fn purge_allocator_after_build(
    memory_control: &dyn rg_lsp::MemoryControl,
) -> Option<AllocatorPurgeReport> {
    let before_stats = memory_control.allocator_stats();
    let result = memory_control.try_purge_allocator()?;
    let after_stats = memory_control.allocator_stats();

    Some(AllocatorPurgeReport {
        result,
        before: before_stats,
        after: after_stats,
    })
}

pub(super) fn print_allocator_purge_after_build(purge: &AllocatorPurgeReport) {
    println!(
        "allocator purge after build: tcache_flushed {}, arenas_purged {}",
        purge.result.tcache_flushed, purge.result.arenas_purged,
    );

    if let (Some(before), Some(after)) = (purge.before, purge.after) {
        println!(
            "allocator purge stats: active {} -> {} ({}), resident {} -> {} ({}), mapped {} -> {} ({})",
            format_bytes(before.active_bytes),
            format_bytes(after.active_bytes),
            format_byte_delta(Some(after.active_bytes), Some(before.active_bytes)),
            format_bytes(before.resident_bytes),
            format_bytes(after.resident_bytes),
            format_byte_delta(Some(after.resident_bytes), Some(before.resident_bytes)),
            format_bytes(before.mapped_bytes),
            format_bytes(after.mapped_bytes),
            format_byte_delta(Some(after.mapped_bytes), Some(before.mapped_bytes)),
        );
    }
}

fn print_build_profile_timing_row(phase_elapsed: String, elapsed: String, label: &'static str) {
    println!("  {phase_elapsed:>10}  {elapsed:>10}  {label}");
}

#[allow(clippy::too_many_arguments)]
fn print_build_profile_memory_row(
    phase_elapsed: String,
    elapsed: String,
    retained_bytes: Option<usize>,
    active_retained_bytes: Option<usize>,
    allocated_bytes: Option<usize>,
    active_bytes: Option<usize>,
    resident_bytes: Option<usize>,
    label: &'static str,
) {
    println!(
        "  {:>10}  {:>10}  {:>12}  {:>12}  {:>12}  {:>12}  {:>12}  {}",
        phase_elapsed,
        elapsed,
        retained_bytes
            .map(format_bytes)
            .unwrap_or_else(|| "-".to_string()),
        active_retained_bytes
            .map(format_bytes)
            .unwrap_or_else(|| "-".to_string()),
        allocated_bytes
            .map(format_bytes)
            .unwrap_or_else(|| "-".to_string()),
        active_bytes
            .map(format_bytes)
            .unwrap_or_else(|| "-".to_string()),
        resident_bytes
            .map(format_bytes)
            .unwrap_or_else(|| "-".to_string()),
        label,
    );
}

pub(super) fn print_memory_summary(project: &Project) {
    // TODO: Make this summary recorder less allocation-heavy. Aggregate mode allocates path/type
    // keys for the human-readable report, so it can perturb allocator state after the measured
    // build profile even though it does not affect the reported post-purge row.
    let mut recorder = MemoryRecorder::new("project");
    project.record_memory_size(&mut recorder);
    let records = recorder.records();

    println!();
    println!(
        "memory: {} retained across {} aggregate buckets",
        format_bytes(recorder.total_bytes()),
        records.len()
    );

    print_memory_section("memory by phase", top_level_totals(&records), usize::MAX);
    print_memory_section("memory by kind", kind_totals(&records), usize::MAX);
    print_memory_section(
        "top memory paths",
        string_totals(
            records
                .iter()
                .map(|record| (record.path.as_str(), record.bytes)),
        ),
        TOP_MEMORY_ROWS,
    );
    print_memory_section(
        "top memory types",
        string_totals(
            records
                .iter()
                .map(|record| (record.type_name.as_str(), record.bytes)),
        ),
        TOP_MEMORY_ROWS,
    );
}

fn print_memory_section(title: &str, rows: Vec<(String, usize)>, limit: usize) {
    println!("{title}:");

    for (label, bytes) in rows.into_iter().take(limit) {
        println!("  {:>10}  {label}", format_bytes(bytes));
    }
}

fn top_level_totals(records: &[MemoryRecord]) -> Vec<(String, usize)> {
    string_totals(records.iter().map(|record| {
        let path = top_level_path(&record.path);
        (path, record.bytes)
    }))
}

fn top_level_path(path: &str) -> String {
    let mut parts = path.split('.');
    let Some(root) = parts.next() else {
        return path.to_string();
    };
    let Some(child) = parts.next() else {
        return root.to_string();
    };

    format!("{root}.{child}")
}

fn kind_totals(records: &[MemoryRecord]) -> Vec<(String, usize)> {
    string_totals(
        records
            .iter()
            .map(|record| (record.kind.as_str(), record.bytes)),
    )
}

fn string_totals<S>(items: impl IntoIterator<Item = (S, usize)>) -> Vec<(String, usize)>
where
    S: Into<String>,
{
    let mut totals = std::collections::BTreeMap::<String, usize>::new();
    for (label, bytes) in items {
        *totals.entry(label.into()).or_default() += bytes;
    }

    let mut rows = totals.into_iter().collect::<Vec<_>>();
    rows.sort_by(|(left_label, left_bytes), (right_label, right_bytes)| {
        right_bytes
            .cmp(left_bytes)
            .then_with(|| left_label.cmp(right_label))
    });
    rows
}

fn format_duration(duration: std::time::Duration) -> String {
    let millis = duration.as_secs_f64() * 1000.0;
    if millis < 1000.0 {
        format!("{millis:.0} ms")
    } else {
        format!("{:.2} s", duration.as_secs_f64())
    }
}

fn format_bytes(bytes: usize) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];

    let mut value = bytes as f64;
    let mut unit = UNITS[0];
    for next_unit in UNITS.iter().skip(1) {
        if value < 1024.0 {
            break;
        }
        value /= 1024.0;
        unit = next_unit;
    }

    if unit == "B" {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {unit}")
    }
}

fn format_byte_delta(after: Option<usize>, before: Option<usize>) -> String {
    let Some(after) = after.and_then(|value| i64::try_from(value).ok()) else {
        return "-".to_string();
    };
    let Some(before) = before.and_then(|value| i64::try_from(value).ok()) else {
        return "-".to_string();
    };
    let delta = after - before;
    let prefix = if delta >= 0 { "+" } else { "-" };
    let Some(bytes) = usize::try_from(delta.unsigned_abs()).ok() else {
        return format!("{delta} B");
    };
    format!("{prefix}{}", format_bytes(bytes))
}
