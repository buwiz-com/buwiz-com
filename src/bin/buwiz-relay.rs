//! Reference relay client: does what the Buwiz app does when the person
//! presses Submit. It sends the return, then forwards bytes between the
//! server and BIR so the filing leaves from this computer.
//!
//! buwiz-relay --server URL --token ACCESS_TOKEN --file RETURN.xml --filename NAME
//!             [--idempotency-key KEY] [--allow HOST:PORT]...
//!
//! `--allow` adds a destination beyond BIR's own hosts (lab servers only).

use anyhow::{Context, bail};
use buwiz_cloud::relay::{Control, Frame, client};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

struct Args {
    server: String,
    token: String,
    file: String,
    filename: String,
    idempotency_key: Option<String>,
    allow: Vec<(String, u16)>,
}

fn args() -> anyhow::Result<Args> {
    let mut server = None;
    let mut token = None;
    let mut file = None;
    let mut filename = None;
    let mut idempotency_key = None;
    let mut allow = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let mut value = || args.next().with_context(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--server" => server = Some(value()?.trim_end_matches('/').to_owned()),
            "--token" => token = Some(value()?),
            "--file" => file = Some(value()?),
            "--filename" => filename = Some(value()?),
            "--idempotency-key" => idempotency_key = Some(value()?),
            "--allow" => {
                let raw = value()?;
                let (host, port) = raw.rsplit_once(':').context("--allow takes HOST:PORT")?;
                allow.push((host.to_owned(), port.parse().context("--allow port")?));
            }
            other => bail!("unknown flag {other}"),
        }
    }
    Ok(Args {
        server: server.context("--server is required")?,
        token: token.context("--token is required")?,
        file: file.context("--file is required")?,
        filename: filename.context("--filename is required")?,
        idempotency_key,
        allow,
    })
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = args()?;
    let xml = std::fs::read_to_string(&args.file).with_context(|| format!("could not read {}", args.file))?;

    let created: Value = reqwest::Client::new()
        .post(format!("{}/api/submissions", args.server))
        .bearer_auth(&args.token)
        .json(&json!({ "filename": args.filename, "xml": xml, "idempotency_key": args.idempotency_key }))
        .send()
        .await?
        .json()
        .await?;
    let relay_path = created["relay_path"].as_str().with_context(|| format!("submission refused: {created}"))?;
    eprintln!("submission {} ({})", created["id"], created["status"]);

    let ws_url = format!("{}{relay_path}", args.server.replacen("http", "ws", 1));
    let mut request = ws_url.into_client_request()?;
    request.headers_mut().insert(http::header::AUTHORIZATION, format!("Bearer {}", args.token).parse()?);
    let (socket, _) = tokio_tungstenite::connect_async(request).await.context("could not open the relay")?;
    let (mut sink, mut stream) = socket.split();

    let (to_server, mut outgoing) = mpsc::channel::<Frame>(64);
    let (to_app, incoming) = mpsc::channel::<Frame>(64);
    tokio::spawn(async move {
        while let Some(frame) = outgoing.recv().await {
            let message = match frame {
                Frame::Control(control) => Message::Text(serde_json::to_string(&control)?.into()),
                Frame::Data { stream, bytes } => Message::Binary(Frame::encode_data(stream, &bytes).into()),
            };
            sink.send(message).await?;
        }
        anyhow::Ok(())
    });
    tokio::spawn(async move {
        while let Some(Ok(message)) = stream.next().await {
            let frame = match message {
                Message::Text(text) => Frame::decode_text(&text),
                Message::Binary(bytes) => Frame::decode_data(&bytes),
                Message::Close(_) => break,
                _ => continue,
            };
            if let Some(frame) = frame
                && to_app.send(frame).await.is_err()
            {
                break;
            }
        }
    });

    let mut allowed = client::bir_destinations();
    allowed.extend(args.allow);
    match client::serve(to_server, incoming, &allowed).await {
        Some(Control::Result { status, error, receipt }) => {
            println!("{}", json!({ "status": status, "error": error, "receipt": receipt }));
            if matches!(status.as_str(), "sent" | "dry_run") { Ok(()) } else { std::process::exit(1) }
        }
        _ => bail!("the server closed the relay without a result"),
    }
}
