//! Exercise the lifecycle of a small library API from declarations to a saved caller.

use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};

use anyhow::Context as _;
use gen_lsp_types::{Position, Range, TextEdit};
use serde_json::{Value, json};

use super::{
    ACTION_TIMEOUT, Action, Document, Expected, Observation, Request, Retry, Status, anchor_offset,
    position,
};
use crate::compare_lsp::{
    benchmark::fixture::{Checkout, Target},
    server::RunningServer,
};

const LEDGER: &str = "src/ledger.rs";
const TESTS: &str = "src/ledger/tests.rs";
const EMPTY_API: &str =
    include_str!("../../../../../../test_targets/bench_fixtures/lsp/library/src/ledger.rs");
pub(crate) struct LibraryEditor;

impl LibraryEditor {
    pub(crate) fn checkout(directory: &Path) -> anyhow::Result<Checkout> {
        Checkout::from_files(
            directory,
            &[
                (
                    "Cargo.toml",
                    include_str!(
                        "../../../../../../test_targets/bench_fixtures/lsp/library/Cargo.toml"
                    ),
                ),
                (
                    "Cargo.lock",
                    include_str!(
                        "../../../../../../test_targets/bench_fixtures/lsp/library/Cargo.lock"
                    ),
                ),
                (
                    "src/lib.rs",
                    include_str!(
                        "../../../../../../test_targets/bench_fixtures/lsp/library/src/lib.rs"
                    ),
                ),
                (LEDGER, EMPTY_API),
            ],
        )
    }

    pub(crate) async fn run(
        root: &Path,
        pair: u32,
        server: &mut RunningServer,
        observations: &mut Vec<Observation>,
    ) -> anyhow::Result<Instant> {
        let ledger = Document::load(root, LEDGER)?;
        ledger.open(server).await?;
        let ready = server.wait_until_ready().await?;
        Self::edit_api(root, pair, server, ledger, TESTS, "96", true, observations).await?;
        Ok(ready)
    }

    pub(crate) async fn run_in_project(
        root: &Path,
        target: &Target,
        pair: u32,
        server: &RunningServer,
        mut entry: Document,
        call: Position,
        observations: &mut Vec<Observation>,
    ) -> anyhow::Result<()> {
        let simulation = target
            .simulation
            .as_ref()
            .context("missing user simulation")?;
        let (definition, destination) = Document::load_at(root, &target.definition)?;
        let mut crate_root = (simulation.crate_root != target.entry.file)
            .then(|| Document::load(root, &simulation.crate_root))
            .transpose()?;
        let navigation = entry.request(
            Action::PROJECT_DEFINITION,
            call,
            Expected::Destination(&definition.uri, destination),
        );
        // The startup hover already covered the opening query. The hint request runs in
        // parallel while navigation determines whether the user can start editing.
        let origin = Instant::now();
        let mut edits = Vec::new();
        let (hints, (navigation, edit_result)) = tokio::join!(
            entry
                .hints(Action::PROJECT_HINTS, call)
                .at(server, origin, 90),
            async {
                let navigation = navigation.at(server, origin, 375).await;
                let result: anyhow::Result<()> = async {
                    if navigation.status != Status::Usable {
                        return Ok(());
                    }
                    if definition.path != entry.path {
                        definition.open(server).await?;
                        definition.close(server).await?;
                    }
                    // Reuse the open buffer when the entry file is the crate root.
                    let separate = crate_root.is_some();
                    let crate_root = if let Some(crate_root) = &mut crate_root {
                        crate_root.open(server).await?;
                        crate_root
                    } else {
                        &mut entry
                    };
                    Self::add_module(
                        root,
                        crate_root,
                        &simulation.project_input,
                        pair,
                        server,
                        &mut edits,
                    )
                    .await?;
                    if separate {
                        crate_root.close(server).await?;
                    }
                    Ok(())
                }
                .await;
                (navigation, result)
            },
        );
        observations.extend([hints, navigation]);
        observations.extend(edits);
        edit_result?;
        entry.close(server).await?;
        Ok(())
    }

