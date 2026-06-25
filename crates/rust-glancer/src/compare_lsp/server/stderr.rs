//! Bounded stderr capture for spawned LSP servers.
//!
//! Server stderr is usually only needed after a protocol failure. This helper keeps the recent tail
//! available for error messages without letting a noisy server grow memory for the whole run.

use std::sync::{Arc, Mutex};

use tokio::{
    io::{AsyncRead, AsyncReadExt},
    task::JoinHandle,
};

const MAX_STDERR_CAPTURE_BYTES: usize = 64 * 1024;

/// Async stderr reader that retains the most recent bytes for diagnostics.
#[derive(Debug)]
pub(super) struct StderrCapture {
    bytes: Arc<Mutex<Vec<u8>>>,
    join_handle: Option<JoinHandle<()>>,
}

impl StderrCapture {
    pub(super) fn spawn(mut reader: impl AsyncRead + Send + Unpin + 'static) -> Self {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&bytes);
        let join_handle = tokio::spawn(async move {
            let mut buffer = [0_u8; 4096];
            loop {
                let read = match reader.read(&mut buffer).await {
                    Ok(0) => break,
                    Ok(read) => read,
                    Err(_error) => break,
                };
                let mut bytes = captured
                    .lock()
                    .expect("stderr capture mutex should not be poisoned");
                bytes.extend_from_slice(&buffer[..read]);
                if bytes.len() > MAX_STDERR_CAPTURE_BYTES {
                    let excess = bytes.len() - MAX_STDERR_CAPTURE_BYTES;
                    bytes.drain(..excess);
                }
            }
        });

        Self {
            bytes,
            join_handle: Some(join_handle),
        }
    }

    /// Return the captured stderr tail as lossy UTF-8 for user-facing errors.
    pub(super) fn snippet(&self) -> String {
        let bytes = self
            .bytes
            .lock()
            .expect("stderr capture mutex should not be poisoned");
        String::from_utf8_lossy(&bytes).trim().to_string()
    }

    pub(super) async fn join(&mut self) {
        if let Some(join_handle) = self.join_handle.take() {
            let _ = join_handle.await;
        }
    }
}

impl Drop for StderrCapture {
    fn drop(&mut self) {
        if let Some(join_handle) = self.join_handle.take() {
            join_handle.abort();
        }
    }
}
