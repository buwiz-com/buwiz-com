//! What one try tells about the BIR (docs/bir-uptime.md §14.2): how far it
//! got (`stage`), what each window did, and, when it did not get in, whose
//! side it failed on (`class`) and how (`kind`). Pure: it reads what
//! [`crate::uptime::observe`] saw and the words of the Sender's result.
//!
//! The words come from the Sender (`src/sender/mod.rs`, the private BIR
//! relay), from the pipe (`src/relay/mod.rs`), from the Gatekeeper's bridge
//! (`src/filing/submissions.rs`) and from the apps' `refused` reasons (the
//! OS's connect errors and the trust-list refusals).

/// The relay mode the Sender said in its first frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Live,
    Lab,
    DryRun,
    /// No `sender` frame: never counted.
    Unknown,
}

impl Mode {
    pub fn parse(raw: &str) -> Self {
        match raw {
            "live" => Self::Live,
            "lab" => Self::Lab,
            "dry_run" => Self::DryRun,
            _ => Self::Unknown,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Lab => "lab",
            Self::DryRun => "dry_run",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Sent,
    DryRun,
    Failed,
    Unknown,
}

impl Outcome {
    /// A result's `status`. Anything but the four is a failure.
    pub fn parse(status: &str) -> Self {
        match status {
            "sent" => Self::Sent,
            "dry_run" => Self::DryRun,
            "unknown" => Self::Unknown,
            _ => Self::Failed,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sent => "sent",
            Self::DryRun => "dry_run",
            Self::Failed => "failed",
            Self::Unknown => "unknown",
        }
    }
}

/// How far a try got.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    None,
    Dispatcher,
    Upload,
    Uploaded,
}

impl Stage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Dispatcher => "dispatcher",
            Self::Upload => "upload",
            Self::Uploaded => "uploaded",
        }
    }
}

/// What a window (the front desk or the mailbox) did in a try.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Window {
    Ok,
    Fail,
    /// The upload broke midway: the return may have landed.
    Maybe,
    /// Not reached, or nothing learned about it.
    None,
}

impl Window {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Fail => "fail",
            Self::Maybe => "maybe",
            Self::None => "none",
        }
    }
}

/// Whose side a failed try failed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Bir,
    UserNetwork,
    Ours,
}

impl Class {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bir => "bir",
            Self::UserNetwork => "user_network",
            Self::Ours => "ours",
        }
    }
}

/// How a try failed. Each kind has one class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    // The BIR's side.
    Dns,
    Refused,
    Timeout,
    Reset,
    Tls,
    Http,
    Ssh,
    Login,
    /// The upload broke midway (`unknown`).
    Cut,
    /// The front desk has no upload server for this TIN (mode 0). Evidence
    /// for no light.
    NoService,
    // The user's own connection.
    Local,
    AppGone,
    /// The app never answered an `open` (every app answers within 10 s).
    NoAnswer,
    // Buwiz's side.
    HostKey,
    UnknownHost,
    Protocol,
    TrustList,
    SenderDown,
    NotBuiltIn,
    Refusal,
    Mismatch,
    Ticket,
    Other,
}

impl Kind {
    pub const ALL: [Kind; 23] = [
        Self::Dns,
        Self::Refused,
        Self::Timeout,
        Self::Reset,
        Self::Tls,
        Self::Http,
        Self::Ssh,
        Self::Login,
        Self::Cut,
        Self::NoService,
        Self::Local,
        Self::AppGone,
        Self::NoAnswer,
        Self::HostKey,
        Self::UnknownHost,
        Self::Protocol,
        Self::TrustList,
        Self::SenderDown,
        Self::NotBuiltIn,
        Self::Refusal,
        Self::Mismatch,
        Self::Ticket,
        Self::Other,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dns => "dns",
            Self::Refused => "refused",
            Self::Timeout => "timeout",
            Self::Reset => "reset",
            Self::Tls => "tls",
            Self::Http => "http",
            Self::Ssh => "ssh",
            Self::Login => "login",
            Self::Cut => "cut",
            Self::NoService => "no_service",
            Self::Local => "local",
            Self::AppGone => "app_gone",
            Self::NoAnswer => "no_answer",
            Self::HostKey => "host_key",
            Self::UnknownHost => "unknown_host",
            Self::Protocol => "protocol",
            Self::TrustList => "trust_list",
            Self::SenderDown => "sender_down",
            Self::NotBuiltIn => "not_built_in",
            Self::Refusal => "refusal",
            Self::Mismatch => "mismatch",
            Self::Ticket => "ticket",
            Self::Other => "other",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == raw)
    }

    pub fn class(self) -> Class {
        match self {
            Self::Dns
            | Self::Refused
            | Self::Timeout
            | Self::Reset
            | Self::Tls
            | Self::Http
            | Self::Ssh
            | Self::Login
            | Self::Cut
            | Self::NoService => Class::Bir,
            Self::Local | Self::AppGone | Self::NoAnswer => Class::UserNetwork,
            Self::HostKey
            | Self::UnknownHost
            | Self::Protocol
            | Self::TrustList
            | Self::SenderDown
            | Self::NotBuiltIn
            | Self::Refusal
            | Self::Mismatch
            | Self::Ticket
            | Self::Other => Class::Ours,
        }
    }
}

