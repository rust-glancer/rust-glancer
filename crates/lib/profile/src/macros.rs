// Stable Rust cannot expose `macro_rules!` macros across crate boundaries through a plain `pub use`.
// Keep the required `#[macro_export]` names internal-looking and re-export the public names below.

#[doc(hidden)]
#[macro_export]
macro_rules! __rg_profile_increment_counter {
    ($path:literal) => {
        $crate::record_counter($path, 1)
    };
    ($path:literal, $amount:expr) => {
        $crate::record_counter($path, $amount)
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __rg_profile_increment_keyed_counter {
    ($path:literal, $key:expr) => {
        $crate::record_keyed_counter($path, $key, 1)
    };
    ($path:literal, $key:expr, $amount:expr) => {
        $crate::record_keyed_counter($path, $key, $amount)
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __rg_profile_record_duration {
    ($path:literal, $duration:expr) => {
        $crate::record_duration($path, $duration)
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __rg_profile_record_gauge {
    ($path:literal, $value:expr) => {
        $crate::record_gauge($path, $value)
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __rg_profile_record_keyed_duration {
    ($path:literal, $key:expr, $duration:expr) => {
        $crate::record_keyed_duration($path, $key, $duration)
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __rg_profile_timer {
    ($path:literal) => {
        $crate::timer($path)
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __rg_profile_checkpoint {
    ($path:literal, $label:expr $(, $key:literal => $value:expr)* $(,)?) => {{
        let values = ::std::vec![
            $($crate::ProfileCheckpointValue::new($key, $value),)*
        ];
        $crate::record_checkpoint($path, $label, values)
    }};
}

// Dear reader, this macro is 100% certified vibe coded. Do not try to understand it,
// just believe it works.
// For syntax description, see doc-comment on `declare_metrics` reexport below.
#[doc(hidden)]
#[macro_export]
macro_rules! __rg_profile_declare_metrics {
    (
        $vis:vis mod $module:ident {
            $(
                scope $scope:literal {
                    $(
                        $(#[doc = $doc:literal])*
                        $kind:ident $name:ident = $suffix:literal $( [$($option:tt)+] )? ;
                    )*
                }
            )*
        }
    ) => {
        $vis mod $module {
            $(
                $(
                    $crate::__rg_profile_declare_metrics!(
                        @declare [$($doc),*] $vis $kind $name $scope $suffix $(, $($option)+)?
                    );
                )*
            )*

            $vis static DESCRIPTORS: &[$crate::ProfileDescriptor] = &[
                $(
                    $(
                        $crate::__rg_profile_declare_metrics!(@descriptor [$($doc),*] $name),
                    )*
                )*
            ];

            $vis fn descriptors() -> &'static [$crate::ProfileDescriptor] {
                DESCRIPTORS
            }
        }
    };
    (@path $scope:literal $suffix:literal) => {
        concat!($scope, ".", $suffix)
    };
    (@descriptor [] $name:ident) => {
        $name.descriptor()
    };
    (@descriptor [$($doc:literal),+] $name:ident) => {
        $name.descriptor().description(concat!($($doc, "\n"),+))
    };
    (@options $metric:expr) => {
        $metric
    };
    (@options $metric:expr, title $title:literal $(, $($rest:tt)+)?) => {
        $crate::__rg_profile_declare_metrics!(
            @options $metric.title($title) $(, $($rest)+)?
        )
    };
    (@options $metric:expr, report $report:expr $(, $($rest:tt)+)?) => {
        $crate::__rg_profile_declare_metrics!(
            @options $metric.report($report) $(, $($rest)+)?
        )
    };
    (@options $metric:expr, columns $columns:expr $(, $($rest:tt)+)?) => {
        $crate::__rg_profile_declare_metrics!(
            @options $metric.columns($columns) $(, $($rest)+)?
        )
    };
    (@declare [$($doc:literal),*] $vis:vis counter $name:ident $scope:literal $suffix:literal $(, $($option:tt)+)?) => {
        $(#[doc = $doc])*
        $vis const $name: $crate::CounterMetric =
            $crate::__rg_profile_declare_metrics!(
                @options
                $crate::CounterMetric::new($crate::__rg_profile_declare_metrics!(@path $scope $suffix), $scope)
                $(, $($option)+)?
            );
    };
    (@declare [$($doc:literal),*] $vis:vis gauge $name:ident $scope:literal $suffix:literal, $unit:ident $(, $($option:tt)+)?) => {
        $(#[doc = $doc])*
        $vis const $name: $crate::GaugeMetric =
            $crate::__rg_profile_declare_metrics!(
                @options
                $crate::GaugeMetric::new(
                    $crate::__rg_profile_declare_metrics!(@path $scope $suffix),
                    $scope,
                    $crate::ProfileUnit::$unit,
                )
                $(, $($option)+)?
            );
    };
    (@declare [$($doc:literal),*] $vis:vis duration $name:ident $scope:literal $suffix:literal $(, $($option:tt)+)?) => {
        $(#[doc = $doc])*
        $vis const $name: $crate::DurationMetric =
            $crate::__rg_profile_declare_metrics!(
                @options
                $crate::DurationMetric::new($crate::__rg_profile_declare_metrics!(@path $scope $suffix), $scope)
                $(, $($option)+)?
            );
    };
    (@declare [$($doc:literal),*] $vis:vis keyed_counter $name:ident $scope:literal $suffix:literal $(, $($option:tt)+)?) => {
        $(#[doc = $doc])*
        $vis const $name: $crate::KeyedCounterMetric =
            $crate::__rg_profile_declare_metrics!(
                @options
                $crate::KeyedCounterMetric::new($crate::__rg_profile_declare_metrics!(@path $scope $suffix), $scope)
                $(, $($option)+)?
            );
    };
    (@declare [$($doc:literal),*] $vis:vis keyed_duration $name:ident $scope:literal $suffix:literal $(, $($option:tt)+)?) => {
        $(#[doc = $doc])*
        $vis const $name: $crate::KeyedDurationMetric =
            $crate::__rg_profile_declare_metrics!(
                @options
                $crate::KeyedDurationMetric::new($crate::__rg_profile_declare_metrics!(@path $scope $suffix), $scope)
                $(, $($option)+)?
            );
    };
    (@declare [$($doc:literal),*] $vis:vis checkpoint $name:ident $scope:literal $suffix:literal $(, $($option:tt)+)?) => {
        $(#[doc = $doc])*
        $vis const $name: $crate::CheckpointMetric =
            $crate::__rg_profile_declare_metrics!(
                @options
                $crate::CheckpointMetric::new($crate::__rg_profile_declare_metrics!(@path $scope $suffix), $scope)
                $(, $($option)+)?
            );
    };
    (@declare [$($doc:literal),*] $vis:vis memory_snapshot $name:ident $scope:literal $suffix:literal $(, $($option:tt)+)?) => {
        $(#[doc = $doc])*
        $vis const $name: $crate::MemorySnapshotMetric =
            $crate::__rg_profile_declare_metrics!(
                @options
                $crate::MemorySnapshotMetric::new($crate::__rg_profile_declare_metrics!(@path $scope $suffix), $scope)
                $(, $($option)+)?
            );
    };
}

/// Increments a registered counter by one, or by the provided amount.
pub use crate::__rg_profile_increment_counter as increment_counter;

/// Records a keyed counter increment.
pub use crate::__rg_profile_increment_keyed_counter as increment_keyed_counter;

/// Adds elapsed time to a registered duration.
pub use crate::__rg_profile_record_duration as record_duration;

/// Records the latest value for a registered gauge.
pub use crate::__rg_profile_record_gauge as record_gauge;

/// Adds elapsed time to a keyed duration aggregate.
pub use crate::__rg_profile_record_keyed_duration as record_keyed_duration;

/// Starts an RAII timer that records elapsed time when dropped.
pub use crate::__rg_profile_timer as timer;

/// Appends a row to a checkpoint stream.
pub use crate::__rg_profile_checkpoint as checkpoint;

/// Declares typed metric handles and a matching descriptor list for one module.
///
/// The macro groups profile items by selector scope. Each item path is built by appending the
/// item suffix to its scope with a dot, so `scope "def_map.macros"` plus
/// `counter CALLS = "calls"` declares the path `def_map.macros.calls`.
///
/// Doc comments on individual metrics are copied to the generated metric constants and are also
/// stored as descriptor descriptions for report renderers.
///
/// The generated module contains one typed metric constant per item, a `DESCRIPTORS` slice, and a
/// `descriptors()` function. The visibility on the module is reused for those generated items.
///
/// Syntax overview:
///
/// ```text
/// rg_profile::declare_metrics! {
///     pub(crate) mod metric {
///         scope "scope.path" {
///             /// Optional description carried into profile reports.
///             counter NAME = "suffix";
///             gauge NAME = "suffix" [Unit];
///             duration NAME = "suffix";
///             keyed_counter NAME = "suffix";
///             keyed_counter NAME = "suffix" [report REPORT_EXPR, title "Report title"];
///             keyed_duration NAME = "suffix";
///             keyed_duration NAME = "suffix" [report REPORT_EXPR, title "Report title"];
///             checkpoint NAME = "suffix";
///             checkpoint NAME = "suffix" [columns COLUMNS_EXPR, title "Report title"];
///             memory_snapshot NAME = "suffix";
///             memory_snapshot NAME = "suffix" [title "Report title"];
///         }
///     }
/// }
/// ```
///
/// Gauge units are written as [`crate::ProfileUnit`] variant names without the `ProfileUnit::`
/// prefix, such as `Count`, `Bytes`, `Duration`, `Percent`, or `None`. Options after the unit can
/// be combined with commas, for example `[Count, title "Pending calls"]`.
///
/// Report and checkpoint-column expressions are evaluated inside the generated module, so constants
/// declared next to the macro usually need a `super::` prefix.
///
/// ```
/// const BY_COUNT: rg_profile::ProfileReport = rg_profile::ProfileReport {
///     sort: Some(rg_profile::ProfileReportSort::CountDescending),
///     limit: Some(20),
/// };
///
/// static CHECKPOINT_COLUMNS: &[rg_profile::ProfileCheckpointColumn] = &[
///     rg_profile::ProfileCheckpointColumn::bytes("retained_bytes", "retained"),
///     rg_profile::ProfileCheckpointColumn::count("packages", "packages"),
/// ];
///
/// rg_profile::declare_metrics! {
///     pub(crate) mod metric {
///         scope "def_map.finalization" {
///             counter ROUNDS = "rounds";
///             gauge EXPANSION_PASS_LIMIT = "expansion_pass_limit" [Count];
///             duration RESOLVE_IMPORT_SCOPES = "timings.resolve_import_scopes";
///         }
///
///         scope "def_map.macros.by_name" {
///             keyed_counter UNRESOLVED_BY_NAME = "unresolved" [report super::BY_COUNT, title "Unresolved macros"];
///             keyed_duration EXPANSION_BY_NAME = "expansion" [title "Slowest macros to expand"];
///         }
///
///         scope "project.build" {
///             checkpoint CHECKPOINTS = "checkpoints" [columns super::CHECKPOINT_COLUMNS, title "Build checkpoints"];
///         }
///
///         scope "project.build.def_map" {
///             memory_snapshot DEF_MAP_MEMORY = "memory" [title "after def-map"];
///         }
///     }
/// }
///
/// fn main() {
///     let _descriptors = metric::descriptors();
///
///     metric::ROUNDS.inc();
///     metric::EXPANSION_PASS_LIMIT.record_count(128);
///
///     let timer = metric::RESOLVE_IMPORT_SCOPES.start_timer();
///     timer.finish();
///
///     metric::UNRESOLVED_BY_NAME.inc("make_item");
/// }
/// ```
pub use crate::__rg_profile_declare_metrics as declare_metrics;