    async fn add_module(
        root: &Path,
        crate_root: &mut Document,
        project_input: &str,
        pair: u32,
        server: &RunningServer,
        observations: &mut Vec<Observation>,
    ) -> anyhow::Result<()> {
        // Write the new module to disk before saving its declaration. Both servers see the
        // same real file and Cargo target as they learn about the new module.
        let source = crate_root
            .path
            .parent()
            .context("crate root has no parent")?;
        let api_path = source.join("lsp_bench_api.rs");
        let tests_path = source.join("lsp_bench_api/tests.rs");
        anyhow::ensure!(
            !api_path.exists() && !tests_path.exists(),
            "benchmark module already exists"
        );
        fs::write(&api_path, EMPTY_API).context("create API module")?;
        let api = Document::load(
            root,
            api_path
                .strip_prefix(root)?
                .to_str()
                .context("invalid API path")?,
        )?;
        let tests = tests_path
            .strip_prefix(root)?
            .to_str()
            .context("invalid tests path")?;
        api.open(server).await?;
        let offset = crate_root.text.len();
        Typing(u64::from(pair))
            .insert(crate_root, server, offset, "\nmod lsp_bench_api;\n")
            .await?;
        let saved_at = Instant::now();
        crate_root.save(server).await?;
        let declaration = anchor_offset(&crate_root.text, "mod lsp_bench_|api;")?;
        let new_module = crate_root.request(
            Action::NEW_MODULE,
            position(&crate_root.text[..declaration]),
            Expected::File(&api.uri),
        );
        observations.push(
            new_module
                .observe(server, saved_at, ACTION_TIMEOUT, Retry::UntilUsable)
                .await,
        );
        Self::edit_api(
            root,
            pair,
            server,
            api,
            tests,
            project_input,
            false,
            observations,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn edit_api(
        root: &Path,
        pair: u32,
        server: &RunningServer,
        mut ledger: Document,
        tests_path: &str,
        project_input: &str,
        auto_import: bool,
        observations: &mut Vec<Observation>,
    ) -> anyhow::Result<()> {
        let test_path = root.join(tests_path);
        fs::create_dir_all(
            test_path
                .parent()
                .context("test file has no parent directory")?,
        )?;
        let scaffold = include_str!(
            "../../../../../../test_targets/bench_fixtures/lsp/library/src/ledger/tests.txt"
        )
        .replace("PROJECT_INPUT", project_input);
        let scaffold = if auto_import {
            scaffold.replace("let limit = ItemLimit;", "let limit = ;")
        } else {
            format!(";\n{scaffold}")
        };
        fs::write(&test_path, scaffold).context("create library test scaffold")?;
        let mut tests = Document::load(root, tests_path)?;
        let mut added = ledger.text.clone();
        added.push_str(include_str!(
            "../../../../../../test_targets/bench_fixtures/lsp/library/src/ledger/items.txt"
        ));
        ledger.change(server, added).await?;
        ledger.save(server).await?;

        tests.open(server).await?;
        let mut typing = Typing(u64::from(pair));
        let prefix = "ItemL";
        let (action, start) = if auto_import {
            (
                Action::FIND_AUTO_IMPORT,
                anchor_offset(&tests.text, "let limit = |;")?,
            )
        } else {
            let start = anchor_offset(&tests.text, "|;\nuse super::{")?;
            let end = typing
                .insert(&mut tests, server, start, "use super::limits::")
                .await?;
            (Action::COMPLETE_IMPORT, end)
        };
        let end = typing.insert(&mut tests, server, start, prefix).await?;
        tests
            .complete(
                server,
                action,
                "ItemLimit",
                start..end,
                auto_import,
                observations,
            )
            .await?;

        let offset = anchor_offset(&tests.text, "let limit = ItemLimit|;")?;
        typing
            .insert(&mut tests, server, offset, "::new(project_items)")
            .await?;
        let signature = tests.request(
            Action::SIGNATURE,
            position(&tests.text[..anchor_offset(&tests.text, "ItemLimit::new(|project_items)")?]),
            Expected::Signature("new("),
        );
        observations.push(
            signature
                .observe(server, Instant::now(), ACTION_TIMEOUT, Retry::Transient)
                .await,
        );

        let start = anchor_offset(&tests.text, "let measured = batch.|;")?;
        let end = typing.insert(&mut tests, server, start, "it").await?;
        tests
            .complete(
                server,
                Action::COMPLETE_FIELD,
                "items",
                start..end,
                false,
                observations,
            )
            .await?;

        // The API is saved and the test remains open while the editor inspects its new code.
        let call = position(
            &tests.text[..anchor_offset(&tests.text, "let batch = ba|tch(limit.get());")?],
        );
        let fn_position =
            position(&ledger.text[..anchor_offset(&ledger.text, "pub fn |batch(items:")?]);
        let definition = tests.request(
            Action::BATCH_DEFINITION,
            call,
            Expected::Destination(&ledger.uri, fn_position),
        );
        observations.push(
            definition
                .observe(server, Instant::now(), ACTION_TIMEOUT, Retry::Transient)
                .await,
        );

        let binding = position(
            &tests.text[..anchor_offset(&tests.text, "let ba|tch = batch(limit.get());")?],
        );
        let type_position =
            position(&ledger.text[..anchor_offset(&ledger.text, "pub struct |Batch {")?]);
        let type_definition = tests.request(
            Action::BATCH_TYPE,
            binding,
            Expected::Destination(&ledger.uri, type_position),
        );
        let hints = tests.hints(Action::TEST_HINTS, binding);
        let use_site =
            position(&tests.text[..anchor_offset(&tests.text, "let size = ba|tch.size();")?]);
        let mut references = tests.request(
            Action::REFERENCES,
            binding,
            Expected::Destination(&tests.uri, use_site),
        );
        references.params["context"] = json!({ "includeDeclaration": false });
        let trigger = Instant::now();
        let (type_definition, hints, references) = tokio::join!(
            type_definition.observe(server, trigger, ACTION_TIMEOUT, Retry::Transient),
            hints.observe(server, trigger, ACTION_TIMEOUT, Retry::Transient),
            references.observe(server, trigger, ACTION_TIMEOUT, Retry::Transient),
        );
        observations.extend([type_definition, hints, references]);

        let implementation = ledger.request(
            Action::IMPLEMENTATION,
            position(&ledger.text[..anchor_offset(&ledger.text, "pub trait |Measure")?]),
            Expected::Destination(
                &ledger.uri,
                position(&ledger.text[..anchor_offset(&ledger.text, "impl Measure for |Batch")?]),
            ),
        );
        let symbols = Request {
            action: Action::OUTLINE,
            params: json!({ "textDocument": { "uri": ledger.uri } }),
            expected: Expected::Symbol("Batch"),
        };
        let trigger = Instant::now();
        let (implementation, symbols) = tokio::join!(
            implementation.observe(server, trigger, ACTION_TIMEOUT, Retry::Transient),
            symbols.observe(server, trigger, ACTION_TIMEOUT, Retry::Transient),
        );
        observations.extend([implementation, symbols]);
        tests.save(server).await
    }
}

impl Document {
    async fn complete(
        &mut self,
        server: &RunningServer,
        action: Action,
        label: &str,
        prefix: std::ops::Range<usize>,
        auto_import: bool,
        observations: &mut Vec<Observation>,
    ) -> anyhow::Result<()> {
        let mut completion = self
            .request(
                action,
                position(&self.text[..prefix.end]),
                Expected::Completion(label),
            )
            .observe(server, Instant::now(), ACTION_TIMEOUT, Retry::Transient)
            .await;
        let item = completion
            .attempts
            .last()
            .filter(|_| completion.status == Status::Usable)
            .and_then(|attempt| Expected::completion_item(&attempt.response, label));
        let accepted = if auto_import {
            let mut resolved = if let Some(item) = item {
                Request {
                    action: Action::RESOLVE_AUTO_IMPORT,
                    params: item.clone(),
                    expected: Expected::AutoImport(label),
                }
                .observe(server, Instant::now(), ACTION_TIMEOUT, Retry::Transient)
                .await
            } else {
                Observation::blocked(Action::RESOLVE_AUTO_IMPORT)
            };
            let accepted = if resolved.status == Status::Usable {
                let attempt = resolved
                    .attempts
                    .last()
                    .context("resolved completion response is missing")?;
                let accepted = self.accept(server, &attempt.response, prefix.clone()).await;
                if matches!(accepted, Ok(false)) {
                    resolved.status = Status::Unexpected;
                }
                accepted
            } else {
                Ok(false)
            };
            observations.extend([completion, resolved]);
            accepted
        } else {
            let accepted = if let Some(item) = item {
                let accepted = self.accept(server, item, prefix.clone()).await;
                if matches!(accepted, Ok(false)) {
                    completion.status = Status::Unexpected;
                }
                accepted
            } else {
                Ok(false)
            };
            observations.push(completion);
            accepted
        };

        if !accepted? {
            // Insert the intended text so a missing completion does not prevent the next
            // action. Its original outcome stays in the report.
            let mut manual = self.text.clone();
            manual.replace_range(prefix, label);
            if auto_import {
                manual.insert_str(0, &format!("use super::limits::{label};\n"));
            }
            self.change(server, manual).await?;
        }
        Ok(())
    }

    async fn accept(
        &mut self,
        server: &RunningServer,
        item: &Value,
        prefix: std::ops::Range<usize>,
    ) -> anyhow::Result<bool> {
        // Edits describe the same original snapshot. Apply them from the end of the document
        // and reject malformed or overlapping edits before changing the editor buffer.
        let mut edits = Vec::new();
        let main = &item["textEdit"];
        if main.is_null() {
            let Some(insert) = item["insertText"]
                .as_str()
                .or_else(|| item["label"].as_str())
            else {
                return Ok(false);
            };
            edits.push((prefix, insert.to_owned()));
        } else if let Ok(edit) = serde_json::from_value::<TextEdit>(main.clone())
            && let Some(range) = self.edit_range(edit.range)
        {
            edits.push((range, edit.new_text));
        } else {
            return Ok(false);
        }
        if let Some(additional) = item["additionalTextEdits"].as_array() {
            for edit in additional {
                let Ok(edit) = serde_json::from_value::<TextEdit>(edit.clone()) else {
                    return Ok(false);
                };
                let Some(range) = self.edit_range(edit.range) else {
                    return Ok(false);
                };
                edits.push((range, edit.new_text));
            }
        }

        edits.sort_by_key(|(range, _)| (range.start, range.end));
        let mut text = self.text.clone();
        let mut last_start = text.len();
        for (range, replacement) in edits.into_iter().rev() {
            if range.end > last_start {
                return Ok(false);
            }
            last_start = range.start;
            text.replace_range(range, &replacement);
        }
        self.change(server, text).await?;
        Ok(true)
    }

    fn edit_range(&self, range: Range) -> Option<std::ops::Range<usize>> {
        let offset = |position: Position| {
            let mut start = 0;
            let line = self.text.split('\n').nth(position.line as usize)?;
            for previous in self.text.split('\n').take(position.line as usize) {
                start += previous.len() + 1;
            }
            let mut remaining = position.character as usize;
            for (offset, ch) in line.char_indices() {
                if remaining == 0 {
                    return Some(start + offset);
                }
                remaining = remaining.checked_sub(ch.len_utf16())?;
            }
            (remaining == 0).then_some(start + line.len())
        };
        let start = offset(range.start)?;
        let end = offset(range.end)?;
        (start <= end).then_some(start..end)
    }
}

struct Typing(u64);

impl Typing {
    async fn insert(
        &mut self,
        document: &mut Document,
        server: &RunningServer,
        mut offset: usize,
        text: &str,
    ) -> anyhow::Result<usize> {
        for ch in text.chars() {
            // The mean interval is 171 ms, approximately 350 characters per minute.
            // Seeding per pair gives both servers the same deterministic jitter.
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1);
            tokio::time::sleep(Duration::from_millis(141 + (self.0 >> 32) % 61)).await;
            let mut next = document.text.clone();
            next.insert(offset, ch);
            document.change(server, next).await?;
            offset += ch.len_utf8();
        }
        Ok(offset)
    }
}
