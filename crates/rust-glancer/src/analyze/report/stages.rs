use std::{fmt, time::Duration};

use rg_project::{BuildProfile, CacheProbeProfile};
use serde::Serialize;

/// Timings collected before the project pipeline itself starts.
#[derive(Debug, Clone, Copy, Serialize)]
pub(crate) struct AnalysisSetupReport {
    pub(crate) cargo_metadata_ms: f64,
    pub(crate) workspace_metadata_ms: f64,
    pub(crate) sysroot_discovery_ms: f64,
    pub(crate) total_ms: f64,
}

impl AnalysisSetupReport {
    pub(crate) fn new(
        cargo_metadata: Duration,
        workspace_metadata: Duration,
        sysroot_discovery: Duration,
    ) -> Self {
        Self {
            cargo_metadata_ms: duration_ms(cargo_metadata),
            workspace_metadata_ms: duration_ms(workspace_metadata),
            sysroot_discovery_ms: duration_ms(sysroot_discovery),
            total_ms: duration_ms(cargo_metadata + workspace_metadata + sysroot_discovery),
        }
    }
}

impl fmt::Display for AnalysisSetupReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut elapsed_ms = 0.0;

        writeln!(f, "analysis setup profile:")?;
        writeln!(f, "  {:>10}  {:>10}  checkpoint", "phase", "elapsed")?;

        for (label, phase_elapsed_ms) in [
            ("cargo metadata", self.cargo_metadata_ms),
            ("workspace metadata", self.workspace_metadata_ms),
            ("sysroot discovery", self.sysroot_discovery_ms),
        ] {
            elapsed_ms += phase_elapsed_ms;
            writeln!(
                f,
                "  {:>10}  {:>10}  {label}",
                format_duration_ms(phase_elapsed_ms),
                format_duration_ms(elapsed_ms),
            )?;
        }

        Ok(())
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct BuildProfileReport {
    pub(crate) checkpoints: Vec<BuildCheckpointReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) cache_probe: Option<CacheProbeReport>,
}

impl BuildProfileReport {
    pub(crate) fn capture(profile: &BuildProfile) -> Self {
        Self {
            checkpoints: profile
                .checkpoints()
                .iter()
                .map(|checkpoint| BuildCheckpointReport {
                    label: checkpoint.label.to_string(),
                    phase_elapsed_ms: duration_ms(checkpoint.phase_elapsed),
                    elapsed_ms: duration_ms(checkpoint.elapsed),
                    retained_bytes: checkpoint.retained_bytes,
                    active_retained_bytes: checkpoint.active_retained_bytes,
                    allocated_bytes: checkpoint.allocated_bytes,
                    active_bytes: checkpoint.active_bytes,
                    resident_bytes: checkpoint.resident_bytes,
                    mapped_bytes: checkpoint.mapped_bytes,
                })
                .collect(),
            cache_probe: profile.cache_probe().map(CacheProbeReport::capture),
        }
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct BuildCheckpointReport {
    pub(crate) label: String,
    pub(crate) phase_elapsed_ms: f64,
    pub(crate) elapsed_ms: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) retained_bytes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) active_retained_bytes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) allocated_bytes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) active_bytes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) resident_bytes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) mapped_bytes: Option<usize>,
}

#[derive(Debug, Serialize)]
pub(crate) struct CacheProbeReport {
    pub(crate) package_count: usize,
    pub(crate) resident_count: usize,
    pub(crate) offloadable_count: usize,
    pub(crate) hit_count: usize,
    pub(crate) miss_count: usize,
    pub(crate) missing_artifact_count: usize,
    pub(crate) artifact_read_error_count: usize,
    pub(crate) source_mismatch_count: usize,
    pub(crate) source_error_count: usize,
    pub(crate) body_ir_policy_mismatch_count: usize,
    pub(crate) restore_error_count: usize,
    pub(crate) unplanned_package_count: usize,
    pub(crate) artifact_read_ms: f64,
    pub(crate) source_fingerprint_ms: f64,
    pub(crate) parse_restore_ms: f64,
}

impl CacheProbeReport {
    fn capture(profile: &CacheProbeProfile) -> Self {
        Self {
            package_count: profile.package_count,
            resident_count: profile.resident_count,
            offloadable_count: profile.offloadable_count,
            hit_count: profile.hit_count,
            miss_count: profile.miss_count(),
            missing_artifact_count: profile.missing_artifact_count,
            artifact_read_error_count: profile.artifact_read_error_count,
            source_mismatch_count: profile.source_mismatch_count,
            source_error_count: profile.source_error_count,
            body_ir_policy_mismatch_count: profile.body_ir_policy_mismatch_count,
            restore_error_count: profile.restore_error_count,
            unplanned_package_count: profile.unplanned_package_count,
            artifact_read_ms: duration_ms(profile.artifact_read_elapsed),
            source_fingerprint_ms: duration_ms(profile.source_fingerprint_elapsed),
            parse_restore_ms: duration_ms(profile.parse_restore_elapsed),
        }
    }
}

impl fmt::Display for CacheProbeReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "cache probe:")?;
        writeln!(
            f,
            "  packages: {} total, {} resident, {} offloadable",
            self.package_count, self.resident_count, self.offloadable_count,
        )?;
        writeln!(
            f,
            "  result: {} hits, {} misses",
            self.hit_count, self.miss_count,
        )?;

        if self.miss_count > 0 {
            writeln!(f, "  miss reasons:")?;
            for (label, count) in [
                ("missing artifact", self.missing_artifact_count),
                ("artifact read error", self.artifact_read_error_count),
                ("source mismatch", self.source_mismatch_count),
                ("source fingerprint error", self.source_error_count),
                (
                    "body IR policy mismatch",
                    self.body_ir_policy_mismatch_count,
                ),
                ("parse restore error", self.restore_error_count),
                ("unplanned package", self.unplanned_package_count),
            ] {
                if count > 0 {
                    writeln!(f, "    {count:>6}  {label}")?;
                }
            }
        }

        writeln!(f, "  timings:")?;
        writeln!(
            f,
            "    {:>10}  artifact read",
            format_duration_ms(self.artifact_read_ms),
        )?;
        writeln!(
            f,
            "    {:>10}  source fingerprint",
            format_duration_ms(self.source_fingerprint_ms),
        )?;
        write!(
            f,
            "    {:>10}  parse restore",
            format_duration_ms(self.parse_restore_ms),
        )
    }
}

pub(super) fn duration_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

pub(crate) fn format_duration_ms(ms: f64) -> String {
    if ms < 1000.0 {
        format!("{ms:.0} ms")
    } else {
        format!("{:.2} s", ms / 1000.0)
    }
}