fn has(text: &str, any: &[&str]) -> bool {
    any.iter().any(|needle| text.contains(needle))
}

/// The number of an OS error as Rust words it, `… (os error N)`. The same
/// on every OS and in every language the OS speaks.
fn os_error(reason: &str) -> Option<i32> {
    let (_, rest) = reason.rsplit_once("(os error ")?;
    rest.strip_suffix(')').or_else(|| rest.split_once(')').map(|(code, _)| code))?.trim().parse().ok()
}

/// An OS connect error by its number: the macOS, Linux and Windows
/// (Winsock) numbers of each. `None` for one it does not know.
fn os_error_kind(code: i32) -> Option<Kind> {
    Some(match code {
        // ECONNREFUSED.
        61 | 111 | 10061 => Kind::Refused,
        // ETIMEDOUT.
        60 | 110 | 10060 => Kind::Timeout,
        // EHOSTUNREACH, EHOSTDOWN: the BIR's side, as the user reached
        // Buwiz, so their network works.
        65 | 113 | 10065 | 64 | 112 | 10064 => Kind::Refused,
        // ECONNRESET.
        54 | 104 | 10054 => Kind::Reset,
        // ENETUNREACH, ENETDOWN: the user's own network.
        51 | 101 | 10051 | 50 | 100 | 10050 => Kind::Local,
        // EACCES, EPERM: the user's own system refused.
        13 | 1 | 10013 => Kind::Local,
        // WSAHOST_NOT_FOUND, WSATRY_AGAIN, WSANO_DATA.
        11001 | 11002 | 11004 => Kind::Dns,
        _ => return None,
    })
}

/// What an app's `refused` says: its `kind` when it sent one it knows,
/// else a trust-list refusal, else the OS error's number, else its words
/// (the OS's connect errors in English). A reason it cannot read is the
/// user's own side: such a try never counts.
pub fn refusal_kind(reason: &str, kind: Option<&str>) -> Kind {
    match kind {
        Some("dns") => return Kind::Dns,
        Some("refused") => return Kind::Refused,
        Some("timeout") => return Kind::Timeout,
        Some("unreachable" | "denied") => return Kind::Local,
        Some("not_listed") => return Kind::TrustList,
        _ => {}
    }
    let reason = reason.to_ascii_lowercase();
    if has(&reason, &["is not on the signed trust list", "is not a bir server"]) {
        return Kind::TrustList;
    }
    if let Some(kind) = os_error(&reason).and_then(os_error_kind) {
        return kind;
    }
    if has(
        &reason,
        &[
            // The user's own system refused.
            "permission denied",
            "forbidden by its access permissions",
            "operation not permitted",
            // The user's own network is down.
            "network is unreachable",
            "unreachable network",
            "network is down",
        ],
    ) {
        Kind::Local
    } else if has(&reason, &["no route to host", "unreachable host", "host is unreachable", "host is down"]) {
        // The BIR's side: the user reached Buwiz, so their network works.
        Kind::Refused
    } else if has(&reason, &["connection refused", "actively refused"]) {
        Kind::Refused
    } else if has(&reason, &["connection timed out", "operation timed out", "did not properly respond"]) {
        Kind::Timeout
    } else if has(
        &reason,
        &[
            "failed to lookup address information",
            "nodename nor servname provided",
            "name or service not known",
            "no such host is known",
            "temporary failure in name resolution",
            "no address associated with hostname",
        ],
    ) {
        Kind::Dns
    } else if has(&reason, &["connection reset"]) {
        Kind::Reset
    } else {
        Kind::Local
    }
}

