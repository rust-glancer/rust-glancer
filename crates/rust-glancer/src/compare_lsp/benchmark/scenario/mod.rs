//! Replay a small editor workflow and retain the observations used to calculate its timings.

mod action;
mod library;

use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::Context as _;
use gen_lsp_types::{Position, Range};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use self::action::Action;
pub(super) use self::library::LibraryEditor;
use super::{
    Workload,
    fixture::{Site, Target, anchor_offset, position},
};
use crate::compare_lsp::{
    lsp_client::RequestOutcome,
    server::{RunningServer, file_uri},
};

const ACTION_TIMEOUT: Duration = Duration::from_secs(10);
const RETRY_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Status {
    Usable,
    Missing,
    Unexpected,
    RpcError,
    ContentModified,
    ServerCancelled,
    Superseded,
    Timeout,
    Disconnected,
    Blocked,
}

impl Status {
    pub(super) fn is_problem(self) -> bool {
        !matches!(self, Self::Usable | Self::Superseded)
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Usable => "Usable",
            Self::Missing => "Missing",
            Self::Unexpected => "Unexpected",
            Self::RpcError => "RPC error",
            Self::ContentModified => "Content modified",
            Self::ServerCancelled => "Server cancelled",
            Self::Superseded => "Superseded by typing",
            Self::Timeout => "Timeout",
            Self::Disconnected => "Disconnected",
            Self::Blocked => "Blocked",
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct Attempt {
    pub(super) sent_ms: f64,
    pub(super) received_ms: f64,
    pub(super) status: Status,
    response: Value,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct Observation {
    pub(super) action: String,
    pub(super) method: String,
    pub(super) foreground: bool,
    pub(super) started_ms: Option<f64>,
    pub(super) ended_ms: Option<f64>,
    pub(super) status: Status,
    params: Option<Value>,
    pub(super) attempts: Vec<Attempt>,
}

impl Observation {
    pub(super) fn latency(&self) -> Option<f64> {
        Some(self.ended_ms? - self.started_ms?)
    }

    pub(super) fn final_rtt(&self) -> Option<f64> {
        let attempt = self.attempts.last()?;
        (!matches!(attempt.status, Status::Timeout | Status::Disconnected))
            .then_some(attempt.received_ms - attempt.sent_ms)
    }

    pub(super) fn result_items(&self) -> Option<usize> {
        let result = &self.attempts.last()?.response;
        match self.method.as_str() {
            "textDocument/inlayHint" => Some(result.as_array()?.len()),
            "textDocument/semanticTokens/range" => Some(result["data"].as_array()?.len() / 5),
            "textDocument/completion" => Some(
                result
                    .as_array()
                    .or_else(|| result["items"].as_array())?
                    .len(),
            ),
            _ => None,
        }
    }

    pub(super) fn complete_plan(workload: Workload, observations: &mut Vec<Self>) {
        let recorded: std::collections::BTreeSet<_> = observations
            .iter()
            .map(|answer| answer.action.as_str())
            .collect();
        let blocked: Vec<_> = Action::planned(workload)
            .filter(|action| !recorded.contains(action.label))
            .map(Self::blocked)
            .collect();
        observations.extend(blocked);
    }

    fn blocked(action: Action) -> Self {
        Self {
            action: action.label.into(),
            method: action.method.into(),
            foreground: action.foreground,
            started_ms: None,
            ended_ms: None,
            status: Status::Blocked,
            params: None,
            attempts: Vec::new(),
        }
    }
}

enum Expected<'a> {
    Hints,
    Type(&'a str),
    File(&'a str),
    Destination(&'a str, Position),
    Completion(&'a str),
    AutoImport(&'a str),
    Signature(&'a str),
    Symbol(&'a str),
}

impl Expected<'_> {
    fn completion_item<'a>(response: &'a Value, label: &str) -> Option<&'a Value> {
        response
            .as_array()
            .or_else(|| response["items"].as_array())?
            .iter()
            .find(|item| {
                item["label"].as_str().is_some_and(|item| {
                    item == label
                        || item
                            .strip_prefix(label)
                            .is_some_and(|suffix| suffix.starts_with('('))
                })
            })
    }

    fn status(&self, response: &Value) -> Status {
        if response.is_null() || response.as_array().is_some_and(Vec::is_empty) {
            return Status::Missing;
        }
        let matches = match self {
            Self::Hints => response.as_array().is_some_and(|hints| !hints.is_empty()),
            Self::Type(expected) => {
                // Match a signature line before the documentation separator. The hover
                // payload can be Markdown or plain text, depending on client capabilities.
                let contents = &response["contents"];
                let text = contents
                    .as_str()
                    .or_else(|| contents["value"].as_str())
                    .unwrap_or("");
                text.lines()
                    .take_while(|line| !line.starts_with("---"))
                    .any(|line| {
                        line.split_once(" -> ")
                            .is_some_and(|(_, ty)| ty.trim_end().trim_end_matches('`') == *expected)
                    })
            }
            Self::File(uri) | Self::Destination(uri, _) => {
                let locations = response
                    .as_array()
                    .map_or_else(|| std::slice::from_ref(response), Vec::as_slice);
                locations.iter().any(|location| {
                    let (target, range) = if let Some(target) = location.get("targetUri") {
                        (target, &location["targetSelectionRange"])
                    } else {
                        (&location["uri"], &location["range"])
                    };
                    if target.as_str() != Some(*uri) {
                        return false;
                    }
                    let Ok(range) = serde_json::from_value::<Range>(range.clone()) else {
                        return false;
                    };
                    match self {
                        Self::Destination(_, position) => {
                            let key = |position: Position| (position.line, position.character);
                            key(range.start) <= key(*position) && key(*position) < key(range.end)
                        }
                        _ => true,
                    }
                })
            }
            Self::Completion(label) => Self::completion_item(response, label).is_some(),
            Self::AutoImport(label) => {
                response["additionalTextEdits"]
                    .as_array()
                    .is_some_and(|edits| {
                        edits.iter().any(|edit| {
                            edit["newText"]
                                .as_str()
                                .is_some_and(|text| text.contains(label))
                        })
                    })
            }
            Self::Signature(name) => response["signatures"].as_array().is_some_and(|signatures| {
                signatures.iter().any(|signature| {
                    signature["label"]
                        .as_str()
                        .is_some_and(|label| label.contains(name))
                })
            }),
            Self::Symbol(name) => response.as_array().is_some_and(|symbols| {
                symbols
                    .iter()
                    .any(|symbol| symbol["name"].as_str() == Some(*name))
            }),
        };
        if matches {
            Status::Usable
        } else {
            Status::Unexpected
        }
    }
}

#[derive(Clone, Copy)]
enum Retry {
    Transient,
    UntilUsable,
}

struct Request<'a> {
    action: Action,
    params: Value,
    expected: Expected<'a>,
}

impl Request<'_> {
    async fn observe(
        self,
        server: &RunningServer,
        trigger: Instant,
        timeout: Duration,
        retry: Retry,
    ) -> Observation {
        let spawned = server.spawned_at();
        let deadline = trigger + timeout;
        let mut attempts = Vec::new();
        let status = loop {
            let sent = Instant::now();
            let (received, result) = server
                .observe_request(
                    self.action.method,
                    self.params.clone(),
                    deadline.saturating_duration_since(sent),
                )
                .await;
            let (status, response) = match result {
                RequestOutcome::Success(response) => (self.expected.status(&response), response),
                RequestOutcome::Error(error) => {
                    let status = match error.code {
                        -32801 => Status::ContentModified,
                        -32802 => Status::ServerCancelled,
                        _ => Status::RpcError,
                    };
                    (
                        status,
                        json!({
                            "code": error.code, "message": error.message, "data": error.data,
                        }),
                    )
                }
                RequestOutcome::Timeout => (Status::Timeout, Value::Null),
                RequestOutcome::TransportFailure { message } => {
                    (Status::Disconnected, json!({ "message": message }))
                }
            };
            attempts.push(Attempt {
                sent_ms: milliseconds(sent.duration_since(spawned)),
                received_ms: milliseconds(received.duration_since(spawned)),
                status,
                response,
            });

            let retryable = match retry {
                Retry::Transient => {
                    matches!(status, Status::ContentModified | Status::ServerCancelled)
                }
                Retry::UntilUsable => !matches!(
                    status,
                    Status::Usable | Status::Timeout | Status::Disconnected
                ),
            };
            let retry_at = Instant::now() + RETRY_INTERVAL;
            if !retryable || retry_at >= deadline {
                break status;
            }
            tokio::time::sleep_until(retry_at.into()).await;
        };
        Observation {
            action: self.action.label.into(),
            method: self.action.method.into(),
            foreground: self.action.foreground,
            started_ms: Some(milliseconds(trigger.duration_since(spawned))),
            ended_ms: attempts.last().map(|attempt| attempt.received_ms),
            status,
            params: Some(self.params),
            attempts,
        }
    }

    async fn at(self, server: &RunningServer, origin: Instant, offset_ms: u64) -> Observation {
        let trigger = origin + Duration::from_millis(offset_ms);
        tokio::time::sleep_until(trigger.into()).await;
        self.observe(server, trigger, ACTION_TIMEOUT, Retry::Transient)
            .await
    }
}

pub(super) struct Document {
    path: PathBuf,
    uri: String,
    text: String,
    version: u32,
}

impl Document {
    pub(super) fn load(root: &Path, file: &str) -> anyhow::Result<Self> {
        let path = root.join(file);
        let text = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        let uri = file_uri(&path)?.to_string();
        Ok(Self {
            path,
            uri,
            text,
            version: 1,
        })
    }

    pub(super) fn load_at(root: &Path, site: &Site) -> anyhow::Result<(Self, Position)> {
        let document = Self::load(root, &site.file)?;
        let position = site.position(&document.text)?;
        Ok((document, position))
    }

    pub(super) async fn open(&self, server: &RunningServer) -> anyhow::Result<()> {
        server.notify("textDocument/didOpen", json!({ "textDocument": {
            "uri": self.uri, "languageId": "rust", "text": self.text, "version": self.version,
        }})).await
    }

    async fn change(&mut self, server: &RunningServer, next: String) -> anyhow::Result<()> {
        // One range covers the changed text. Keep the unchanged prefix and suffix out of the
        // notification, using scalar boundaries before converting to UTF-16 coordinates.
        let start = self
            .text
            .chars()
            .zip(next.chars())
            .take_while(|(left, right)| left == right)
            .map(|(ch, _)| ch.len_utf8())
            .sum::<usize>();
        let suffix = self.text[start..]
            .chars()
            .rev()
            .zip(next[start..].chars().rev())
            .take_while(|(left, right)| left == right)
            .map(|(ch, _)| ch.len_utf8())
            .sum::<usize>();
        let range = Range::new(
            position(&self.text[..start]),
            position(&self.text[..self.text.len() - suffix]),
        );
        self.version += 1;
        server.notify("textDocument/didChange", json!({
            "textDocument": { "uri": self.uri, "version": self.version },
            "contentChanges": [{ "range": range, "text": &next[start..next.len() - suffix] }],
        })).await?;
        self.text = next;
        Ok(())
    }

    async fn save_and_close(&self, server: &RunningServer) -> anyhow::Result<()> {
        self.save(server).await?;
        self.close(server).await
    }

    async fn save(&self, server: &RunningServer) -> anyhow::Result<()> {
        fs::write(&self.path, &self.text)
            .with_context(|| format!("save {}", self.path.display()))?;
        server
            .notify(
                "textDocument/didSave",
                json!({
                    "textDocument": { "uri": self.uri }, "text": self.text,
                }),
            )
            .await
    }

    async fn close(&self, server: &RunningServer) -> anyhow::Result<()> {
        server
            .notify(
                "textDocument/didClose",
                json!({ "textDocument": { "uri": self.uri } }),
            )
            .await
    }

    fn viewport(&self, anchor: Position) -> Range {
        let start = anchor.line.saturating_sub(15);
        let end = position(&self.text);
        let end = if start + 35 < end.line {
            Position::new(start + 35, 0)
        } else {
            end
        };
        Range::new(Position::new(start, 0), end)
    }

    fn request<'a>(
        &self,
        action: Action,
        position: Position,
        expected: Expected<'a>,
    ) -> Request<'a> {
        Request {
            action,
            expected,
            params: json!({ "textDocument": { "uri": self.uri }, "position": position }),
        }
    }

    fn hints(&self, action: Action, position: Position) -> Request<'static> {
        Request {
            action,
            expected: Expected::Hints,
            params: json!({ "textDocument": { "uri": self.uri }, "range": self.viewport(position) }),
        }
    }

    pub(super) async fn first_hover(
        &self,
        server: &RunningServer,
        position: Position,
        expected: &str,
    ) -> Observation {
        self.request(Action::FIRST_HOVER, position, Expected::Type(expected))
            .observe(
                server,
                Instant::now(),
                Duration::from_secs(120),
                Retry::UntilUsable,
            )
            .await
    }
}

pub(super) struct EditorScenario<'a> {
    target: &'a Target,
    entry: Document,
    entry_position: Position,
    definition: Document,
    definition_position: Position,
    changed: String,
    draft: String,
    completion_position: Position,
}

