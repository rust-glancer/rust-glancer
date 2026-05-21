//! Human-readable rendering for `analyze` reports.

use std::fmt::Write as _;

use super::report::{
    AllocatorPurgeReport, AnalyzeReport, BuildCheckpointReport, BuildProfileReport, format_bytes,
    format_duration_ms,
};

/// Controls which optional sections should be included in the text report.
pub(crate) struct TextRenderOptions {
    pub(crate) include_profile: bool,
    pub(crate) include_memory: bool,
}

impl AnalyzeReport {
    pub(crate) fn render_text(
        &self,
        options: TextRenderOptions,
        out: &mut String,
    ) -> std::fmt::Result {
        writeln!(out, "{}", self.project)?;

        if options.include_profile {
            writeln!(out)?;
            writeln!(out, "{}", self.analysis_setup)?;
        }

        if options.include_memory {
            if let Some(allocator) = &self.allocator {
                writeln!(out, "{allocator}")?;
            }
            if let Some(build_profile) = &self.build_profile {
                let purge = self
                    .allocator
                    .as_ref()
                    .and_then(|allocator| allocator.purge.as_ref());
                build_profile.render_to(purge, out)?;
            }
            if let Some(memory) = &self.memory {
                writeln!(out)?;
                writeln!(out, "{memory}")?;
            }
        } else if options.include_profile
            && let Some(build_profile) = &self.build_profile
        {
            build_profile.render_to(None, out)?;
        }

        if let Some(finalization_stats) = &self.def_map_finalization_stats {
            writeln!(out)?;
            writeln!(out, "{finalization_stats}")?;
        }

        Ok(())
    }
}

impl BuildProfileReport {
    fn render_to(
        &self,
        purge: Option<&AllocatorPurgeReport>,
        out: &mut String,
    ) -> std::fmt::Result {
        writeln!(out)?;
        writeln!(out, "build profile:")?;

        let includes_memory = purge.is_some()
            || self.checkpoints.iter().any(|checkpoint| {
                checkpoint.retained_bytes.is_some()
                    || checkpoint.active_retained_bytes.is_some()
                    || checkpoint.allocated_bytes.is_some()
                    || checkpoint.active_bytes.is_some()
                    || checkpoint.resident_bytes.is_some()
            });

        if includes_memory {
            writeln!(
                out,
                "  {:>10}  {:>10}  {:>12}  {:>12}  {:>12}  {:>12}  {:>12}  checkpoint",
                "phase",
                "elapsed",
                "rg_sampled",
                "rg_total",
                "j_allocated",
                "j_active",
                "j_resident",
            )?;
        } else {
            writeln!(out, "  {:>10}  {:>10}  checkpoint", "phase", "elapsed")?;
        }

        for checkpoint in &self.checkpoints {
            if includes_memory {
                render_build_profile_memory_row(checkpoint, out)?;
            } else {
                render_build_profile_timing_row(
                    out,
                    checkpoint.phase_elapsed_ms,
                    checkpoint.elapsed_ms,
                    &checkpoint.label,
                )?;
            }
        }

        if let Some(purge) = purge {
            let project_checkpoint = self.checkpoints.last();
            render_allocator_purge_build_row(project_checkpoint, purge, out)?;
        }

        if let Some(cache_probe) = &self.cache_probe {
            writeln!(out)?;
            writeln!(out, "{cache_probe}")?;
        }

        Ok(())
    }
}

fn render_build_profile_timing_row(
    out: &mut String,
    phase_elapsed_ms: f64,
    elapsed_ms: f64,
    label: &str,
) -> std::fmt::Result {
    writeln!(
        out,
        "  {:>10}  {:>10}  {label}",
        format_duration_ms(phase_elapsed_ms),
        format_duration_ms(elapsed_ms),
    )
}

fn render_build_profile_memory_row(
    checkpoint: &BuildCheckpointReport,
    out: &mut String,
) -> std::fmt::Result {
    render_build_profile_memory_values(
        out,
        format_duration_ms(checkpoint.phase_elapsed_ms),
        format_duration_ms(checkpoint.elapsed_ms),
        checkpoint.retained_bytes,
        checkpoint.active_retained_bytes,
        checkpoint.allocated_bytes,
        checkpoint.active_bytes,
        checkpoint.resident_bytes,
        &checkpoint.label,
    )
}

fn render_allocator_purge_build_row(
    project_checkpoint: Option<&BuildCheckpointReport>,
    purge: &AllocatorPurgeReport,
    out: &mut String,
) -> std::fmt::Result {
    render_build_profile_memory_values(
        out,
        "-".to_string(),
        "-".to_string(),
        project_checkpoint.and_then(|checkpoint| checkpoint.retained_bytes),
        project_checkpoint.and_then(|checkpoint| checkpoint.active_retained_bytes),
        purge.after.map(|stats| stats.allocated_bytes),
        purge.after.map(|stats| stats.active_bytes),
        purge.after.map(|stats| stats.resident_bytes),
        "after allocator purge",
    )
}

#[allow(clippy::too_many_arguments)]
fn render_build_profile_memory_values(
    out: &mut String,
    phase_elapsed: String,
    elapsed: String,
    retained_bytes: Option<usize>,
    active_retained_bytes: Option<usize>,
    allocated_bytes: Option<usize>,
    active_bytes: Option<usize>,
    resident_bytes: Option<usize>,
    label: &str,
) -> std::fmt::Result {
    writeln!(
        out,
        "  {:>10}  {:>10}  {:>12}  {:>12}  {:>12}  {:>12}  {:>12}  {}",
        phase_elapsed,
        elapsed,
        format_optional_bytes(retained_bytes),
        format_optional_bytes(active_retained_bytes),
        format_optional_bytes(allocated_bytes),
        format_optional_bytes(active_bytes),
        format_optional_bytes(resident_bytes),
        label,
    )
}

fn format_optional_bytes(bytes: Option<usize>) -> String {
    bytes.map(format_bytes).unwrap_or_else(|| "-".to_string())
}