/// The pipe's own words for a stream that did not open
/// (`crate::relay::PipeError`), as the Sender repeats them.
fn pipe_kind(reason: &str) -> Option<Kind> {
    if reason.contains("the app disconnected") {
        return Some(Kind::AppGone);
    }
    if let Some(rest) = reason.split_once("the app refused to connect to ").map(|(_, rest)| rest) {
        // `host:port: reason`.
        let os = rest.split_once(": ").map_or(rest, |(_, os)| os);
        return Some(refusal_kind(os, None));
    }
    if reason.contains("the app did not connect to ") && reason.contains("in time") {
        // The app never answered the `open`.
        return Some(Kind::NoAnswer);
    }
    None
}

/// One front-desk attempt's words, from the Sender's "BIR's dispatcher did
/// not answer: …" list.
pub fn dispatcher_kind(reason: &str) -> Kind {
    if let Some(kind) = pipe_kind(reason) {
        return kind;
    }
    let lower = reason.to_ascii_lowercase();
    if lower.contains("no upload server for this tin (mode 0)") {
        Kind::NoService
    } else if has(&lower, &["answer was not understood", "field did not unwrap"]) {
        Kind::Protocol
    } else if lower.starts_with("tls:") {
        if lower.contains("reset") { Kind::Reset } else { Kind::Tls }
    } else if lower.starts_with("dispatcher request failed") {
        if has(&lower, &["timed out", "timeout"]) {
            Kind::Timeout
        } else if lower.contains("reset") {
            Kind::Reset
        } else {
            Kind::Http
        }
    } else if lower.starts_with("dispatcher answered") {
        Kind::Http
    } else {
        Kind::Other
    }
}

/// An opened front-desk stream's kind when the attempts in the words do not
/// line up with the streams: the first failure the words name.
fn dispatcher_kind_anywhere(lower: &str) -> Kind {
    let at = |needle: &str| lower.find(needle).unwrap_or(usize::MAX);
    let (tls, request, answered) = (at("tls:"), at("dispatcher request failed"), at("dispatcher answered"));
    let first = tls.min(request).min(answered);
    if first == usize::MAX { Kind::Other } else { dispatcher_kind(&lower[first..]) }
}

/// The front-desk attempts named in a result's error, in order: (over
/// HTTPS, the reason). Empty when the error is not that list.
pub fn dispatcher_attempts(error: &str) -> Vec<(bool, &str)> {
    const PREFIX: &str = "BIR's dispatcher did not answer: ";
    let Some(list) = error.find(PREFIX).map(|at| &error[at + PREFIX.len()..]) else {
        return Vec::new();
    };
    // Each attempt starts with its scheme; a reason may hold "; " itself.
    let mut starts: Vec<usize> = Vec::new();
    for (at, _) in list.match_indices("http") {
        let begins = at == 0 || list[..at].ends_with("; ");
        if begins && (list[at..].starts_with("https://") || list[at..].starts_with("http://")) {
            starts.push(at);
        }
    }
    let mut attempts = Vec::new();
    for (index, &start) in starts.iter().enumerate() {
        let end = starts.get(index + 1).map_or(list.len(), |next| next - 2);
        let attempt = &list[start..end];
        let tls = attempt.starts_with("https://");
        let after_scheme = attempt.split_once("://").map_or(attempt, |(_, rest)| rest);
        let reason = after_scheme.split_once(": ").map_or("", |(_, reason)| reason);
        attempts.push((tls, reason));
    }
    attempts
}

/// After the upload server was opened: what the Sender's words say. `None`
/// when they name no upload failure.
pub fn upload_kind(error: &str) -> Option<Kind> {
    let lower = error.to_ascii_lowercase();
    if lower.contains("host key is not pinned") {
        Some(Kind::HostKey)
    } else if lower.contains("no common") && lower.contains("algorithm") {
        // The Sender and the server share no algorithm: Buwiz's own SSH
        // settings, which still touched the BIR.
        Some(Kind::Protocol)
    } else if lower.contains("invalid ssh version string") {
        // What answered is not an SSH server: something on the user's way
        // (a proxy, a captive portal) took the connection.
        Some(Kind::Local)
    } else if lower.contains("sftp login was refused") {
        Some(Kind::Login)
    } else if lower.contains("sftp timed out") {
        Some(Kind::Timeout)
    } else if has(&lower, &["ssh failed", "sftp failed before the upload started"]) {
        Some(Kind::Ssh)
    } else {
        None
    }
}

