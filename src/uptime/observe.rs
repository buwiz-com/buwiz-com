//! Watches one relay try in the Gatekeeper's bridge
//! (`crate::filing::submissions`), frame by frame, and ends in what the
//! record keeps of it. A pure state machine: it is fed the raw frames and
//! touches nothing.
//!
//! It reads the frames as raw JSON, so the relay's [`crate::relay::Control`]
//! stays as it is: the Sender's first frame `{"type":"sender","mode":…}`,
//! its `open{stream,port}` (never the host), the app's `opened` and
//! `refused{reason[,kind]}`, and the app's data. On an upload stream the
//! data goes to [`sshkex`] until it has the host key or gives up. It reads
//! leniently: a frame is known by its `type` alone, and any other field of
//! an unexpected type reads as absent, so a `refused` with an odd `kind` is
//! still a refusal and a `sender` frame with an odd `mode` still stops here.

use serde_json::Value;

use super::classify::{self, Answer, Class, Classified, Kind, Mode, Seen, StreamFacts};
use super::sshkex::HostKeyReader;

/// The fields of a relay text frame the observer reads. Others are ignored.
struct RawFrame {
    frame: String,
    stream: Option<u8>,
    port: Option<u16>,
    mode: Option<String>,
    reason: Option<String>,
    kind: Option<String>,
}

/// A text frame: a JSON object with a string `type`, its other fields each
/// read on its own.
fn raw(text: &str) -> Option<RawFrame> {
    let value: Value = serde_json::from_str(text).ok()?;
    let object = value.as_object()?;
    let text_of = |name: &str| object.get(name).and_then(Value::as_str).map(str::to_owned);
    let number = |name: &str| object.get(name).and_then(Value::as_u64);
    Some(RawFrame {
        frame: text_of("type")?,
        stream: number("stream").and_then(|stream| u8::try_from(stream).ok()),
        port: number("port").and_then(|port| u16::try_from(port).ok()),
        mode: text_of("mode"),
        reason: text_of("reason"),
        kind: text_of("kind"),
    })
}

/// Whether a Sender text frame is its `sender` frame, which never goes to
/// the app.
pub fn is_sender_frame(text: &str) -> bool {
    raw(text).is_some_and(|frame| frame.frame == "sender")
}

struct Stream {
    id: u8,
    port: u16,
    answer: Option<Answer>,
    bytes: u64,
    kex: Option<HostKeyReader>,
}

/// One try, as it goes.
pub struct TryObserver {
    upload_ports: Vec<u16>,
    mode: Option<Mode>,
    streams: Vec<Stream>,
    app_gone: bool,
    sender_down: bool,
    sender_lost: bool,
    wait_expired: bool,
    host_key: Option<String>,
}

/// What the record keeps of a try.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observed {
    pub mode: Mode,
    pub classified: Classified,
    /// `None` for a try that got in, and for any try in `lab` or `dry_run`.
    pub kind: Option<Kind>,
    /// How many streams the Sender asked for.
    pub opens: u16,
    /// A stream reached a server, and not in `lab`.
    pub touched_bir: bool,
    pub counted: bool,
    /// The upload server's host key, if the try was offered one. Read from
    /// bytes the user's app relays, so it can be forged: information only.
    pub host_key: Option<String>,
    /// Whether the Sender went on to complete the key exchange with that
    /// key ([`classify::key_exchange_done`]).
    pub key_exchange_done: bool,
}

impl Observed {
    /// Whose side it failed on, as the app is told.
    pub fn class(&self) -> Option<Class> {
        self.kind.map(Kind::class)
    }
}

impl TryObserver {
    pub fn new(upload_ports: &[u16]) -> Self {
        Self {
            upload_ports: upload_ports.to_vec(),
            mode: None,
            streams: Vec::new(),
            app_gone: false,
            sender_down: false,
            sender_lost: false,
            wait_expired: false,
            host_key: None,
        }
    }

    fn is_upload(&self, port: u16) -> bool {
        // A lab send skips the front desk: its one stream is the upload.
        self.mode == Some(Mode::Lab) || self.upload_ports.contains(&port)
    }

    fn stream(&mut self, id: u8) -> Option<&mut Stream> {
        self.streams.iter_mut().rev().find(|stream| stream.id == id)
    }

    /// A text frame from the Sender. `false` for its `sender` frame, which
    /// the bridge does not forward; any other frame goes on to the app.
    pub fn sender_text(&mut self, text: &str) -> bool {
        let Some(frame) = raw(text) else {
            return true;
        };
        match frame.frame.as_str() {
            "sender" => {
                // Only the first one says the mode.
                if self.mode.is_none() {
                    self.mode = Some(frame.mode.as_deref().map_or(Mode::Unknown, Mode::parse));
                }
                false
            }
            "open" => {
                if let (Some(id), Some(port)) = (frame.stream, frame.port) {
                    // The Sender asks for one stream at a time and waits for
                    // its answer: one still unanswered timed out.
                    for stream in &mut self.streams {
                        stream.answer.get_or_insert(Answer::NoAnswer);
                    }
                    let kex = self.is_upload(port).then(HostKeyReader::new);
                    self.streams.push(Stream { id, port, answer: None, bytes: 0, kex });
                }
                true
            }
            _ => true,
        }
    }

