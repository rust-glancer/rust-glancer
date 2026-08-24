//! Recovers cfg and environment instructions retained for one Cargo build-script unit.
//!
//! Cargo stores a build script's stdout in an `output` file beside its `out` directory. Dep-info
//! has already established which generated files belong to the candidate; this module only adds
//! the `cargo::rustc-cfg` and `cargo::rustc-env` values needed to interpret those files. Other
//! directives are irrelevant to source indexing and are ignored.

use std::{collections::BTreeMap, fs, path::Path};

use rg_cfg_eval::CfgOptions;
use rg_std::ExpectedUnique;

use super::CargoCompileEnvVar;

const MAX_BUILD_OUTPUT_BYTES: u64 = 256 * 1024;
const MAX_CFG_ENTRIES: usize = 512;
const MAX_COMPILE_ENV_VARS: usize = 128;

/// Bounded analysis-facing subset of one retained build-script output file.
#[derive(Default)]
pub(super) struct BuildScriptInstructions {
    pub(super) cfg_options: CfgOptions,
    compile_env: Vec<CargoCompileEnvVar>,
}

impl BuildScriptInstructions {
    /// Reads the sibling Cargo `output` file, falling back to empty enrichment on any mismatch.
    ///
    /// Build-output recovery is optional, so a missing, oversized, non-UTF-8, or unfamiliar
    /// file must not turn workspace loading into an error.
    pub(super) fn read(out_dir: &Path) -> Self {
        let Some(unit_dir) = out_dir.parent() else {
            return Self::default();
        };
        let output_path = unit_dir.join("output");
        let Ok(metadata) = fs::metadata(&output_path) else {
            return Self::default();
        };
        if metadata.len() > MAX_BUILD_OUTPUT_BYTES {
            return Self::default();
        }
        let Ok(contents) = fs::read(output_path) else {
            return Self::default();
        };

        Self::parse(&contents).unwrap_or_default()
    }

    fn parse(contents: &[u8]) -> Option<Self> {
        let contents = std::str::from_utf8(contents).ok()?;
        let mut recovered = Self::default();
        let mut cfg_entries = 0;
        let mut compile_env = BTreeMap::<String, ExpectedUnique<String>>::new();

        for instruction in contents
            .lines()
            .map(str::trim)
            .filter_map(BuildScriptInstruction::parse)
        {
            match instruction {
                BuildScriptInstruction::Cfg(cfg) => {
                    // Bound directive processing independently from the output-file size. One
                    // directive consumes one slot even when it repeats an already known cfg.
                    if cfg_entries >= MAX_CFG_ENTRIES {
                        continue;
                    }
                    cfg_entries += 1;

                    let parsed = CfgOptions::from_rustc_cfg_output(cfg);
                    for atom in parsed.atoms() {
                        recovered.cfg_options.insert_atom(atom);
                    }
                    for key_value in parsed.key_values() {
                        recovered
                            .cfg_options
                            .insert_key_value(key_value.key(), key_value.value());
                    }
                }
                BuildScriptInstruction::CompileEnv { name, value } => {
                    // Once the key limit is reached, keep observing admitted keys so a later
                    // conflicting value can still make one unusable. New names are ignored.
                    if compile_env.len() >= MAX_COMPILE_ENV_VARS && !compile_env.contains_key(name)
                    {
                        continue;
                    }
                    compile_env
                        .entry(name.to_string())
                        .or_default()
                        .push(value.to_string());
                }
            }
        }

        recovered.compile_env = compile_env
            .into_iter()
            .filter_map(|(name, value)| {
                Some(CargoCompileEnvVar {
                    name,
                    value: value.into_option()?,
                })
            })
            .collect();
        Some(recovered)
    }