/// A try that opened nothing: whose side, from the words.
pub fn unopened_kind(error: &str) -> Kind {
    let lower = error.to_ascii_lowercase();
    if lower.contains(crate::sender::NOT_BUILT_IN) {
        Kind::NotBuiltIn
    } else if has(&lower, &["the sending service is not reachable", "the sending service stopped"]) {
        Kind::SenderDown
    } else if lower.contains("(return_mismatch)") || lower.contains("does not match its") {
        Kind::Mismatch
    } else if is_refusal(error) {
        Kind::Refusal
    } else if has(&lower, &["ticket", "sealed", "envelope", "the job"]) || lower.contains("did not understand the job")
    {
        Kind::Ticket
    } else {
        Kind::Other
    }
}

/// A TIN or account refusal: its code in brackets at the end, or the test
/// TIN's words.
fn is_refusal(error: &str) -> bool {
    let trimmed = error.trim_end();
    let bracketed = trimmed
        .strip_suffix(')')
        .and_then(|head| head.rsplit_once('('))
        .is_some_and(|(_, code)| code.starts_with("tin_") || code == "account_banned");
    bracketed || error.contains(crate::filing::gate::TEST_ONLY_MESSAGE)
}

/// What [`crate::uptime::observe`] saw of one stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamFacts {
    /// The upload window's stream; else the front desk's.
    pub upload: bool,
    /// A front-desk stream on port 443.
    pub tls: bool,
    pub answer: Answer,
    /// Bytes from the far end.
    pub bytes: u64,
}

/// How the app answered an `open`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Opened,
    Refused(Kind),
    /// No answer before the Sender moved on (its 20 s timeout).
    NoAnswer,
}

/// Everything one try showed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seen<'a> {
    pub mode: Mode,
    pub status: &'a str,
    pub error: &'a str,
    /// In the order the Sender opened them.
    pub streams: &'a [StreamFacts],
    /// The app's socket closed before the result.
    pub app_gone: bool,
    /// The Sender could not be reached: no job went out.
    pub sender_down: bool,
    /// The Sender's socket ended without a result; the outcome is the
    /// Gatekeeper's own.
    pub sender_lost: bool,
    /// The app went away and the Sender's outcome did not come within the
    /// 60 s the Gatekeeper waits for it; the outcome is the Gatekeeper's
    /// own.
    pub wait_expired: bool,
}

/// What the record keeps of a try.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Classified {
    pub outcome: Outcome,
    pub stage: Stage,
    pub dispatcher: Window,
    pub upload: Window,
    /// `None` for a try that got in.
    pub kind: Option<Kind>,
    /// Some stream reached a server.
    pub opened: bool,
}

impl Classified {
    pub fn class(&self) -> Option<Class> {
        self.kind.map(Kind::class)
    }
}

