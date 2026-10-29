//! The relay pipe: byte streams that the server opens through the
//! taxpayer's computer, so BIR sees the taxpayer's own connection while the
//! secrets and the SSH session stay on the server.
//!
//! One WebSocket carries everything. Text frames are JSON [`Control`]
//! messages; binary frames are data, `[stream id][bytes]`. The server asks
//! the app to `open` a TCP connection to a host and port; the app connects
//! only to hosts on its own allow-list and answers `opened` or `refused`.
//! Either side sends `close` when it has no more bytes to write. The server
//! ends with one `result`. See `docs/relay-protocol.md`.

pub mod client;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::sync::{Mutex, mpsc, oneshot};

const OPEN_TIMEOUT: Duration = Duration::from_secs(20);
const STREAM_BUFFER: usize = 64 * 1024;
const CHUNK: usize = 16 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Control {
    /// Server → app: connect to `host:port` for stream `stream`.
    Open { stream: u8, host: String, port: u16 },
    /// App → server: the connection is up.
    Opened { stream: u8 },
    /// App → server: the connection was not made.
    Refused { stream: u8, reason: String },
    /// Either way: no more bytes will be written on this stream.
    Close { stream: u8 },
    /// Server → app: the outcome. The server closes the pipe after this.
    Result { status: String, error: Option<String>, receipt: Option<Value> },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Frame {
    Control(Control),
    Data { stream: u8, bytes: Bytes },
}

impl Frame {
    /// A binary WebSocket message: `[stream id][bytes]`.
    pub fn encode_data(stream: u8, bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(bytes.len() + 1);
        out.push(stream);
        out.extend_from_slice(bytes);
        out
    }

    pub fn decode_data(message: &[u8]) -> Option<Self> {
        let (&stream, bytes) = message.split_first()?;
        Some(Self::Data { stream, bytes: Bytes::copy_from_slice(bytes) })
    }

    pub fn decode_text(message: &str) -> Option<Self> {
        serde_json::from_str(message).ok().map(Self::Control)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PipeError {
    #[error("the app refused to connect to {host}:{port}: {reason}")]
    Refused { host: String, port: u16, reason: String },
    #[error("the app did not connect to {host}:{port} in time")]
    Timeout { host: String, port: u16 },
    #[error("the app disconnected")]
    Gone,
}

#[derive(Default)]
struct Streams {
    pending: HashMap<u8, oneshot::Sender<Result<(), String>>>,
    /// Bytes from the app for each open stream; `None` means the app closed it.
    inbound: HashMap<u8, mpsc::UnboundedSender<Option<Bytes>>>,
    gone: bool,
}

/// The server end of a pipe.
#[derive(Clone)]
pub struct Pipe {
    out: mpsc::Sender<Frame>,
    streams: Arc<Mutex<Streams>>,
    next: Arc<AtomicU8>,
}

impl Pipe {
    /// `out` carries frames to the app; `inbound` brings its frames back.
    pub fn new(out: mpsc::Sender<Frame>, mut inbound: mpsc::Receiver<Frame>) -> Self {
        let streams = Arc::new(Mutex::new(Streams::default()));
        let routes = streams.clone();
        tokio::spawn(async move {
            while let Some(frame) = inbound.recv().await {
                let mut streams = routes.lock().await;
                match frame {
                    Frame::Control(Control::Opened { stream }) => {
                        if let Some(waiter) = streams.pending.remove(&stream) {
                            let _ = waiter.send(Ok(()));
                        }
                    }
                    Frame::Control(Control::Refused { stream, reason }) => {
                        if let Some(waiter) = streams.pending.remove(&stream) {
                            let _ = waiter.send(Err(reason));
                        }
                    }
                    Frame::Control(Control::Close { stream }) => {
                        if let Some(sender) = streams.inbound.remove(&stream) {
                            let _ = sender.send(None);
                        }
                    }
                    Frame::Data { stream, bytes } => {
                        if let Some(sender) = streams.inbound.get(&stream) {
                            let _ = sender.send(Some(bytes));
                        }
                    }
                    // Only the server sends these.
                    Frame::Control(Control::Open { .. } | Control::Result { .. }) => {}
                }
            }
            // The app is gone: every stream ends and every open fails.
            let mut streams = routes.lock().await;
            streams.gone = true;
            streams.pending.clear();
            streams.inbound.clear();
        });
        Self { out, streams, next: Arc::new(AtomicU8::new(1)) }
    }

    /// Asks the app to connect to `host:port` and returns the stream.
    pub async fn connect(&self, host: &str, port: u16) -> Result<DuplexStream, PipeError> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (opened, waiting) = oneshot::channel();
        {
            let mut streams = self.streams.lock().await;
            if streams.gone {
                return Err(PipeError::Gone);
            }
            streams.pending.insert(id, opened);
        }
        let open = Control::Open { stream: id, host: host.to_owned(), port };
        self.out.send(Frame::Control(open)).await.map_err(|_| PipeError::Gone)?;
        match tokio::time::timeout(OPEN_TIMEOUT, waiting).await {
            Ok(Ok(Ok(()))) => {}
            Ok(Ok(Err(reason))) => {
                return Err(PipeError::Refused { host: host.to_owned(), port, reason });
            }
            Ok(Err(_)) => return Err(PipeError::Gone),
            Err(_) => {
                self.streams.lock().await.pending.remove(&id);
                return Err(PipeError::Timeout { host: host.to_owned(), port });
            }
        }

        let (ours, theirs) = tokio::io::duplex(STREAM_BUFFER);
        let (mut from_us, mut to_us) = tokio::io::split(theirs);
        let (sender, mut from_app) = mpsc::unbounded_channel();
        self.streams.lock().await.inbound.insert(id, sender);
        // App → us.
        tokio::spawn(async move {
            while let Some(Some(bytes)) = from_app.recv().await {
                if to_us.write_all(&bytes).await.is_err() {
                    return;
                }
            }
            let _ = to_us.shutdown().await;
        });
        // Us → app.
        let out = self.out.clone();
        tokio::spawn(async move {
            let mut buffer = vec![0u8; CHUNK];
            loop {
                match from_us.read(&mut buffer).await {
                    Ok(0) | Err(_) => break,
                    Ok(read) => {
                        let frame = Frame::Data { stream: id, bytes: Bytes::copy_from_slice(&buffer[..read]) };
                        if out.send(frame).await.is_err() {
                            return;
                        }
                    }
                }
            }
            let _ = out.send(Frame::Control(Control::Close { stream: id })).await;
        });
        Ok(ours)
    }

    /// Sends the outcome. The caller then drops the pipe.
    pub async fn finish(&self, result: Control) {
        let _ = self.out.send(Frame::Control(result)).await;
    }
}

/// Moves frames between the WebSocket and the pipe until either ends. The
/// pipe ends after the server's `result`.
pub async fn pump_socket(socket: WebSocket, mut from_server: mpsc::Receiver<Frame>, to_server: mpsc::Sender<Frame>) {
    let (mut sink, mut stream) = socket.split();
    let writer = tokio::spawn(async move {
        while let Some(frame) = from_server.recv().await {
            let message = match frame {
                Frame::Control(control) => {
                    Message::Text(serde_json::to_string(&control).expect("controls serialize").into())
                }
                Frame::Data { stream, bytes } => Message::Binary(Frame::encode_data(stream, &bytes).into()),
            };
            if sink.send(message).await.is_err() {
                return;
            }
        }
        let _ = sink.send(Message::Close(None)).await;
    });
    while let Some(Ok(message)) = stream.next().await {
        let frame = match message {
            Message::Text(text) => Frame::decode_text(&text),
            Message::Binary(bytes) => Frame::decode_data(&bytes),
            Message::Close(_) => break,
            Message::Ping(_) | Message::Pong(_) => continue,
        };
        match frame {
            Some(frame) => {
                if to_server.send(frame).await.is_err() {
                    break;
                }
            }
            None => tracing::warn!("relay: unreadable frame from the app"),
        }
    }
    drop(to_server);
    let _ = tokio::time::timeout(Duration::from_secs(5), writer).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_messages_use_a_type_tag() {
        let open = Control::Open { stream: 1, host: "birgovph.com".into(), port: 443 };
        let json = serde_json::to_string(&open).unwrap();
        assert_eq!(json, r#"{"type":"open","stream":1,"host":"birgovph.com","port":443}"#);
        assert_eq!(Frame::decode_text(&json), Some(Frame::Control(open)));
        assert_eq!(Frame::decode_text(r#"{"type":"nope"}"#), None);
    }

    #[test]
    fn data_frames_lead_with_the_stream_id() {
        let encoded = Frame::encode_data(7, b"SSH-2.0");
        assert_eq!(encoded[0], 7);
        assert_eq!(
            Frame::decode_data(&encoded),
            Some(Frame::Data { stream: 7, bytes: Bytes::from_static(b"SSH-2.0") })
        );
        assert_eq!(Frame::decode_data(&[]), None);
    }
}