    /// Returns stable compile-time environment with `OUT_DIR` anchored to the attributed unit.
    ///
    /// Cargo chooses `OUT_DIR` structurally rather than through a `rustc-env` instruction. Any
    /// printed value with that name is therefore replaced by the concrete directory found from
    /// dep-info.
    pub(super) fn compile_env_with_out_dir(mut self, out_dir: &Path) -> Vec<CargoCompileEnvVar> {
        self.compile_env.retain(|entry| entry.name != "OUT_DIR");
        self.compile_env.push(CargoCompileEnvVar {
            name: "OUT_DIR".to_string(),
            value: out_dir.to_string_lossy().into_owned(),
        });
        self.compile_env
            .sort_by(|left, right| left.name.cmp(&right.name));
        self.compile_env
    }
}

/// One analysis-relevant Cargo instruction after line-level syntax has been recognized.
enum BuildScriptInstruction<'a> {
    Cfg(&'a str),
    CompileEnv { name: &'a str, value: &'a str },
}

impl<'a> BuildScriptInstruction<'a> {
    fn parse(line: &'a str) -> Option<Self> {
        // Cargo accepts both the modern `cargo::key=value` spelling and its legacy single-colon
        // form. Normalize that transport detail before deciding whether analysis needs the key.
        let instruction = line
            .strip_prefix("cargo::")
            .or_else(|| line.strip_prefix("cargo:"))?;
        let (name, value) = instruction.split_once('=')?;

        match name {
            "rustc-cfg" => Some(Self::Cfg(value)),
            "rustc-env" => {
                let (name, value) = value.split_once('=')?;
                (!name.is_empty()).then_some(Self::CompileEnv { name, value })
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::BuildScriptInstructions;

    #[test]
    fn parses_both_instruction_spellings_and_drops_conflicts() {
        let output = BuildScriptInstructions::parse(
            b"cargo:rustc-cfg=generated\n\
              cargo::rustc-cfg=mode=\"fast\"\n\
              cargo:rustc-env=FILE=one.rs\n\
              cargo::rustc-env=SAME=value\n\
              cargo:rustc-env=SAME=value\n\
              cargo:rustc-env=FILE=two.rs\n\
              cargo:rustc-link-lib=ignored\n",
        )
        .expect("build output should parse");

        assert!(output.cfg_options.contains_atom("generated"));
        assert!(output.cfg_options.contains_key_value("mode", "fast"));
        assert_eq!(output.compile_env.len(), 1);
        assert_eq!(output.compile_env[0].name, "SAME");
        assert_eq!(output.compile_env[0].value, "value");
    }

    #[test]
    fn an_existing_key_can_still_be_invalidated_at_the_entry_limit() {
        let mut contents = String::new();
        for index in 0..super::MAX_COMPILE_ENV_VARS {
            contents.push_str(&format!("cargo:rustc-env=KEY_{index}=first\n"));
        }
        contents.push_str("cargo:rustc-env=KEY_0=conflicting\n");
        contents.push_str("cargo:rustc-env=BEYOND_LIMIT=ignored\n");

        let output = BuildScriptInstructions::parse(contents.as_bytes())
            .expect("bounded build output should parse");
        assert_eq!(output.compile_env.len(), super::MAX_COMPILE_ENV_VARS - 1);
        assert!(
            output
                .compile_env
                .iter()
                .all(|entry| entry.name != "KEY_0" && entry.name != "BEYOND_LIMIT")
        );
    }

    #[test]
    fn bounds_cfg_directives_independently_from_output_bytes() {
        let mut contents = String::new();
        for index in 0..=super::MAX_CFG_ENTRIES {
            contents.push_str(&format!("cargo:rustc-cfg=generated_{index}\n"));
        }

        let output = BuildScriptInstructions::parse(contents.as_bytes())
            .expect("bounded build output should parse");
        assert_eq!(output.cfg_options.atoms().len(), super::MAX_CFG_ENTRIES);
        assert!(
            !output
                .cfg_options
                .contains_atom(&format!("generated_{}", super::MAX_CFG_ENTRIES))
        );
    }
}