/// Names a try (docs/bir-uptime.md §14.2). Precedence: `sent`; an
/// `unknown` result from the Sender (the user's side when the app also went
/// away); the Sender lost (the app's side when the outcome wait expired
/// after the app went away); the app gone; the table.
pub fn classify(seen: &Seen<'_>) -> Classified {
    let outcome = Outcome::parse(seen.status);
    let streams = seen.streams;
    let opened = streams.iter().any(|stream| stream.answer == Answer::Opened);
    let first_upload = streams.iter().position(|stream| stream.upload);
    let upload_opened = streams.iter().any(|stream| stream.upload && stream.answer == Answer::Opened);
    let stage = if outcome == Outcome::Sent {
        Stage::Uploaded
    } else if first_upload.is_some() {
        Stage::Upload
    } else if !streams.is_empty() {
        Stage::Dispatcher
    } else {
        Stage::None
    };
    // The front-desk stream that answered: the last one with bytes before
    // the upload was opened (or, with no upload, the last one with bytes).
    let answered = |before: usize| {
        streams[..before]
            .iter()
            .rev()
            .find(|stream| !stream.upload && stream.bytes > 0 && stream.answer == Answer::Opened)
    };
    let front_desk_ok = |before: usize| match answered(before) {
        Some(stream) if stream.tls => Window::Ok,
        _ => Window::None,
    };
    let answered_desk = first_upload.map_or(Window::None, front_desk_ok);
    let made = |kind: Option<Kind>, dispatcher: Window, upload: Window| Classified {
        outcome,
        stage,
        dispatcher,
        upload,
        kind,
        opened,
    };

    let lost = seen.sender_lost || seen.wait_expired;
    match outcome {
        Outcome::Sent => return made(None, answered_desk, Window::Ok),
        Outcome::DryRun => return made(None, Window::None, Window::None),
        // The Sender's own `unknown`: the upload broke midway. When the
        // app went away it broke on the user's side.
        Outcome::Unknown if !lost => {
            let kind = if seen.app_gone { Kind::AppGone } else { Kind::Cut };
            return made(Some(kind), answered_desk, Window::Maybe);
        }
        _ => {}
    }
    if lost {
        let upload = if upload_opened { Window::Maybe } else { Window::None };
        let kind = if seen.wait_expired && seen.app_gone { Kind::AppGone } else { Kind::SenderDown };
        return made(Some(kind), Window::None, upload);
    }
    if seen.app_gone {
        return made(Some(Kind::AppGone), Window::None, Window::None);
    }
    if seen.sender_down {
        return made(Some(Kind::SenderDown), Window::None, Window::None);
    }
    let lower = seen.error.to_ascii_lowercase();
    if streams.is_empty() || has(&lower, &["could not encrypt", "no lab sftp server set"]) {
        let kind = if streams.is_empty() { unopened_kind(seen.error) } else { Kind::Other };
        return made(Some(kind), Window::None, Window::None);
    }
    // A refusal that is local or from the trust list decides the try only
    // when no later stream got further (opened, or was the upload).
    let decisive =
        |stream: &StreamFacts| matches!(stream.answer, Answer::Refused(Kind::TrustList) | Answer::Refused(Kind::Local));
    let got_further = streams
        .iter()
        .rposition(decisive)
        .is_some_and(|at| streams[at + 1..].iter().any(|stream| stream.upload || stream.answer == Answer::Opened));
    if !got_further {
        let refused = |wanted: Kind| streams.iter().any(|stream| stream.answer == Answer::Refused(wanted));
        if refused(Kind::TrustList) {
            return made(Some(Kind::TrustList), Window::None, Window::None);
        }
        if refused(Kind::Local) {
            return made(Some(Kind::Local), Window::None, Window::None);
        }
    }
    if let Some(at) = first_upload {
        // The last upload stream decides (there is one).
        let stream = streams.iter().rev().find(|stream| stream.upload).copied().unwrap_or(streams[at]);
        return match stream.answer {
            Answer::Refused(kind) if kind.class() == Class::Bir => made(Some(kind), answered_desk, Window::Fail),
            Answer::Refused(kind) => made(Some(kind), answered_desk, Window::None),
            Answer::NoAnswer => made(Some(Kind::NoAnswer), answered_desk, Window::None),
            Answer::Opened => match upload_kind(seen.error) {
                Some(kind) if kind.class() == Class::Bir => made(Some(kind), answered_desk, Window::Fail),
                Some(kind) => made(Some(kind), answered_desk, Window::None),
                None => made(Some(Kind::Other), answered_desk, Window::None),
            },
        };
    }

    // The front desk only.
    if lower.contains("the dispatcher named an unknown upload server") {
        return made(Some(Kind::UnknownHost), front_desk_ok(streams.len()), Window::None);
    }
    let attempts = dispatcher_attempts(seen.error);
    // "No service" and "not understood" are evidence only over TLS: on
    // plain HTTP the answer may come from a middlebox on the user's way, so
    // it is read as the user's side.
    let words: Vec<Kind> = attempts
        .iter()
        .map(|(tls, reason)| match dispatcher_kind(reason) {
            Kind::NoService | Kind::Protocol if !tls => Kind::Local,
            kind => kind,
        })
        .collect();
    if words.contains(&Kind::NoService) {
        return made(Some(Kind::NoService), Window::None, Window::None);
    }
    if words.contains(&Kind::Protocol) {
        return made(Some(Kind::Protocol), Window::None, Window::None);
    }
    // Each stream's own kind: from its answer, or for one that opened from
    // its attempt's words when they line up one for one. A local or
    // trust-list refusal that a later stream got past says nothing.
    let lined_up =
        attempts.len() == streams.len() && attempts.iter().zip(streams).all(|((tls, _), stream)| *tls == stream.tls);
    let kinds: Vec<(bool, Kind)> = streams
        .iter()
        .enumerate()
        .filter(|(_, stream)| !(got_further && decisive(stream)))
        .map(|(index, stream)| {
            let kind = match stream.answer {
                Answer::Refused(kind) => kind,
                Answer::NoAnswer => Kind::NoAnswer,
                Answer::Opened if lined_up => words[index],
                Answer::Opened => match dispatcher_kind_anywhere(&lower) {
                    Kind::NoService | Kind::Protocol if !stream.tls => Kind::Local,
                    kind => kind,
                },
            };
            (stream.tls, kind)
        })
        .collect();
    let tls_tried = streams.iter().any(|stream| stream.tls);
    let pick = kinds.iter().find(|(tls, _)| *tls).or(kinds.first()).map(|(_, kind)| *kind).unwrap_or(Kind::Other);
    if pick.class() != Class::Bir {
        return made(Some(pick), Window::None, Window::None);
    }
    made(Some(pick), if tls_tried { Window::Fail } else { Window::None }, Window::None)
}