impl<'a> EditorScenario<'a> {
    pub(super) fn load(root: &Path, target: &'a Target) -> anyhow::Result<Self> {
        let entry = Document::load(root, &target.entry.file)?;
        let definition = Document::load(root, &target.definition.file)?;
        let entry_position = target.entry.position(&entry.text)?;
        let definition_position = target.definition.position(&definition.text)?;
        let changed = target.edit(&definition.text)?;
        // Leave a partially typed identifier at the cursor for completion. The saved version
        // restores that identifier and keeps the type edit.
        let offset = anchor_offset(&changed, &target.completion.anchor)?;
        let completion_position = position(&changed[..offset]);
        let suffix = changed[offset..]
            .bytes()
            .take_while(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
            .count();
        let mut draft = changed.clone();
        draft.replace_range(offset..offset + suffix, "");
        Ok(Self {
            target,
            entry,
            entry_position,
            definition,
            definition_position,
            changed,
            draft,
            completion_position,
        })
    }

    pub(super) async fn run(
        mut self,
        server: &mut RunningServer,
        observations: &mut Vec<Observation>,
    ) -> anyhow::Result<Instant> {
        self.entry.open(server).await?;
        let ready = server.wait_until_ready().await?;
        let origin = Instant::now();
        let hints = self.entry.hints(Action::EDITOR_HINTS, self.entry_position);
        let hover = self.entry.request(
            Action::HOVER_CALL,
            self.entry_position,
            Expected::Type(&self.target.return_type.before),
        );
        let definition_uri = self.definition.uri.clone();
        let definition = self.entry.request(
            Action::DEFINITION,
            self.entry_position,
            Expected::Destination(&definition_uri, self.definition_position),
        );
        // The edit depends on navigation. The opening traffic keeps its own schedule so a slow
        // background request never delays a user's hover or navigation.
        let mut editing = Vec::new();
        let (hints, hover, edit_result) = tokio::join!(
            hints.at(server, origin, 75),
            hover.at(server, origin, 180),
            async {
                let navigation = definition.at(server, origin, 375).await;
                let navigated = navigation.status == Status::Usable;
                editing.push(navigation);
                if !navigated {
                    return Ok::<_, anyhow::Error>(());
                }
                self.definition.open(server).await?;
                tokio::time::sleep(Duration::from_millis(150)).await;
                self.definition.change(server, self.draft).await?;
                let completion = self
                    .definition
                    .request(
                        Action::EDIT_COMPLETION,
                        self.completion_position,
                        Expected::Completion(&self.target.completion.label),
                    )
                    .observe(server, Instant::now(), ACTION_TIMEOUT, Retry::Transient)
                    .await;
                editing.push(completion);

                self.definition.change(server, self.changed).await?;
                let saved_at = Instant::now();
                self.definition.save_and_close(server).await?;
                let saved = self
                    .entry
                    .request(
                        Action::SAVED_TYPE,
                        self.entry_position,
                        Expected::Type(&self.target.return_type.after),
                    )
                    .observe(server, saved_at, ACTION_TIMEOUT, Retry::UntilUsable)
                    .await;
                editing.push(saved);
                Ok(())
            },
        );
        observations.extend([hints, hover]);
        observations.extend(editing);
        edit_result?;
        Ok(ready)
    }
}

pub(super) async fn first_hints(
    root: &Path,
    target: &Target,
    server: &mut RunningServer,
) -> anyhow::Result<(Instant, Vec<Observation>)> {
    let (entry, position) = Document::load_at(root, &target.entry)?;
    entry.open(server).await?;
    let ready = server.wait_until_ready().await?;

    // Unlike the isolated mixed probes, these hints run before any other semantic query.
    let mut observations = Vec::with_capacity(2);
    for action in [Action::FIRST_HINTS, Action::REPEAT_HINTS] {
        observations.push(
            entry
                .hints(action, position)
                .observe(server, Instant::now(), ACTION_TIMEOUT, Retry::Transient)
                .await,
        );
    }
    Ok((ready, observations))
}

pub(super) async fn isolated_requests(
    root: &Path,
    target: &Target,
    server: &mut RunningServer,
) -> anyhow::Result<(Instant, Vec<Observation>)> {
    let (entry, entry_position) = Document::load_at(root, &target.entry)?;
    entry.open(server).await?;
    let ready = server.wait_until_ready().await?;

    let hover = |action| {
        entry.request(
            action,
            entry_position,
            Expected::Type(&target.return_type.before),
        )
    };
    let requests = [
        hover(Action::FIRST_ISOLATED_HOVER),
        hover(Action::REPEAT_ISOLATED_HOVER),
        entry.hints(Action::FIRST_ISOLATED_HINTS, entry_position),
        entry.hints(Action::REPEAT_ISOLATED_HINTS, entry_position),
    ];
    let mut observations = Vec::with_capacity(requests.len());
    for request in requests {
        observations.push(
            request
                .observe(server, Instant::now(), ACTION_TIMEOUT, Retry::Transient)
                .await,
        );
    }
    Ok((ready, observations))
}

pub(super) async fn startup(
    root: &Path,
    site: &Site,
    expected: &str,
    server: &RunningServer,
) -> anyhow::Result<Observation> {
    let (document, position) = Document::load_at(root, site)?;
    document.open(server).await?;
    Ok(document.first_hover(server, position, expected).await)
}

pub(super) fn milliseconds(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}
