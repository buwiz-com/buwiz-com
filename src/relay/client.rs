//! The app's end of the pipe: a dumb byte forwarder that connects only to
//! BIR. This is the reference for the eBIRForms client.

use std::collections::HashMap;
use std::time::Duration;

use bytes::Bytes;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

use super::{Control, Frame};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// BIR's dispatcher and upload hosts. The app opens nothing else, so the
/// server can never use a taxpayer's computer as a general proxy.
pub const BIR_DESTINATIONS: &[(&str, u16)] = &[
    ("birgovph.com", 443),
    ("birgovph.com", 80),
    ("ws2.birgovph.com", 443),
    ("ws2.birgovph.com", 80),
    ("ebf2.bir.gov.ph", 22),
];

/// Forwards bytes until the server sends its result, and returns it.
/// `allowed` is the list of `(host, port)` the app may open.
pub async fn serve(
    out: mpsc::Sender<Frame>,
    mut inbound: mpsc::Receiver<Frame>,
    allowed: &[(String, u16)],
) -> Option<Control> {
    let mut writers: HashMap<u8, mpsc::UnboundedSender<Option<Bytes>>> = HashMap::new();
    while let Some(frame) = inbound.recv().await {
        match frame {
            Frame::Control(Control::Open { stream, host, port }) => {
                let permitted = allowed.iter().any(|(allowed_host, allowed_port)| {
                    allowed_host.eq_ignore_ascii_case(&host) && *allowed_port == port
                });
                if !permitted {
                    let reason = format!("{host}:{port} is not a BIR server");
                    let _ = out.send(Frame::Control(Control::Refused { stream, reason })).await;
                    continue;
                }
                match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect((host.as_str(), port))).await {
                    Ok(Ok(tcp)) => {
                        let _ = tcp.set_nodelay(true);
                        // `opened` first: the far end may speak first (SSH).
                        let _ = out.send(Frame::Control(Control::Opened { stream })).await;
                        writers.insert(stream, forward(stream, tcp, out.clone()));
                    }
                    Ok(Err(error)) => {
                        let reason = error.to_string();
                        let _ = out.send(Frame::Control(Control::Refused { stream, reason })).await;
                    }
                    Err(_) => {
                        let reason = "connection timed out".to_owned();
                        let _ = out.send(Frame::Control(Control::Refused { stream, reason })).await;
                    }
                }
            }
            Frame::Data { stream, bytes } => {
                if let Some(writer) = writers.get(&stream) {
                    let _ = writer.send(Some(bytes));
                }
            }
            Frame::Control(Control::Close { stream }) => {
                if let Some(writer) = writers.remove(&stream) {
                    let _ = writer.send(None);
                }
            }
            Frame::Control(result @ Control::Result { .. }) => return Some(result),
            // Only the app sends these.
            Frame::Control(Control::Opened { .. } | Control::Refused { .. }) => {}
        }
    }
    None
}

/// Copies between one TCP connection and the pipe. Returns the sender that
/// feeds bytes from the server into the connection.
fn forward(stream: u8, tcp: TcpStream, out: mpsc::Sender<Frame>) -> mpsc::UnboundedSender<Option<Bytes>> {
    let (mut tcp_read, mut tcp_write) = tcp.into_split();
    let (sender, mut from_server) = mpsc::unbounded_channel::<Option<Bytes>>();
    tokio::spawn(async move {
        while let Some(Some(bytes)) = from_server.recv().await {
            if tcp_write.write_all(&bytes).await.is_err() {
                return;
            }
        }
        let _ = tcp_write.shutdown().await;
    });
    tokio::spawn(async move {
        let mut buffer = vec![0u8; 16 * 1024];
        loop {
            match tcp_read.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    let frame = Frame::Data { stream, bytes: Bytes::copy_from_slice(&buffer[..read]) };
                    if out.send(frame).await.is_err() {
                        return;
                    }
                }
            }
        }
        let _ = out.send(Frame::Control(Control::Close { stream })).await;
    });
    sender
}

/// [`BIR_DESTINATIONS`] as owned pairs, for [`serve`].
pub fn bir_destinations() -> Vec<(String, u16)> {
    BIR_DESTINATIONS.iter().map(|(host, port)| ((*host).to_owned(), *port)).collect()
}