    /// A text frame from the app (forwarded unchanged by the bridge).
    pub fn app_text(&mut self, text: &str) {
        let Some(frame) = raw(text) else {
            return;
        };
        let Some(id) = frame.stream else {
            return;
        };
        let answer = match frame.frame.as_str() {
            "opened" => Answer::Opened,
            "refused" => Answer::Refused(classify::refusal_kind(
                frame.reason.as_deref().unwrap_or_default(),
                frame.kind.as_deref(),
            )),
            _ => return,
        };
        if let Some(stream) = self.stream(id)
            && stream.answer.is_none()
        {
            stream.answer = Some(answer);
        }
    }

    /// A binary frame from the app: `[stream][bytes]` from the far end.
    pub fn app_binary(&mut self, message: &[u8]) {
        let Some((&id, bytes)) = message.split_first() else {
            return;
        };
        let mut found = None;
        if let Some(stream) = self.stream(id) {
            stream.bytes = stream.bytes.saturating_add(bytes.len() as u64);
            // Bytes from the far end mean it connected, whatever the order
            // the frames came in.
            stream.answer.get_or_insert(Answer::Opened);
            if let Some(kex) = stream.kex.as_mut() {
                found = kex.feed(bytes);
                if kex.is_done() {
                    stream.kex = None;
                }
            }
        }
        if self.host_key.is_none() {
            self.host_key = found;
        }
    }

    /// The app's socket closed before the result.
    pub fn app_gone(&mut self) {
        self.app_gone = true;
    }

    /// The Sender could not be reached; no job went out.
    pub fn sender_down(&mut self) {
        self.sender_down = true;
    }

    /// The Sender's socket ended (or the 24-hour bound ran out) without a
    /// result.
    pub fn sender_lost(&mut self) {
        self.sender_lost = true;
    }

    /// The app went away and the Sender's outcome did not come within the
    /// 60 s the bridge waits for it.
    pub fn wait_expired(&mut self) {
        self.wait_expired = true;
    }