/// Whether the Sender went on to complete the SSH key exchange in this try,
/// from its outcome: a try that got in, broke during the upload (the
/// Sender's own `unknown`) or reached the login did; "host key is not
/// pinned", "wrong server signature" and every earlier failure did not.
pub fn key_exchange_done(seen: &Seen<'_>) -> bool {
    match Outcome::parse(seen.status) {
        Outcome::Sent => true,
        Outcome::Unknown => !(seen.sender_lost || seen.wait_expired),
        Outcome::DryRun => false,
        Outcome::Failed => upload_kind(seen.error) == Some(Kind::Login),
    }
}

/// Whether a try moves the sign: a `live` try that got in, or failed on
/// the BIR's side, but not "no upload service for this TIN".
pub fn counted(mode: Mode, classified: &Classified) -> bool {
    mode == Mode::Live
        && (classified.outcome == Outcome::Sent || classified.class() == Some(Class::Bir))
        && classified.kind != Some(Kind::NoService)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_has_a_name_the_table_takes_and_reads_back() {
        for kind in Kind::ALL {
            let name = kind.as_str();
            assert!((1..=20).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'));
            assert_eq!(Kind::parse(name), Some(kind));
        }
        assert_eq!(Kind::parse("nope"), None);
        // The kinds a direct app may report, docs/bir-uptime.md §7.
        for (name, class) in [
            ("dns", Class::Bir),
            ("refused", Class::Bir),
            ("timeout", Class::Bir),
            ("reset", Class::Bir),
            ("tls", Class::Bir),
            ("http", Class::Bir),
            ("ssh", Class::Bir),
            ("login", Class::Bir),
            ("cut", Class::Bir),
            ("no_service", Class::Bir),
            ("host_key", Class::Ours),
            ("unknown_host", Class::Ours),
            ("protocol", Class::Ours),
            ("local", Class::UserNetwork),
            ("no_answer", Class::UserNetwork),
            ("other", Class::Ours),
        ] {
            assert_eq!(Kind::parse(name).map(Kind::class), Some(class), "{name}");
        }
    }

    /// Every refusal an app sends: the OS's connect errors on macOS, Linux
    /// and Windows, the apps' own timeout, and both trust-list refusals.
    #[test]
    fn refusals_read_from_their_words() {
        for (reason, kind) in [
            ("Connection refused (os error 61)", Kind::Refused),
            ("Connection refused (os error 111)", Kind::Refused),
            (
                "No connection could be made because the target machine actively refused it. (os error 10061)",
                Kind::Refused,
            ),
            ("No route to host (os error 65)", Kind::Refused),
            ("No route to host (os error 113)", Kind::Refused),
            ("A socket operation was attempted to an unreachable host. (os error 10065)", Kind::Refused),
            ("Host is unreachable (os error 113)", Kind::Refused),
            ("connection timed out", Kind::Timeout),
            ("Operation timed out (os error 60)", Kind::Timeout),
            ("Connection timed out (os error 110)", Kind::Timeout),
            (
                "A connection attempt failed because the connected party did not properly respond after a period of time, or established connection failed because connected host has failed to respond. (os error 10060)",
                Kind::Timeout,
            ),
            ("failed to lookup address information: nodename nor servname provided, or not known", Kind::Dns),
            ("failed to lookup address information: Name or service not known", Kind::Dns),
            ("failed to lookup address information: Temporary failure in name resolution", Kind::Dns),
            ("No such host is known. (os error 11001)", Kind::Dns),
            ("Connection reset by peer (os error 54)", Kind::Reset),
            ("Network is unreachable (os error 51)", Kind::Local),
            ("Network is unreachable (os error 101)", Kind::Local),
            ("A socket operation was attempted to an unreachable network. (os error 10051)", Kind::Local),
            ("Permission denied (os error 13)", Kind::Local),
            (
                "An attempt was made to access a socket in a way forbidden by its access permissions. (os error 10013)",
                Kind::Local,
            ),
            ("Operation not permitted (os error 1)", Kind::Local),
            ("Host is down (os error 64)", Kind::Refused),
            ("Host is down (os error 112)", Kind::Refused),
            ("Host is down", Kind::Refused),
            ("Network is down (os error 50)", Kind::Local),
            ("dispatcher.test:443 is not on the signed trust list", Kind::TrustList),
            ("dispatcher.test:443 is not a BIR server", Kind::TrustList),
            ("something no OS says", Kind::Local),
            ("", Kind::Local),
        ] {
            assert_eq!(refusal_kind(reason, None), kind, "{reason}");
        }
    }

    /// The OS error's number reads the same in any language the OS speaks.
    #[test]
    fn os_error_numbers_win_over_words() {
        for (reason, kind) in [
            (
                "Es konnte keine Verbindung hergestellt werden, da der Zielcomputer die Verbindung verweigerte. (os error 10061)",
                Kind::Refused,
            ),
            ("Ein Verbindungsversuch ist fehlgeschlagen. (os error 10060)", Kind::Timeout),
            ("Ce h\u{f4}te est inconnu. (os error 11001)", Kind::Dns),
            ("Le r\u{e9}seau est inaccessible (os error 10051)", Kind::Local),
            ("Conexi\u{f3}n rehusada (os error 111)", Kind::Refused),
            ("Tiempo de espera agotado (os error 110)", Kind::Timeout),
            ("No hay ruta hacia el host (os error 113)", Kind::Refused),
            ("Conexi\u{f3}n reinicializada por la m\u{e1}quina remota (os error 104)", Kind::Reset),
            ("Permiso denegado (os error 13)", Kind::Local),
            // A number it does not know falls back to the words.
            ("Connection refused (os error 9999)", Kind::Refused),
            ("(os error 9999)", Kind::Local),
            ("(os error x)", Kind::Local),
        ] {
            assert_eq!(refusal_kind(reason, None), kind, "{reason}");
        }
    }

    /// Words an app's own text might hold are not read as an OS's.
    #[test]
    fn loose_words_are_not_read_as_a_connect_error() {
        for reason in [
            "the app refused this host",
            "timeout waiting for the person",
            "lookup of the trust list is off",
            "not known to this app",
            "reset by the person",
        ] {
            assert_eq!(refusal_kind(reason, None), Kind::Local, "{reason}");
        }
    }

    #[test]
    fn a_refusals_kind_wins_over_its_words() {
        let words = "Connection refused (os error 61)";
        for (kind, expected) in [
            ("dns", Kind::Dns),
            ("refused", Kind::Refused),
            ("timeout", Kind::Timeout),
            ("unreachable", Kind::Local),
            ("denied", Kind::Local),
            ("not_listed", Kind::TrustList),
        ] {
            assert_eq!(refusal_kind(words, Some(kind)), expected, "{kind}");
        }
        // A kind it does not know falls back to the words.
        assert_eq!(refusal_kind(words, Some("future")), Kind::Refused);
    }

    #[test]
    fn front_desk_attempts_split_and_read() {
        let error = "BIR's dispatcher did not answer: https://dispatcher.test: TLS: invalid peer certificate: UnknownIssuer; \
                     http://dispatcher.test: dispatcher answered 503 Service Unavailable; \
                     https://backup.test: dispatcher request failed: timed out; \
                     http://backup.test: the app refused to connect to backup.test:80: Connection refused (os error 61)";
        let attempts = dispatcher_attempts(error);
        assert_eq!(attempts.iter().map(|(tls, _)| *tls).collect::<Vec<_>>(), [true, false, true, false]);
        let kinds: Vec<Kind> = attempts.iter().map(|(_, reason)| dispatcher_kind(reason)).collect();
        assert_eq!(kinds, [Kind::Tls, Kind::Http, Kind::Timeout, Kind::Refused]);
        assert!(dispatcher_attempts("the app disconnected").is_empty());
        for (reason, kind) in [
            ("dispatcher has no upload server for this TIN (mode 0)", Kind::NoService),
            ("dispatcher answer was not understood: expected value at line 1 column 1", Kind::Protocol),
            ("dispatcher field did not unwrap: bad padding", Kind::Protocol),
            ("dispatcher request failed: connection error: Connection reset by peer", Kind::Reset),
            ("dispatcher request failed: connection closed before message completed", Kind::Http),
            ("TLS: Connection reset by peer (os error 54)", Kind::Reset),
            ("the app disconnected", Kind::AppGone),
            ("the app did not connect to dispatcher.test:443 in time", Kind::NoAnswer),
            (
                "the app refused to connect to dispatcher.test:443: failed to lookup address information: Name or service not known",
                Kind::Dns,
            ),
            ("invalid dnsname", Kind::Other),
        ] {
            assert_eq!(dispatcher_kind(reason), kind, "{reason}");
        }
    }

    #[test]
    fn upload_and_unopened_words() {
        for (error, kind) in [
            ("the SFTP server's host key is not pinned (see the log for its fingerprint)", Some(Kind::HostKey)),
            ("SSH failed: Disconnected", Some(Kind::Ssh)),
            ("SFTP login was refused", Some(Kind::Login)),
            ("SFTP failed before the upload started: channel closed", Some(Kind::Ssh)),
            ("SFTP timed out before the upload started", Some(Kind::Timeout)),
            ("SSH failed: No common key exchange algorithm", Some(Kind::Protocol)),
            ("SSH failed: no common cipher algorithm found", Some(Kind::Protocol)),
            ("SSH failed: invalid SSH version string", Some(Kind::Local)),
            ("something else", None),
        ] {
            assert_eq!(upload_kind(error), kind, "{error}");
        }
        for (error, kind) in [
            (crate::sender::NOT_BUILT_IN.to_owned(), Kind::NotBuiltIn),
            ("the sending service is not reachable; try again later".to_owned(), Kind::SenderDown),
            ("the sending service stopped before connecting to BIR".to_owned(), Kind::SenderDown),
            (
                "The return does not match its ticket: the TIN in the return is not the ticket's TIN. (return_mismatch)"
                    .to_owned(),
                Kind::Mismatch,
            ),
            (crate::filing::gate::Standing::TestOnly.failure(), Kind::Refusal),
            (crate::filing::gate::Standing::Paused.failure(), Kind::Refusal),
            (crate::filing::gate::Standing::TestOnly.message(), Kind::Refusal),
            ("the ticket is not valid: bad signature".to_owned(), Kind::Ticket),
            ("the ticket expired; ask for a new one".to_owned(), Kind::Ticket),
            ("the ticket belongs to another account".to_owned(), Kind::Ticket),
            ("the ticket was already used".to_owned(), Kind::Ticket),
            ("the envelope does not open".to_owned(), Kind::Ticket),
            ("the return was sealed to another key".to_owned(), Kind::Ticket),
            ("the sealed return is not the one the ticket names".to_owned(), Kind::Ticket),
            ("the job's XML is not base64".to_owned(), Kind::Ticket),
            ("the job's TIN is not valid".to_owned(), Kind::Ticket),
            ("the Sender did not understand the job".to_owned(), Kind::Ticket),
            ("could not encrypt the return: bad passphrase".to_owned(), Kind::Other),
            ("no lab SFTP server set".to_owned(), Kind::Other),
            ("".to_owned(), Kind::Other),
        ] {
            assert_eq!(unopened_kind(&error), kind, "{error}");
        }
    }
}
