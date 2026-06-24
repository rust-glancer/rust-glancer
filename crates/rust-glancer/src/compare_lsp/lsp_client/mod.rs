//! Minimal JSON-RPC client for stdio LSP servers.
//!
//! The benchmark needs the same protocol boundary for rust-glancer and rust-analyzer. This module
//! deliberately stops at framed JSON-RPC over generic `Read`/`Write`; process spawning, stderr
//! capture, and server-specific launch details belong to the lifecycle harness.

use std::{
    collections::HashMap,
    io::{Read, Write},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

use anyhow::Context as _;
use serde_json::{Value, json};

const MAX_HEADER_BYTES: usize = 8 * 1024;

#[derive(Debug)]
pub(crate) struct LspClient<W> {
    writer: W,
    incoming: Receiver<ReaderEvent>,
    pending_responses: HashMap<u64, RpcResponse>,
    next_request_id: u64,
}

impl<W: Write> LspClient<W> {
    pub(crate) fn new<R>(reader: R, writer: W) -> Self
    where
        R: Read + Send + 'static,
    {
        let (sender, incoming) = mpsc::channel();

        thread::spawn(move || reader_loop(reader, sender));

        Self {
            writer,
            incoming,
            pending_responses: HashMap::new(),
            next_request_id: 1,
        }
    }

    pub(crate) fn request(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> RequestOutcome {
        let request_id = self.next_request_id;
        self.next_request_id += 1;

        let message = json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": method,
            "params": params,
        });

        if let Err(error) = write_framed_message(&mut self.writer, &message)
            .with_context(|| format!("while attempting to send LSP request `{method}`"))
        {
            return RequestOutcome::TransportFailure {
                message: error.to_string(),
            };
        }

        self.wait_for_response(request_id, timeout)
    }

    pub(crate) fn notify(&mut self, method: &str, params: Value) -> anyhow::Result<()> {
        let message = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });

        write_framed_message(&mut self.writer, &message)
            .with_context(|| format!("while attempting to send LSP notification `{method}`"))
    }

    fn wait_for_response(&mut self, request_id: u64, timeout: Duration) -> RequestOutcome {
        if let Some(response) = self.pending_responses.remove(&request_id) {
            return response.into_outcome();
        }

        let deadline = Instant::now() + timeout;
        loop {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                return RequestOutcome::Timeout;
            };

            match self.incoming.recv_timeout(remaining) {
                Ok(ReaderEvent::Message(IncomingMessage::Response(response))) => {
                    if response.id == request_id {
                        return response.into_outcome();
                    }
                    self.pending_responses.insert(response.id, response);
                }
                Ok(ReaderEvent::Message(IncomingMessage::Request(request))) => {
                    if let Err(error) = self.respond_to_server_request(request) {
                        return RequestOutcome::TransportFailure {
                            message: error.to_string(),
                        };
                    }
                }
                Ok(ReaderEvent::Message(IncomingMessage::Notification(_notification))) => {}
                Ok(ReaderEvent::TransportClosed) => {
                    return RequestOutcome::TransportFailure {
                        message: "LSP peer closed stdout".to_string(),
                    };
                }
                Ok(ReaderEvent::TransportError(message)) => {
                    return RequestOutcome::TransportFailure { message };
                }
                Err(mpsc::RecvTimeoutError::Timeout) => return RequestOutcome::Timeout,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return RequestOutcome::TransportFailure {
                        message: "LSP reader thread stopped before receiving a response"
                            .to_string(),
                    };
                }
            }
        }
    }

    fn respond_to_server_request(&mut self, request: IncomingRequest) -> anyhow::Result<()> {
        let result = default_server_request_result(&request);
        let message = json!({
            "jsonrpc": "2.0",
            "id": request.id,
            "result": result,
        });

        write_framed_message(&mut self.writer, &message).with_context(|| {
            format!(
                "while attempting to answer server-to-client LSP request `{}`",
                request.method
            )
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum RequestOutcome {
    Success(Value),
    Error(RpcError),
    Timeout,
    TransportFailure { message: String },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RpcError {
    pub(crate) code: i64,
    pub(crate) message: String,
    pub(crate) data: Option<Value>,
}

#[derive(Debug)]
enum ReaderEvent {
    Message(IncomingMessage),
    TransportClosed,
    TransportError(String),
}

#[derive(Debug)]
enum IncomingMessage {
    Response(RpcResponse),
    Request(IncomingRequest),
    Notification(IncomingNotification),
}

impl IncomingMessage {
    fn parse(value: Value) -> anyhow::Result<Self> {
        let object = value
            .as_object()
            .context("while attempting to parse JSON-RPC message object")?;
        let has_id = object.contains_key("id");
        let is_response = has_id && (object.contains_key("result") || object.contains_key("error"));

        if is_response {
            return RpcResponse::parse(object).map(Self::Response);
        }
        if has_id {
            return IncomingRequest::parse(object).map(Self::Request);
        }
        IncomingNotification::parse(object).map(Self::Notification)
    }
}

#[derive(Debug)]
struct RpcResponse {
    id: u64,
    result: Option<Value>,
    error: Option<RpcError>,
}

impl RpcResponse {
    fn parse(object: &serde_json::Map<String, Value>) -> anyhow::Result<Self> {
        let id = object
            .get("id")
            .and_then(response_id)
            .context("while attempting to parse JSON-RPC response id")?;
        let result = object.get("result").cloned();
        let error = object.get("error").map(RpcError::parse).transpose()?;

        if result.is_none() && error.is_none() {
            anyhow::bail!("JSON-RPC response must contain either result or error");
        }

        Ok(Self { id, result, error })
    }

    fn into_outcome(self) -> RequestOutcome {
        match self.error {
            Some(error) => RequestOutcome::Error(error),
            None => RequestOutcome::Success(self.result.unwrap_or(Value::Null)),
        }
    }
}

impl RpcError {
    fn parse(value: &Value) -> anyhow::Result<Self> {
        let object = value
            .as_object()
            .context("while attempting to parse JSON-RPC error object")?;
        let code = object
            .get("code")
            .and_then(Value::as_i64)
            .context("while attempting to parse JSON-RPC error code")?;
        let message = object
            .get("message")
            .and_then(Value::as_str)
            .context("while attempting to parse JSON-RPC error message")?
            .to_string();
        let data = object.get("data").cloned();

        Ok(Self {
            code,
            message,
            data,
        })
    }
}

#[derive(Debug)]
struct IncomingRequest {
    id: Value,
    method: String,
    params: Option<Value>,
}

impl IncomingRequest {
    fn parse(object: &serde_json::Map<String, Value>) -> anyhow::Result<Self> {
        let id = object
            .get("id")
            .cloned()
            .context("while attempting to parse JSON-RPC request id")?;
        let method = object
            .get("method")
            .and_then(Value::as_str)
            .context("while attempting to parse JSON-RPC request method")?
            .to_string();
        let params = object.get("params").cloned();

        Ok(Self { id, method, params })
    }
}

#[derive(Debug)]
struct IncomingNotification {
    method: String,
    params: Option<Value>,
}

impl IncomingNotification {
    fn parse(object: &serde_json::Map<String, Value>) -> anyhow::Result<Self> {
        let method = object
            .get("method")
            .and_then(Value::as_str)
            .context("while attempting to parse JSON-RPC notification method")?
            .to_string();
        let params = object.get("params").cloned();

        Ok(Self { method, params })
    }
}

fn default_server_request_result(request: &IncomingRequest) -> Value {
    if request.method == "workspace/configuration" {
        let item_count = request
            .params
            .as_ref()
            .and_then(|params| params.get("items"))
            .and_then(Value::as_array)
            .map_or(0, Vec::len);

        return Value::Array(vec![Value::Null; item_count]);
    }

    Value::Null
}

fn response_id(value: &Value) -> Option<u64> {
    match value {
        Value::Number(number) => number.as_u64(),
        Value::String(text) => text.parse().ok(),
        _ => None,
    }
}

fn reader_loop<R: Read>(mut reader: R, sender: mpsc::Sender<ReaderEvent>) {
    loop {
        match read_framed_message(&mut reader) {
            Ok(Some(value)) => match IncomingMessage::parse(value) {
                Ok(message) => {
                    if sender.send(ReaderEvent::Message(message)).is_err() {
                        break;
                    }
                }
                Err(error) => {
                    let _ = sender.send(ReaderEvent::TransportError(error.to_string()));
                    break;
                }
            },
            Ok(None) => {
                let _ = sender.send(ReaderEvent::TransportClosed);
                break;
            }
            Err(error) => {
                let _ = sender.send(ReaderEvent::TransportError(error.to_string()));
                break;
            }
        }
    }
}

fn write_framed_message(writer: &mut impl Write, message: &Value) -> anyhow::Result<()> {
    let body = serde_json::to_vec(message)
        .context("while attempting to serialize JSON-RPC message body")?;

    write!(writer, "Content-Length: {}\r\n\r\n", body.len())
        .context("while attempting to write LSP frame header")?;
    writer
        .write_all(&body)
        .context("while attempting to write LSP frame body")?;
    writer
        .flush()
        .context("while attempting to flush LSP frame")
}

fn read_framed_message(reader: &mut impl Read) -> anyhow::Result<Option<Value>> {
    let Some(header) = read_header(reader)? else {
        return Ok(None);
    };
    let content_length = parse_content_length(&header)?;

    let mut body = vec![0; content_length];
    reader
        .read_exact(&mut body)
        .context("while attempting to read LSP frame body")?;
    serde_json::from_slice(&body)
        .context("while attempting to parse LSP frame body as JSON")
        .map(Some)
}

fn read_header(reader: &mut impl Read) -> anyhow::Result<Option<Vec<u8>>> {
    let mut header = Vec::new();
    let mut byte = [0_u8; 1];

    loop {
        let read = reader
            .read(&mut byte)
            .context("while attempting to read LSP frame header")?;
        if read == 0 {
            if header.is_empty() {
                return Ok(None);
            }
            anyhow::bail!("LSP stream ended before the frame header terminator");
        }

        header.push(byte[0]);
        if header.ends_with(b"\r\n\r\n") {
            return Ok(Some(header));
        }
        if header.len() > MAX_HEADER_BYTES {
            anyhow::bail!("LSP frame header exceeded {MAX_HEADER_BYTES} bytes");
        }
    }
}

fn parse_content_length(header: &[u8]) -> anyhow::Result<usize> {
    let text =
        std::str::from_utf8(header).context("while attempting to parse LSP header as UTF-8")?;
    for line in text.split("\r\n") {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("Content-Length") {
            return value
                .trim()
                .parse()
                .context("while attempting to parse LSP Content-Length header");
        }
    }

    anyhow::bail!("LSP frame header is missing Content-Length")
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Cursor, Read, Write},
        sync::{Arc, Mutex, mpsc},
        time::Duration,
    };

    use serde_json::{Value, json};

    use super::{LspClient, RequestOutcome, read_framed_message, write_framed_message};

    #[test]
    fn frame_roundtrip_uses_content_length_headers() {
        let message = json!({
            "jsonrpc": "2.0",
            "method": "initialized",
            "params": {},
        });
        let mut buffer = Vec::new();

        write_framed_message(&mut buffer, &message).expect("message should be framed");

        let mut cursor = Cursor::new(buffer);
        let parsed = read_framed_message(&mut cursor)
            .expect("frame should parse")
            .expect("frame should contain a message");

        assert_eq!(parsed, message, "framed message should roundtrip");
    }

    #[test]
    fn malformed_content_length_is_reported() {
        let mut cursor = Cursor::new(b"Content-Length: nope\r\n\r\n{}".to_vec());

        let error = read_framed_message(&mut cursor).expect_err("header should be rejected");

        assert!(
            error.to_string().contains("Content-Length"),
            "error should name the invalid header: {error}",
        );
    }

    #[test]
    fn request_waits_for_matching_response_id() {
        let reader = Cursor::new(frames([
            json!({"jsonrpc": "2.0", "id": 2, "result": "second"}),
            json!({"jsonrpc": "2.0", "id": 1, "result": "first"}),
        ]));
        let writer = SharedWriter::default();
        let mut client = LspClient::new(reader, writer);

        let first = client.request("first/request", Value::Null, Duration::from_secs(1));
        let second = client.request("second/request", Value::Null, Duration::from_secs(1));

        assert_eq!(
            first,
            RequestOutcome::Success(json!("first")),
            "first request should ignore and buffer the out-of-order response",
        );
        assert_eq!(
            second,
            RequestOutcome::Success(json!("second")),
            "second request should use the previously buffered matching response",
        );
    }

    #[test]
    fn request_timeout_is_separate_from_transport_close() {
        let (_sender, receiver) = mpsc::channel();
        let writer = SharedWriter::default();
        let mut client = LspClient::new(ChannelReader::new(receiver), writer);

        let outcome = client.request("slow/request", Value::Null, Duration::from_millis(10));

        assert_eq!(
            outcome,
            RequestOutcome::Timeout,
            "a quiet but still-open stream should be classified as timeout",
        );
    }

    #[test]
    fn server_to_client_request_gets_default_response() {
        let writer = SharedWriter::default();
        let reader = Cursor::new(frames([
            json!({
                "jsonrpc": "2.0",
                "id": "server-request",
                "method": "workspace/configuration",
                "params": {"items": [{"section": "rust-analyzer"}]},
            }),
            json!({"jsonrpc": "2.0", "id": 1, "result": "client-response"}),
        ]));
        let mut client = LspClient::new(reader, writer.clone());

        let outcome = client.request("client/request", Value::Null, Duration::from_secs(1));

        assert_eq!(
            outcome,
            RequestOutcome::Success(json!("client-response")),
            "client request should still complete after answering a server request",
        );

        let written = writer.bytes();
        let mut cursor = Cursor::new(written);
        let _client_request = read_framed_message(&mut cursor)
            .expect("client request frame should parse")
            .expect("client request should exist");
        let server_response = read_framed_message(&mut cursor)
            .expect("server response frame should parse")
            .expect("server response should exist");

        assert_eq!(
            server_response,
            json!({
                "jsonrpc": "2.0",
                "id": "server-request",
                "result": [null],
            }),
            "workspace/configuration should receive one null entry per requested item",
        );
    }

    fn frames<const N: usize>(messages: [Value; N]) -> Vec<u8> {
        let mut buffer = Vec::new();
        for message in messages {
            write_framed_message(&mut buffer, &message).expect("test frame should serialize");
        }
        buffer
    }

    #[derive(Clone, Debug, Default)]
    struct SharedWriter {
        bytes: Arc<Mutex<Vec<u8>>>,
    }

    impl SharedWriter {
        fn bytes(&self) -> Vec<u8> {
            self.bytes
                .lock()
                .expect("shared writer mutex should not be poisoned")
                .clone()
        }
    }

    impl Write for SharedWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.bytes
                .lock()
                .expect("shared writer mutex should not be poisoned")
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    struct ChannelReader {
        receiver: mpsc::Receiver<Vec<u8>>,
        pending: Cursor<Vec<u8>>,
    }

    impl ChannelReader {
        fn new(receiver: mpsc::Receiver<Vec<u8>>) -> Self {
            Self {
                receiver,
                pending: Cursor::new(Vec::new()),
            }
        }
    }

    impl Read for ChannelReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.pending.position() == self.pending.get_ref().len() as u64 {
                let bytes = self.receiver.recv().map_err(|_| {
                    std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "channel closed")
                })?;
                self.pending = Cursor::new(bytes);
            }

            self.pending.read(buf)
        }
    }
}