    /// The result the app is told: its `status` and `error`.
    pub fn finish(self, status: &str, error: Option<&str>) -> Observed {
        let mode = self.mode.unwrap_or(Mode::Unknown);
        let facts: Vec<StreamFacts> = self
            .streams
            .iter()
            .map(|stream| {
                let upload = mode == Mode::Lab || self.upload_ports.contains(&stream.port);
                StreamFacts {
                    upload,
                    tls: !upload && stream.port == 443,
                    answer: stream.answer.unwrap_or(Answer::NoAnswer),
                    bytes: stream.bytes,
                }
            })
            .collect();
        let seen = Seen {
            mode,
            status,
            error: error.unwrap_or_default(),
            streams: &facts,
            app_gone: self.app_gone,
            sender_down: self.sender_down,
            sender_lost: self.sender_lost,
            wait_expired: self.wait_expired,
        };
        let classified = classify::classify(&seen);
        let kind = match mode {
            Mode::Live => classified.kind,
            Mode::Lab | Mode::DryRun => None,
            // No `sender` frame (a Sender from before it): like lab, no
            // class and nothing told. A Sender that could not be reached
            // at all is Buwiz's side whatever its mode.
            Mode::Unknown if self.sender_down => classified.kind,
            Mode::Unknown => None,
        };
        Observed {
            mode,
            kind,
            opens: u16::try_from(self.streams.len()).unwrap_or(u16::MAX),
            touched_bir: classified.opened && mode != Mode::Lab,
            counted: classify::counted(mode, &classified),
            key_exchange_done: self.host_key.is_some() && classify::key_exchange_done(&seen),
            host_key: self.host_key,
            classified,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::uptime::classify::{Outcome, Stage, Window};
    use crate::uptime::sshkex;

    /// One frame of a try, as the bridge sees it.
    #[derive(Clone)]
    enum Step {
        /// The Sender's `sender` frame.
        Says(&'static str),
        /// The Sender's `open` for stream N on a port.
        Open(u8, u16),
        Opened(u8),
        Refused(u8, &'static str, Option<&'static str>),
        /// Bytes from the far end on stream N.
        Bytes(u8, Vec<u8>),
        AppGone,
        SenderLost,
        SenderDown,
        WaitExpired,
    }
    use Step::*;

    fn text(step: &Step) -> Option<String> {
        Some(match step {
            Says(mode) => format!(r#"{{"type":"sender","mode":"{mode}"}}"#),
            Open(stream, port) => {
                format!(r#"{{"type":"open","stream":{stream},"host":"dispatcher.test","port":{port}}}"#)
            }
            Opened(stream) => format!(r#"{{"type":"opened","stream":{stream}}}"#),
            Refused(stream, reason, None) => format!(r#"{{"type":"refused","stream":{stream},"reason":"{reason}"}}"#),
            Refused(stream, reason, Some(kind)) => {
                format!(r#"{{"type":"refused","stream":{stream},"reason":"{reason}","kind":"{kind}"}}"#)
            }
            _ => return None,
        })
    }

    fn observe(steps: &[Step], status: &str, error: &str) -> Observed {
        let mut observer = TryObserver::new(&[22]);
        for step in steps {
            match step {
                Says(_) => assert!(!observer.sender_text(&text(step).unwrap()), "the sender frame is never forwarded"),
                Open(..) => assert!(observer.sender_text(&text(step).unwrap())),
                Opened(_) | Refused(..) => observer.app_text(&text(step).unwrap()),
                Bytes(stream, bytes) => observer.app_binary(&[&[*stream], bytes.as_slice()].concat()),
                AppGone => observer.app_gone(),
                SenderLost => observer.sender_lost(),
                SenderDown => observer.sender_down(),
                WaitExpired => observer.wait_expired(),
            }
        }
        observer.finish(status, (!error.is_empty()).then_some(error))
    }

    /// The row's columns: outcome, stage, dispatcher, upload, kind, counted,
    /// touched_bir.
    type Row = (Outcome, Stage, Window, Window, Option<Kind>, bool, bool);

    fn row(observed: &Observed) -> Row {
        let c = &observed.classified;
        (c.outcome, c.stage, c.dispatcher, c.upload, observed.kind, observed.counted, observed.touched_bir)
    }

    const REFUSED: &str = "Connection refused (os error 61)";

    /// The front desk answered on 443, then the upload server opened.
    fn desk_then_upload() -> Vec<Step> {
        vec![
            Says("live"),
            Open(1, 443),
            Opened(1),
            Bytes(1, vec![22; 900]),
            Open(2, 22),
            Opened(2),
            Bytes(2, b"SSH-2.0-x\r\n".to_vec()),
        ]
    }

    fn all_four(reason: &str) -> String {
        let attempt = |scheme: &str, host: &str| format!("{scheme}://{host}: {reason}");
        format!(
            "BIR's dispatcher did not answer: {}; {}; {}; {}",
            attempt("https", "dispatcher.test"),
            attempt("http", "dispatcher.test"),
            attempt("https", "backup.test"),
            attempt("http", "backup.test")
        )
    }

    fn four_refused(reason: &'static str, kind: Option<&'static str>) -> Vec<Step> {
        let mut steps = vec![Says("live")];
        for (stream, port) in [(1, 443), (2, 80), (3, 443), (4, 80)] {
            steps.push(Open(stream, port));
            steps.push(Refused(stream, reason, kind));
        }
        steps
    }

    #[test]
    fn each_line_of_the_table() {
        use Kind as K;
        use Outcome as O;
        use Stage as S;
        use Window as W;
        let cut = "SFTP failed during the upload: connection reset. BIR may have received this return; check for its confirmation email before sending again.";
        let upload_refused = "could not reach BIR's upload server: the app refused to connect to upload.test:22: Connection refused (os error 61)";
        let upload_late = "could not reach BIR's upload server: the app did not connect to upload.test:22 in time";
        let mut upload_no_answer = desk_then_upload();
        upload_no_answer.truncate(5);
        let mut upload_refusal = desk_then_upload();
        upload_refusal.truncate(5);
        upload_refusal.push(Refused(2, REFUSED, Some("refused")));
        let desk_only = vec![Says("live"), Open(1, 443), Opened(1), Bytes(1, vec![1; 400])];
        let mut app_gone = desk_then_upload();
        app_gone.push(AppGone);
        let tls_then_http = vec![
            Says("live"),
            Open(1, 443),
            Opened(1),
            Bytes(1, vec![1; 40]),
            Open(2, 80),
            Opened(2),
            Bytes(2, vec![1; 200]),
            Open(3, 443),
            Refused(3, REFUSED, None),
            Open(4, 80),
            Refused(4, REFUSED, None),
        ];
        let tls_then_http_words = "BIR's dispatcher did not answer: https://dispatcher.test: TLS: invalid peer certificate: UnknownIssuer; \
            http://dispatcher.test: dispatcher answered 503 Service Unavailable; \
            https://backup.test: the app refused to connect to backup.test:443: Connection refused (os error 61); \
            http://backup.test: the app refused to connect to backup.test:80: Connection refused (os error 61)";
        let cases: Vec<(&str, Vec<Step>, &str, String, Row)> = vec![
            ("sent", desk_then_upload(), "sent", String::new(), (O::Sent, S::Uploaded, W::Ok, W::Ok, None, true, true)),
            (
                "sent, the front desk answered over plain HTTP",
                vec![
                    Says("live"),
                    Open(1, 443),
                    Refused(1, REFUSED, None),
                    Open(2, 80),
                    Opened(2),
                    Bytes(2, vec![1; 300]),
                    Open(3, 22),
                    Opened(3),
                ],
                "sent",
                String::new(),
                (O::Sent, S::Uploaded, W::None, W::Ok, None, true, true),
            ),
            ("unknown: cut", desk_then_upload(), "unknown", cut.into(), (O::Unknown, S::Upload, W::Ok, W::Maybe, Some(K::Cut), true, true)),
            (
                "unknown stays when the app went away, but on the user's side",
                app_gone.clone(),
                "unknown",
                cut.into(),
                (O::Unknown, S::Upload, W::Ok, W::Maybe, Some(K::AppGone), false, true),
            ),
            (
                "the outcome wait expired after the app went away",
                [app_gone.clone(), vec![WaitExpired]].concat(),
                "unknown",
                "the send ended without an outcome; BIR may have received this return".into(),
                (O::Unknown, S::Upload, W::None, W::Maybe, Some(K::AppGone), false, true),
            ),
            (
                "the outcome wait expired before any upload",
                vec![Says("live"), Open(1, 443), Opened(1), AppGone, WaitExpired],
                "unknown",
                "the send ended without an outcome; BIR may have received this return".into(),
                (O::Unknown, S::Dispatcher, W::None, W::None, Some(K::AppGone), false, true),
            ),
            (
                "the app went away",
                app_gone,
                "failed",
                "SSH failed: the app disconnected".into(),
                (O::Failed, S::Upload, W::None, W::None, Some(K::AppGone), false, true),
            ),
            (
                "the front desk down: every stream refused",
                four_refused(REFUSED, None),
                "failed",
                all_four("the app refused to connect to dispatcher.test:443: Connection refused (os error 61)"),
                (O::Failed, S::Dispatcher, W::Fail, W::None, Some(K::Refused), true, false),
            ),
            (
                "no route to host is the BIR's side",
                four_refused("No route to host (os error 65)", None),
                "failed",
                all_four("the app refused to connect to dispatcher.test:443: No route to host (os error 65)"),
                (O::Failed, S::Dispatcher, W::Fail, W::None, Some(K::Refused), true, false),
            ),
            (
                "the BIR's name does not resolve",
                four_refused("failed to lookup address information: nodename nor servname provided, or not known", Some("dns")),
                "failed",
                all_four("the app refused to connect to dispatcher.test:443: failed to lookup address information"),
                (O::Failed, S::Dispatcher, W::Fail, W::None, Some(K::Dns), true, false),
            ),
            (
                "the app never answered: the user's side",
                vec![Says("live"), Open(1, 443), Open(2, 80), Open(3, 443), Open(4, 80)],
                "failed",
                all_four("the app did not connect to dispatcher.test:443 in time"),
                (O::Failed, S::Dispatcher, W::None, W::None, Some(K::NoAnswer), false, false),
            ),
            (
                "the front desk timed out, as the app says",
                four_refused("Connection timed out (os error 110)", None),
                "failed",
                all_four("the app refused to connect to dispatcher.test:443: Connection timed out (os error 110)"),
                (O::Failed, S::Dispatcher, W::Fail, W::None, Some(K::Timeout), true, false),
            ),
            (
                "TLS failed, then plain HTTP answered an error: the TLS stream decides",
                tls_then_http,
                "failed",
                tls_then_http_words.into(),
                (O::Failed, S::Dispatcher, W::Fail, W::None, Some(K::Tls), true, true),
            ),
            (
                "only plain HTTP tried: no front-desk evidence",
                vec![Says("live"), Open(1, 80), Refused(1, REFUSED, None)],
                "failed",
                "BIR's dispatcher did not answer: http://dispatcher.test: the app refused to connect to dispatcher.test:80: Connection refused (os error 61)".into(),
                (O::Failed, S::Dispatcher, W::None, W::None, Some(K::Refused), true, false),
            ),
            (
                "no upload service for this TIN",
                desk_only.clone(),
                "failed",
                "BIR's dispatcher did not answer: https://dispatcher.test: dispatcher has no upload server for this TIN (mode 0)".into(),
                (O::Failed, S::Dispatcher, W::None, W::None, Some(K::NoService), false, true),
            ),
            (
                "an answer the Sender cannot read",
                desk_only.clone(),
                "failed",
                "BIR's dispatcher did not answer: https://dispatcher.test: dispatcher answer was not understood: expected value".into(),
                (O::Failed, S::Dispatcher, W::None, W::None, Some(K::Protocol), false, true),
            ),
            (
                "a field that does not unwrap",
                desk_only.clone(),
                "failed",
                "BIR's dispatcher did not answer: https://dispatcher.test: dispatcher field did not unwrap: bad padding".into(),
                (O::Failed, S::Dispatcher, W::None, W::None, Some(K::Protocol), false, true),
            ),
            (
                "an unknown upload server",
                desk_only,
                "failed",
                "the dispatcher named an unknown upload server: elsewhere.test".into(),
                (O::Failed, S::Dispatcher, W::Ok, W::None, Some(K::UnknownHost), false, true),
            ),
            (
                "the mailbox refused",
                upload_refusal,
                "failed",
                upload_refused.into(),
                (O::Failed, S::Upload, W::Ok, W::Fail, Some(K::Refused), true, true),
            ),
            (
                "the app never answered the mailbox's open",
                upload_no_answer,
                "failed",
                upload_late.into(),
                (O::Failed, S::Upload, W::Ok, W::None, Some(K::NoAnswer), false, true),
            ),
            (
                "a host key that is not pinned",
                desk_then_upload(),
                "failed",
                "the SFTP server's host key is not pinned (see the log for its fingerprint)".into(),
                (O::Failed, S::Upload, W::Ok, W::None, Some(K::HostKey), false, true),
            ),
            ("SSH failed", desk_then_upload(), "failed", "SSH failed: Disconnected".into(), (O::Failed, S::Upload, W::Ok, W::Fail, Some(K::Ssh), true, true)),
            (
                "no common SSH algorithm: Buwiz's own settings",
                desk_then_upload(),
                "failed",
                "SSH failed: No common key exchange algorithm".into(),
                (O::Failed, S::Upload, W::Ok, W::None, Some(K::Protocol), false, true),
            ),
            (
                "not an SSH server answered: the user's side",
                desk_then_upload(),
                "failed",
                "SSH failed: invalid SSH version string".into(),
                (O::Failed, S::Upload, W::Ok, W::None, Some(K::Local), false, true),
            ),
            (
                "no service said over plain HTTP proves nothing; the TLS stream decides",
                vec![
                    Says("live"),
                    Open(1, 443),
                    Refused(1, REFUSED, None),
                    Open(2, 80),
                    Opened(2),
                    Bytes(2, vec![1; 200]),
                ],
                "failed",
                "BIR's dispatcher did not answer: https://dispatcher.test: the app refused to connect to dispatcher.test:443: Connection refused (os error 61); \
                 http://dispatcher.test: dispatcher has no upload server for this TIN (mode 0)"
                    .into(),
                (O::Failed, S::Dispatcher, W::Fail, W::None, Some(K::Refused), true, true),
            ),
            (
                "an answer not understood over plain HTTP only: the user's side",
                vec![Says("live"), Open(1, 80), Opened(1), Bytes(1, vec![1; 200])],
                "failed",
                "BIR's dispatcher did not answer: http://dispatcher.test: dispatcher answer was not understood: expected value".into(),
                (O::Failed, S::Dispatcher, W::None, W::None, Some(K::Local), false, true),
            ),
            (
                "a local refusal, then a later stream got to the mailbox",
                vec![
                    Says("live"),
                    Open(1, 443),
                    Refused(1, "Permission denied (os error 13)", None),
                    Open(2, 443),
                    Opened(2),
                    Bytes(2, vec![1; 200]),
                    Open(3, 22),
                    Refused(3, REFUSED, Some("refused")),
                ],
                "failed",
                upload_refused.into(),
                (O::Failed, S::Upload, W::Ok, W::Fail, Some(K::Refused), true, true),
            ),
            (
                "a trust-list refusal, then a later front desk answered",
                vec![
                    Says("live"),
                    Open(1, 443),
                    Refused(1, "dispatcher.test:443 is not on the signed trust list", None),
                    Open(2, 443),
                    Opened(2),
                    Bytes(2, vec![1; 40]),
                ],
                "failed",
                "BIR's dispatcher did not answer: https://dispatcher.test: the app refused to connect to dispatcher.test:443: dispatcher.test:443 is not on the signed trust list; \
                 https://backup.test: TLS: invalid peer certificate: UnknownIssuer"
                    .into(),
                (O::Failed, S::Dispatcher, W::Fail, W::None, Some(K::Tls), true, true),
            ),
            (
                "a local refusal after the last stream that got anywhere decides",
                vec![
                    Says("live"),
                    Open(1, 443),
                    Opened(1),
                    Bytes(1, vec![1; 40]),
                    Open(2, 443),
                    Refused(2, "Permission denied (os error 13)", None),
                ],
                "failed",
                String::new(),
                (O::Failed, S::Dispatcher, W::None, W::None, Some(K::Local), false, true),
            ),
            ("login refused", desk_then_upload(), "failed", "SFTP login was refused".into(), (O::Failed, S::Upload, W::Ok, W::Fail, Some(K::Login), true, true)),
            (
                "failed before the upload",
                desk_then_upload(),
                "failed",
                "SFTP failed before the upload started: channel closed".into(),
                (O::Failed, S::Upload, W::Ok, W::Fail, Some(K::Ssh), true, true),
            ),
            (
                "timed out before the upload",
                desk_then_upload(),
                "failed",
                "SFTP timed out before the upload started".into(),
                (O::Failed, S::Upload, W::Ok, W::Fail, Some(K::Timeout), true, true),
            ),
            (
                "v2 trust list refusal",
                vec![Says("live"), Open(1, 443), Refused(1, "dispatcher.test:443 is not on the signed trust list", None)],
                "failed",
                all_four("the app refused to connect to dispatcher.test:443: dispatcher.test:443 is not on the signed trust list"),
                (O::Failed, S::Dispatcher, W::None, W::None, Some(K::TrustList), false, false),
            ),
            (
                "v1 trust list refusal",
                vec![Says("live"), Open(1, 443), Refused(1, "dispatcher.test:443 is not a BIR server", None)],
                "failed",
                all_four("the app refused to connect to dispatcher.test:443: dispatcher.test:443 is not a BIR server"),
                (O::Failed, S::Dispatcher, W::None, W::None, Some(K::TrustList), false, false),
            ),
            (
                "trust list refusal by kind",
                vec![Says("live"), Open(1, 443), Refused(1, "not allowed", Some("not_listed"))],
                "failed",
                String::new(),
                (O::Failed, S::Dispatcher, W::None, W::None, Some(K::TrustList), false, false),
            ),
            (
                "the user's own network is down",
                four_refused("Network is unreachable (os error 51)", None),
                "failed",
                all_four("the app refused to connect to dispatcher.test:443: Network is unreachable (os error 51)"),
                (O::Failed, S::Dispatcher, W::None, W::None, Some(K::Local), false, false),
            ),
            (
                "the user's own system refused, by kind",
                four_refused("whatever", Some("denied")),
                "failed",
                String::new(),
                (O::Failed, S::Dispatcher, W::None, W::None, Some(K::Local), false, false),
            ),
            (
                "the user's network unreachable, by kind",
                four_refused("whatever", Some("unreachable")),
                "failed",
                String::new(),
                (O::Failed, S::Dispatcher, W::None, W::None, Some(K::Local), false, false),
            ),
            (
                "not built in",
                vec![Says("live")],
                "failed",
                crate::sender::NOT_BUILT_IN.into(),
                (O::Failed, S::None, W::None, W::None, Some(K::NotBuiltIn), false, false),
            ),
            (
                "the Sender unreachable",
                vec![SenderDown],
                "failed",
                "the sending service is not reachable; try again later".into(),
                (O::Failed, S::None, W::None, W::None, Some(K::SenderDown), false, false),
            ),
            (
                "the Sender stopped before connecting",
                vec![Says("live"), SenderLost],
                "failed",
                "the sending service stopped before connecting to BIR".into(),
                (O::Failed, S::None, W::None, W::None, Some(K::SenderDown), false, false),
            ),
            (
                "the Sender stopped during the upload",
                [desk_then_upload(), vec![SenderLost]].concat(),
                "unknown",
                "the send ended without an outcome; BIR may have received this return".into(),
                (O::Unknown, S::Upload, W::None, W::Maybe, Some(K::SenderDown), false, true),
            ),
            (
                "a return that is not its ticket's",
                vec![Says("live")],
                "failed",
                "The return does not match its ticket: the form is not the ticket's. (return_mismatch)".into(),
                (O::Failed, S::None, W::None, W::None, Some(K::Mismatch), false, false),
            ),
            (
                "a test TIN, refused by the Sender",
                vec![Says("live")],
                "failed",
                crate::filing::gate::Standing::TestOnly.failure(),
                (O::Failed, S::None, W::None, W::None, Some(K::Refusal), false, false),
            ),
            (
                "a spent ticket",
                vec![Says("live")],
                "failed",
                "the ticket was already used".into(),
                (O::Failed, S::None, W::None, W::None, Some(K::Ticket), false, false),
            ),
            (
                "could not encrypt",
                vec![Says("live")],
                "failed",
                "could not encrypt the return: bad passphrase".into(),
                (O::Failed, S::None, W::None, W::None, Some(K::Other), false, false),
            ),
            ("dry run", vec![Says("dry_run")], "dry_run", String::new(), (O::DryRun, S::None, W::None, W::None, None, false, false)),
            (
                "lab: sent, never counted",
                vec![Says("lab"), Open(1, 2222), Opened(1), Bytes(1, b"SSH-2.0-lab\r\n".to_vec())],
                "sent",
                String::new(),
                (O::Sent, S::Uploaded, W::None, W::Ok, None, false, false),
            ),
            (
                "lab: failed, no class",
                vec![Says("lab"), Open(1, 2222), Opened(1)],
                "failed",
                "SFTP login was refused".into(),
                (O::Failed, S::Upload, W::None, W::Fail, None, false, false),
            ),
            (
                "lab: no lab server set",
                vec![Says("lab")],
                "failed",
                "no lab SFTP server set".into(),
                (O::Failed, S::None, W::None, W::None, None, false, false),
            ),
            (
                "no sender frame: mode unknown, never counted",
                desk_then_upload()[1..].to_vec(),
                "sent",
                String::new(),
                (O::Sent, S::Uploaded, W::Ok, W::Ok, None, false, true),
            ),
            (
                "no sender frame, failed: like lab, no class, not counted",
                desk_then_upload()[1..].to_vec(),
                "failed",
                "SSH failed: Disconnected".into(),
                (O::Failed, S::Upload, W::Ok, W::Fail, None, false, true),
            ),
            (
                "no sender frame, the Sender stopped: no class",
                [desk_then_upload()[1..].to_vec(), vec![SenderLost]].concat(),
                "unknown",
                "the send ended without an outcome; BIR may have received this return".into(),
                (O::Unknown, S::Upload, W::None, W::Maybe, None, false, true),
            ),
        ];
        for (name, steps, status, error, expected) in cases {
            let observed = observe(&steps, status, &error);
            assert_eq!(row(&observed), expected, "{name}");
            let mode = observed.mode;
            if matches!(mode, Mode::Lab | Mode::DryRun) {
                assert_eq!(observed.class(), None, "{name}");
            }
        }
    }

    #[test]
    fn a_stream_unanswered_before_the_next_open_is_the_apps_side() {
        // Stream 1 got no answer before the Sender moved on; stream 2 was
        // refused. The first TLS stream's kind decides: no answer, which is
        // the user's side (every app answers within 10 s) and not counted.
        let steps = [Says("live"), Open(1, 443), Open(2, 80), Refused(2, REFUSED, None)];
        let observed = observe(&steps, "failed", "BIR's dispatcher did not answer: …");
        assert_eq!(
            (observed.kind, observed.class(), observed.counted),
            (Some(Kind::NoAnswer), Some(Class::UserNetwork), false)
        );
        // A late answer for a stream already unanswered changes nothing.
        let steps = [Says("live"), Open(1, 443), Open(2, 80), Opened(1), Refused(2, REFUSED, None)];
        assert_eq!(observe(&steps, "failed", "").kind, Some(Kind::NoAnswer));
    }

    #[test]
    fn frames_are_known_by_their_type_whatever_their_other_fields() {
        let mut observer = TryObserver::new(&[22]);
        // A `sender` frame with a mode that is not a string still stops
        // here; its mode is unknown.
        for odd in [r#"{"type":"sender","mode":5}"#, r#"{"type":"sender","mode":null,"v":"x"}"#] {
            assert!(!observer.sender_text(odd), "{odd}");
            assert!(is_sender_frame(odd), "{odd}");
        }
        assert!(!is_sender_frame(r#"{"type":5,"mode":"live"}"#));
        assert!(!is_sender_frame(r#"["sender"]"#));
        // A `refused` whose `kind` is not a string is still a refusal, read
        // from its words.
        assert!(observer.sender_text(r#"{"type":"open","stream":1,"host":"dispatcher.test","port":443}"#));
        observer.app_text(r#"{"type":"refused","stream":1,"reason":"Connection refused (os error 61)","kind":7}"#);
        assert!(observer.sender_text(r#"{"type":"open","stream":2,"port":443,"host":["odd"]}"#));
        observer.app_text(r#"{"type":"refused","stream":2,"reason":{"odd":true},"kind":"refused"}"#);
        let observed = observer.finish("failed", Some("BIR's dispatcher did not answer: …"));
        assert_eq!(observed.mode, Mode::Unknown);
        assert_eq!(observed.opens, 2);
        assert_eq!(observed.classified.kind, Some(Kind::Refused));
        // Mode unknown: nothing told.
        assert_eq!(observed.kind, None);
    }

    #[test]
    fn bytes_before_opened_mean_it_opened() {
        let steps = [
            Says("live"),
            Open(1, 443),
            Bytes(1, vec![1; 100]),
            Opened(1),
            Open(2, 22),
            Bytes(2, b"SSH-2.0-x\r\n".to_vec()),
            Opened(2),
        ];
        let observed = observe(&steps, "failed", "SSH failed: Disconnected");
        assert_eq!(
            (observed.classified.dispatcher, observed.classified.upload, observed.kind),
            (Window::Ok, Window::Fail, Some(Kind::Ssh))
        );
        assert!(observed.touched_bir);
    }

    #[test]
    fn the_first_sender_frame_says_the_mode_and_none_is_forwarded() {
        let mut observer = TryObserver::new(&[22]);
        assert!(!observer.sender_text(r#"{"type":"sender","mode":"lab"}"#));
        assert!(!observer.sender_text(r#"{"type":"sender","mode":"live"}"#));
        assert!(!observer.sender_text(r#"{"type":"sender"}"#));
        assert!(observer.sender_text(r#"{"type":"close","stream":1}"#));
        assert!(observer.sender_text("not json"));
        assert_eq!(observer.finish("dry_run", None).mode, Mode::Lab);
        assert!(is_sender_frame(r#"{"mode":"live","type":"sender","v":2}"#));
        assert!(!is_sender_frame(r#"{"type":"result","status":"sent"}"#));
        // A mode it does not know is unknown.
        let mut observer = TryObserver::new(&[22]);
        observer.sender_text(r#"{"type":"sender","mode":"orbit"}"#);
        assert_eq!(observer.finish("sent", None).mode, Mode::Unknown);
    }

    #[test]
    fn the_upload_servers_host_key_is_read_from_its_handshake() {
        let key = ed25519_dalek::SigningKey::from_bytes(&[11; 32]);
        let string = |bytes: &[u8]| [(bytes.len() as u32).to_be_bytes().as_slice(), bytes].concat();
        let blob = [string(b"ssh-ed25519"), string(key.verifying_key().as_bytes())].concat();
        let reply = [vec![31], string(&blob), string(&[9; 32]), string(&[1; 83])].concat();
        let packet = |payload: &[u8]| {
            let padding = 8 - (payload.len() + 5) % 8 + 8;
            [
                ((payload.len() + padding + 1) as u32).to_be_bytes().as_slice(),
                &[padding as u8],
                payload,
                &vec![0; padding],
            ]
            .concat()
        };
        let handshake = [b"SSH-2.0-upload\r\n".to_vec(), packet(&[20; 40]), packet(&reply), packet(&[21])].concat();
        let (first, rest) = handshake.split_at(30);
        let mut steps = desk_then_upload();
        steps.truncate(6);
        steps.push(Bytes(2, first.to_vec()));
        steps.push(Bytes(2, rest.to_vec()));
        let observed =
            observe(&steps, "failed", "the SFTP server's host key is not pinned (see the log for its fingerprint)");
        assert_eq!(observed.host_key, Some(sshkex::fingerprint(&blob)));
        assert_eq!(observed.kind, Some(Kind::HostKey));
        // A key that is not pinned: the Sender stopped before completing
        // the key exchange.
        assert!(!observed.key_exchange_done);
        // The Sender completes it in a try that reaches the login or the
        // upload, never in one that fails before.
        for (status, error, done) in [
            ("sent", "", true),
            ("unknown", "SFTP failed during the upload: connection reset.", true),
            ("failed", "SFTP login was refused", true),
            ("failed", "SSH failed: wrong server signature", false),
            ("failed", "SSH failed: Disconnected", false),
            ("failed", "SFTP timed out before the upload started", false),
        ] {
            assert_eq!(observe(&steps, status, error).key_exchange_done, done, "{status} {error}");
        }
        let lost = [steps.clone(), vec![SenderLost]].concat();
        assert!(!observe(&lost, "unknown", "the send ended without an outcome").key_exchange_done);
        // The same bytes on a front-desk stream are not read.
        let steps = [Says("live"), Open(1, 443), Opened(1), Bytes(1, handshake.clone())];
        assert_eq!(observe(&steps, "failed", "").host_key, None);
    }

    #[test]
    fn frames_it_cannot_read_change_nothing() {
        let mut observer = TryObserver::new(&[22]);
        observer.sender_text(r#"{"type":"sender","mode":"live"}"#);
        observer.sender_text(r#"{"type":"open","stream":300,"port":443}"#);
        observer.sender_text(r#"{"type":"open","stream":1}"#);
        observer.app_text(r#"{"type":"opened"}"#);
        observer.app_text(r#"{"type":"opened","stream":9}"#);
        observer.app_text("]");
        observer.app_binary(&[]);
        observer.app_binary(&[9, 1, 2, 3]);
        let observed = observer.finish("failed", Some(crate::sender::NOT_BUILT_IN));
        assert_eq!((observed.opens, observed.kind), (0, Some(Kind::NotBuiltIn)));
    }
}
