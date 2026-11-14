//! The rules, every one and its edges, with made-up tries and a made-up
//! clock.

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use time::macros::datetime;

use super::*;

/// 10:00 in Manila.
const T0: OffsetDateTime = datetime!(2026-11-11 02:00 UTC);
const EPOCH: i32 = 7;

fn t(minutes: i64) -> OffsetDateTime {
    T0 + Duration::minutes(minutes)
}

fn keys_in(epoch: i32, account: u8, network: u8) -> Keys {
    Keys { epoch, account: [account; 8], network: [network; 8] }
}

/// A made-up try by `account` from `network`.
fn base(at: OffsetDateTime, account: u8, network: u8) -> Evidence {
    Evidence {
        id: None,
        at,
        source: Source::Relay,
        admission: Admission::Open,
        outcome: Outcome::Failed,
        stage: Stage::Upload,
        dispatcher: Window::Ok,
        upload: Window::Fail,
        kind: Some(Kind::Refused),
        counted: true,
        late: false,
        keys: vec![keys_in(EPOCH, account, network)],
        account_id: Some(Uuid::from_u128(u128::from(account))),
        port: Some(22),
    }
}

/// Got in: past both windows.
fn landed(at: OffsetDateTime, account: u8, network: u8) -> Evidence {
    Evidence {
        outcome: Outcome::Sent,
        stage: Stage::Uploaded,
        upload: Window::Ok,
        kind: None,
        ..base(at, account, network)
    }
}

/// The front desk answered; the mailbox refused.
fn mailbox_down(at: OffsetDateTime, account: u8, network: u8) -> Evidence {
    base(at, account, network)
}

/// The front desk did not answer over TLS.
fn desk_down(at: OffsetDateTime, account: u8, network: u8) -> Evidence {
    Evidence {
        stage: Stage::Dispatcher,
        dispatcher: Window::Fail,
        upload: Window::None,
        kind: Some(Kind::Timeout),
        port: Some(443),
        ..base(at, account, network)
    }
}

/// The upload broke midway.
fn cut(at: OffsetDateTime, account: u8, network: u8) -> Evidence {
    Evidence { outcome: Outcome::Unknown, upload: Window::Maybe, kind: Some(Kind::Cut), ..base(at, account, network) }
}

/// The user's own network.
fn own_network(at: OffsetDateTime, account: u8, network: u8) -> Evidence {
    Evidence {
        stage: Stage::Dispatcher,
        dispatcher: Window::None,
        upload: Window::None,
        kind: Some(Kind::Local),
        counted: false,
        ..base(at, account, network)
    }
}

fn ours(at: OffsetDateTime, account: u8, network: u8, kind: Kind) -> Evidence {
    Evidence { upload: Window::None, kind: Some(kind), counted: false, ..base(at, account, network) }
}

fn direct(item: Evidence) -> Evidence {
    Evidence { source: Source::Direct, account_id: None, ..item }
}

fn scout(item: Evidence) -> Evidence {
    Evidence { admission: Admission::Scout, ..item }
}

/// The engine as the live layer runs it, in memory.
struct Sim {
    state: SignState,
    evidence: Vec<Evidence>,
    fresh: usize,
    note: Option<Note>,
    log: Vec<(OffsetDateTime, Event)>,
}

impl Sim {
    fn new() -> Self {
        Self {
            state: SignState::new(T0 - Duration::hours(4)),
            evidence: Vec::new(),
            fresh: 0,
            note: None,
            log: Vec::new(),
        }
    }

    fn run(&mut self, now: OffsetDateTime, resume_relay: bool) -> Vec<Event> {
        self.evidence.retain(|item| item.at >= now - EVIDENCE_KEPT);
        self.fresh = self.fresh.min(self.evidence.len());
        let inputs =
            Inputs { evidence: &self.evidence, fresh: self.fresh, note: self.note.as_ref(), resume_relay, now };
        let evaluation = evaluate(&self.state, &inputs);
        // The same inputs, the same answer.
        assert_eq!(evaluate(&self.state, &inputs), evaluation);
        if let Some((id, _)) = evaluation.note_ended
            && self.note.as_ref().is_some_and(|note| note.id == id)
        {
            self.note = None;
        }
        self.fresh = 0;
        self.state = evaluation.state;
        self.log.extend(evaluation.events.iter().map(|event| (now, event.clone())));
        evaluation.events
    }

    /// A try recorded at its own minute.
    fn feed(&mut self, item: Evidence) -> Vec<Event> {
        let now = item.at.max(self.state.evaluated_at);
        self.evidence.push(item);
        self.fresh += 1;
        self.run(now, false)
    }

    fn feed_all(&mut self, items: impl IntoIterator<Item = Evidence>) {
        for item in items {
            self.feed(item);
        }
    }

    fn tick(&mut self, now: OffsetDateTime) -> Vec<Event> {
        self.run(now, false)
    }

    fn lamp(&self, side: Side) -> Light {
        self.state.lamp(side).state
    }

    fn upload(&self) -> Light {
        self.lamp(Side::Upload)
    }

    fn desk(&self) -> Light {
        self.lamp(Side::Dispatcher)
    }

    fn overall(&self) -> Light {
        self.state.state
    }
}

fn note(kind: NoteKind, starts_at: OffsetDateTime, until: OffsetDateTime) -> Note {
    Note { id: Uuid::from_u128(0xabc), kind, text: "Made-up note text".into(), starts_at, until }
}

/// Online, with landings from five accounts at `minute`.
fn online_at(minute: i64) -> Sim {
    let mut sim = Sim::new();
    for account in 1..=5 {
        sim.feed(landed(t(minute), account, account));
    }
    assert_eq!((sim.upload(), sim.desk(), sim.overall()), (Light::Online, Light::Online, Light::Online));
    sim
}

#[test]
fn the_first_landing_after_quiet_says_online() {
    let mut sim = Sim::new();
    assert_eq!(sim.overall(), Light::NoRecentNews);
    let events = sim.feed(landed(t(0), 1, 1));
    assert_eq!(sim.upload(), Light::Online);
    assert_eq!(sim.desk(), Light::Online);
    assert_eq!(sim.overall(), Light::Online);
    let scopes: Vec<&str> = events.iter().map(|event| event.scope.as_str()).collect();
    assert_eq!(scopes, ["dispatcher", "upload", "overall"]);
    assert_eq!(events[1].reason, "1 of the last 1 got in");
    assert_eq!(sim.state.seq, 1);
    assert_eq!(sim.state.last_success_at, Some(t(0)));
    assert_eq!(sim.state.upload.last_ok_at, Some(t(0)));
}

#[test]
fn online_needs_the_newest_to_get_in_and_at_most_one_failure() {
    // From No recent news: nine of ten got in, the newest among them.
    let mut sim = Sim::new();
    sim.feed(mailbox_down(t(0), 1, 1));
    assert_eq!(sim.upload(), Light::NoRecentNews);
    for account in 2..=10 {
        sim.feed(landed(t(1), account, account));
    }
    assert_eq!(sim.upload(), Light::Online);

    // Two failures (one account each, so never Busy) and the newest got in:
    // not Online.
    let mut sim = Sim::new();
    sim.feed(mailbox_down(t(0), 1, 1));
    sim.feed(mailbox_down(t(0), 2, 2));
    for account in 3..=10 {
        sim.feed(landed(t(1), account, account));
    }
    assert_eq!(sim.upload(), Light::NoRecentNews);

    // The newest failed: not Online, though 9 of 10 got in.
    let mut evidence: Vec<Evidence> = (1..=9).map(|account| landed(t(0), account, account)).collect();
    evidence.push(mailbox_down(t(1), 11, 11));
    let inputs = Inputs { evidence: &evidence, fresh: 0, note: None, resume_relay: false, now: t(2) };
    let reading = read(Side::Upload, &Lamp::new(t(0)), &inputs, &same_accounts(&evidence), &[]);
    assert_eq!((reading.oks, reading.failures), (9, 1));
    assert!(!reading.online());
    evidence.push(landed(t(1), 12, 12));
    let inputs = Inputs { evidence: &evidence, fresh: 0, note: None, resume_relay: false, now: t(2) };
    assert!(read(Side::Upload, &Lamp::new(t(0)), &inputs, &same_accounts(&evidence), &[]).online());

    // Online with one failure stays Online: one failure moves nothing.
    let mut sim = online_at(0);
    sim.feed(mailbox_down(t(1), 11, 11));
    assert_eq!(sim.upload(), Light::Online);
}

#[test]
fn busy_needs_three_failures_from_two_accounts_on_two_networks_and_a_landing() {
    // Two accounts on two networks, three failures, and one landing: Busy,
    // at once.
    let mut sim = online_at(0);
    sim.feed(mailbox_down(t(1), 11, 11));
    sim.feed(mailbox_down(t(1), 11, 11));
    let events = sim.feed(mailbox_down(t(2), 12, 12));
    assert_eq!(sim.upload(), Light::Busy);
    assert_eq!(sim.overall(), Light::Busy);
    assert_eq!(events[0].reason, "5 of the last 8 got in; 3 didn't, from 2 accounts on 2 networks");

    // One account, however many failures (it fills 2 of the 10).
    let mut sim = online_at(0);
    for minute in 1..=6 {
        sim.feed(mailbox_down(t(minute), 11, 11));
    }
    assert_eq!(sim.upload(), Light::Online);

    // Two accounts on one network.
    let mut sim = online_at(0);
    sim.feed_all([mailbox_down(t(1), 11, 20), mailbox_down(t(1), 11, 20), mailbox_down(t(2), 12, 20)]);
    assert_eq!(sim.upload(), Light::Online);

    // One account on two networks.
    let mut sim = online_at(0);
    sim.feed_all([mailbox_down(t(1), 11, 20), mailbox_down(t(1), 11, 21), mailbox_down(t(2), 11, 22)]);
    assert_eq!(sim.upload(), Light::Online);

    // Two failures only.
    let mut sim = online_at(0);
    sim.feed_all([mailbox_down(t(1), 11, 11), mailbox_down(t(2), 12, 12)]);
    assert_eq!(sim.upload(), Light::Online);

    // A broken upload (`maybe`) is a failure for Busy.
    let mut sim = online_at(0);
    sim.feed_all([cut(t(1), 11, 11), cut(t(1), 11, 11), mailbox_down(t(2), 12, 12)]);
    assert_eq!(sim.upload(), Light::Busy);

    // Nothing got in among the last 10: not Busy (and from No recent
    // news, nothing moves until a quorum says Offline).
    let mut sim = Sim::new();
    sim.feed_all([mailbox_down(t(1), 11, 11), mailbox_down(t(1), 11, 11), mailbox_down(t(2), 12, 12)]);
    assert_eq!(sim.upload(), Light::NoRecentNews);
}

#[test]
fn one_account_fills_at_most_two_of_the_last_ten() {
    let mut evidence: Vec<Evidence> = (0..12).map(|_| landed(t(1), 1, 1)).collect();
    evidence.extend((2..=9).map(|account| mailbox_down(t(2), account, account)));
    let inputs = Inputs { evidence: &evidence, fresh: 0, note: None, resume_relay: false, now: t(3) };
    let accounts = same_accounts(&evidence);
    let lamp = Lamp::new(t(0));
    let reading = read(Side::Upload, &lamp, &inputs, &accounts, &[]);
    assert_eq!(reading.last.len(), 10);
    assert_eq!(reading.oks, 2, "account 1 fills two places, however many it sent");
    assert_eq!(reading.failures, 8);
    assert!(reading.busy());
    // The newest first, within 60 minutes.
    let inputs = Inputs { now: t(62), ..inputs };
    let reading = read(Side::Upload, &lamp, &inputs, &accounts, &[]);
    assert_eq!(reading.last.len(), 8, "the landings of minute 1 are older than 60 minutes");
}

#[test]
fn offline_needs_three_accounts_on_three_networks_within_15_minutes() {
    // 3 on 3, within 15 minutes, at once.
    let mut sim = online_at(0);
    sim.feed_all([mailbox_down(t(10), 11, 11), mailbox_down(t(15), 12, 12)]);
    assert_eq!(sim.upload(), Light::Online);
    let events = sim.feed(mailbox_down(t(25), 13, 13));
    assert_eq!(sim.upload(), Light::Offline);
    assert_eq!(sim.overall(), Light::Offline);
    assert_eq!(events[0].reason, "3 accounts on 3 networks didn't get past the mailbox within 15 min");
    assert_eq!(events[0].cause, Cause::Rule);
    assert_eq!(sim.desk(), Light::Online, "the front desk answered every one");

    // The 15-minute edge: the first failure 16 minutes before the third.
    let mut sim = online_at(0);
    sim.feed_all([mailbox_down(t(10), 11, 11), mailbox_down(t(15), 12, 12), mailbox_down(t(26), 13, 13)]);
    assert_ne!(sim.upload(), Light::Offline);

    // Two accounts.
    let mut sim = online_at(0);
    sim.feed_all([mailbox_down(t(10), 11, 11), mailbox_down(t(11), 12, 12), mailbox_down(t(12), 12, 13)]);
    assert_ne!(sim.upload(), Light::Offline);

    // The same account on three networks.
    let mut sim = online_at(0);
    sim.feed_all([mailbox_down(t(10), 11, 11), mailbox_down(t(11), 11, 12), mailbox_down(t(12), 11, 13)]);
    assert_ne!(sim.upload(), Light::Offline);

    // Three accounts on one network.
    let mut sim = online_at(0);
    sim.feed_all([mailbox_down(t(10), 11, 20), mailbox_down(t(11), 12, 20), mailbox_down(t(12), 13, 20)]);
    assert_ne!(sim.upload(), Light::Offline);

    // Three accounts and three networks, but no three with a network each:
    // one person on three networks and two on the first.
    let mut sim = online_at(0);
    sim.feed_all([
        mailbox_down(t(10), 11, 20),
        mailbox_down(t(10), 11, 21),
        mailbox_down(t(10), 11, 22),
        mailbox_down(t(11), 12, 20),
        mailbox_down(t(12), 13, 20),
    ]);
    assert_eq!(spread(&sim.evidence.iter().rev().take(5).collect::<Vec<_>>()), 2);
    assert_ne!(sim.upload(), Light::Offline);

    // A relay return got past the mailbox after the first failure: none
    // got in "since", so only the failures after it count.
    let mut sim = online_at(0);
    sim.feed_all([
        mailbox_down(t(10), 11, 11),
        mailbox_down(t(11), 12, 12),
        landed(t(12), 1, 1),
        mailbox_down(t(13), 13, 13),
    ]);
    assert_ne!(sim.upload(), Light::Offline);

    // A broken upload might have landed: never toward Offline.
    let mut sim = online_at(0);
    sim.feed_all([cut(t(10), 11, 11), cut(t(11), 12, 12), cut(t(12), 13, 13)]);
    assert_ne!(sim.upload(), Light::Offline);

    // The front desk, the same way.
    let mut sim = online_at(0);
    sim.feed_all([desk_down(t(10), 11, 11), desk_down(t(11), 12, 12), desk_down(t(12), 13, 13)]);
    assert_eq!(sim.desk(), Light::Offline);
    assert_eq!(sim.upload(), Light::Online, "no try reached the mailbox");
    assert_eq!(sim.overall(), Light::Offline);
}

#[test]
fn offline_holds_until_a_relay_return_gets_in() {
    let mut sim = online_at(0);
    sim.feed_all([mailbox_down(t(10), 11, 11), mailbox_down(t(11), 12, 12), mailbox_down(t(12), 13, 13)]);
    assert_eq!(sim.upload(), Light::Offline);
    // More failures, and an hour of quiet: still Offline.
    sim.feed(mailbox_down(t(30), 14, 14));
    sim.tick(t(120));
    assert_eq!(sim.upload(), Light::Offline);
    // A direct-app report that got in does not end it.
    sim.feed(direct(landed(t(121), 15, 15)));
    assert_eq!(sim.upload(), Light::Offline);
    // A scout that gets in does, at once, and the last 10 start empty.
    let events = sim.feed(scout(landed(t(122), 16, 16)));
    assert_eq!(sim.upload(), Light::Online);
    assert_eq!(sim.overall(), Light::Online);
    assert_eq!(events[0].cause, Cause::Scout);
    assert_eq!(events[0].reason, "A scout got in past the mailbox");
    assert_eq!(events.last().unwrap().cause, Cause::Scout);
    assert_eq!(sim.state.upload.fresh_from, Some(t(122)));
    assert_eq!(sim.state.waking_since, Some(t(122)), "the end of Offline starts waking");
    // A Try anyway says so.
    let mut sim = online_at(0);
    sim.feed_all([mailbox_down(t(10), 11, 11), mailbox_down(t(11), 12, 12), mailbox_down(t(12), 13, 13)]);
    let events = sim.feed(Evidence { admission: Admission::TryAnyway, ..landed(t(14), 9, 9) });
    assert_eq!(events[0].cause, Cause::TryAnyway);
}

#[test]
fn offline_turns_to_no_recent_news_after_three_quiet_hours() {
    let mut sim = online_at(0);
    sim.feed_all([mailbox_down(t(10), 11, 11), mailbox_down(t(11), 12, 12), mailbox_down(t(12), 13, 13)]);
    sim.tick(t(12 + 179));
    assert_eq!(sim.upload(), Light::Offline);
    assert_eq!(sim.desk(), Light::NoRecentNews, "the front desk went quiet after 60 minutes");
    let events = sim.tick(t(12 + 180));
    assert_eq!(sim.upload(), Light::NoRecentNews);
    assert_eq!(sim.state.upload.last_state, Some(Light::Offline));
    assert_eq!(events[0].cause, Cause::Quiet);
    // The front desk went quiet at 60 minutes; overall now says No recent
    // news, after Offline.
    assert_eq!(sim.overall(), Light::NoRecentNews);
    assert_eq!(sim.state.last_state, Some(Light::Offline));
    assert_eq!(sim.state.waking_since, Some(t(192)));
}

#[test]
fn quiet_hours_say_no_recent_news() {
    let mut sim = online_at(0);
    assert!(sim.tick(t(59)).is_empty());
    let events = sim.tick(t(60));
    assert_eq!(sim.upload(), Light::NoRecentNews);
    assert_eq!(sim.overall(), Light::NoRecentNews);
    assert_eq!(sim.state.last_state, Some(Light::Online));
    assert_eq!(events.last().unwrap().reason, "Nobody filed in the last 60 min");
    assert_eq!(events.last().unwrap().cause, Cause::Quiet);
    assert_eq!(sim.state.last_success_at, Some(t(0)), "for \"last return got in 1 h ago\"");
    // Then the first landing decides.
    sim.feed(landed(t(70), 1, 1));
    assert_eq!(sim.overall(), Light::Online);
    assert_eq!(sim.state.last_state, None);
}

#[test]
fn the_dwell_holds_only_what_lets_more_through() {
    // Busy at once, from Online.
    let mut sim = online_at(0);
    sim.feed_all([mailbox_down(t(1), 11, 11), mailbox_down(t(1), 11, 11), mailbox_down(t(2), 12, 12)]);
    assert_eq!(sim.upload(), Light::Busy);
    assert_eq!(sim.state.upload.since, t(2));
    // Landings push the failures out, but Online waits until Busy is 10
    // minutes old.
    for account in 21..=30 {
        sim.feed(landed(t(3), account, account));
    }
    assert_eq!(sim.upload(), Light::Busy);
    assert!(sim.tick(t(11) + Duration::seconds(59)).is_empty());
    let events = sim.tick(t(12));
    assert_eq!(sim.upload(), Light::Online);
    assert_eq!(events[0].reason, "10 of the last 10 got in");
    // And straight back to Busy, at once: tightening never waits. (11 and
    // 12 lost their Offline votes to the landings at 3.)
    sim.feed_all([mailbox_down(t(13), 31, 31), mailbox_down(t(13), 31, 31), mailbox_down(t(13), 32, 32)]);
    assert_eq!(sim.upload(), Light::Busy);
    // Offline from Busy, at once too.
    sim.feed(mailbox_down(t(14), 33, 33));
    assert_eq!(sim.upload(), Light::Offline);

    // Busy to No recent news only after the dwell (an hour of quiet is
    // always past it).
    let mut sim = online_at(0);
    sim.feed_all([mailbox_down(t(1), 11, 11), mailbox_down(t(1), 11, 11), mailbox_down(t(2), 12, 12)]);
    sim.tick(t(61));
    assert_eq!(sim.upload(), Light::Busy);
    sim.tick(t(62));
    assert_eq!(sim.upload(), Light::NoRecentNews);
}

#[test]
fn flapping_input_loosens_at_most_every_ten_minutes() {
    let mut sim = online_at(0);
    let mut rng = StdRng::seed_from_u64(11);
    for minute in 1..=300 {
        let account = rng.random_range(11..=16u8);
        let item = if rng.random_bool(0.5) {
            landed(t(minute), account, account)
        } else {
            mailbox_down(t(minute), account, account)
        };
        let before = (sim.upload(), sim.state.upload.since);
        sim.feed(item);
        if before.0 == Light::Busy && matches!(sim.upload(), Light::Online | Light::NoRecentNews) {
            assert!(t(minute) - before.1 >= DWELL, "loosened at minute {minute}");
        }
    }
    assert!(sim.log.iter().any(|(_, event)| event.to == "busy"));
}

#[test]
fn a_landing_disproves_the_votes_of_those_who_just_failed() {
    let mut sim = online_at(0);
    sim.feed_all([mailbox_down(t(10), 11, 11), mailbox_down(t(11), 12, 12)]);
    // A landing at 12: 11 and 12 failed within the 15 minutes before.
    sim.feed(landed(t(12), 1, 1));
    assert_eq!(sim.state.vote_losses.len(), 2);
    // Now 11, 12 and 13 fail: only 13 votes.
    sim.feed_all([mailbox_down(t(13), 11, 11), mailbox_down(t(13), 12, 12), mailbox_down(t(14), 13, 13)]);
    assert_ne!(sim.upload(), Light::Offline);
    // 14 and 15 make three with votes.
    sim.feed_all([mailbox_down(t(15), 14, 14), mailbox_down(t(16), 15, 15)]);
    assert_eq!(sim.upload(), Light::Offline);

    // The loss lasts 24 hours, then the vote is back.
    let mut sim = online_at(0);
    sim.feed_all([mailbox_down(t(10), 11, 11), landed(t(12), 1, 1)]);
    let back = t(12) + VOTE_LOST_FOR;
    sim.feed(landed(back - Duration::minutes(30), 2, 2));
    sim.feed_all([
        mailbox_down(back - Duration::minutes(2), 11, 11),
        mailbox_down(back - Duration::minutes(2), 12, 12),
        mailbox_down(back - Duration::minutes(1), 13, 13),
    ]);
    assert_ne!(sim.upload(), Light::Offline, "11 has no vote yet");
    sim.feed(mailbox_down(back, 11, 11));
    assert_eq!(sim.upload(), Light::Offline);

    // A direct report that got in disproves nothing.
    let mut sim = online_at(0);
    sim.feed_all([mailbox_down(t(10), 11, 11), direct(landed(t(12), 1, 1))]);
    assert!(sim.state.vote_losses.is_empty());

    // Read back after a restart (no account id), the keys still match.
    let mut sim = online_at(0);
    sim.feed_all([mailbox_down(t(10), 11, 11), landed(t(12), 1, 1)]);
    let stored: SignState = serde_json::from_value(serde_json::to_value(&sim.state).unwrap()).unwrap();
    assert_eq!(stored.vote_losses[0].account_id, None);
    let reloaded = Evidence { account_id: None, ..mailbox_down(t(13), 11, 11) };
    assert!(stored.vote_losses[0].covers(Side::Upload, &reloaded));
    assert!(!stored.vote_losses[0].covers(Side::Dispatcher, &reloaded));
}

#[test]
fn direct_reports_are_trusted_in_part() {
    // Two relay accounts and one direct: a quorum.
    let mut sim = online_at(0);
    sim.feed_all([mailbox_down(t(10), 11, 11), mailbox_down(t(11), 12, 12), direct(mailbox_down(t(12), 13, 13))]);
    assert_eq!(sim.upload(), Light::Offline);
    // One relay and two direct: at most one direct account counts.
    let mut sim = online_at(0);
    sim.feed_all([
        mailbox_down(t(10), 11, 11),
        direct(mailbox_down(t(11), 12, 12)),
        direct(mailbox_down(t(12), 13, 13)),
    ]);
    assert_ne!(sim.upload(), Light::Offline);
    // A late direct report adds no network.
    let mut sim = online_at(0);
    sim.feed_all([
        mailbox_down(t(10), 11, 11),
        mailbox_down(t(11), 12, 12),
        Evidence { late: true, ..direct(mailbox_down(t(12), 13, 13)) },
    ]);
    assert_ne!(sim.upload(), Light::Offline);
    // A direct success is not "got in since": it holds nothing off.
    let mut sim = online_at(0);
    sim.feed_all([
        mailbox_down(t(10), 11, 11),
        mailbox_down(t(11), 12, 12),
        direct(landed(t(11), 2, 2)),
        mailbox_down(t(12), 13, 13),
    ]);
    assert_eq!(sim.upload(), Light::Offline);
    // Only a direct quorum: nothing.
    let mut sim = online_at(0);
    sim.feed_all((11..=16).map(|account| direct(mailbox_down(t(10), account, account))));
    assert_ne!(sim.upload(), Light::Offline);
}

#[test]
fn the_users_own_network_and_buwizs_side_never_move_a_light() {
    let mut rng = StdRng::seed_from_u64(3);
    for round in 0..20 {
        let mut sim = online_at(0);
        let start = sim.state.clone();
        for minute in 1..=50 {
            let account = rng.random_range(1..=40u8);
            let network = rng.random_range(1..=40u8);
            let item = match rng.random_range(0..4) {
                0 => own_network(t(minute), account, network),
                1 => ours(t(minute), account, network, Kind::SenderDown),
                2 => ours(t(minute), account, network, Kind::TrustList),
                _ => Evidence { kind: Some(Kind::AppGone), ..own_network(t(minute), account, network) },
            };
            sim.feed(item);
            assert_eq!(sim.upload(), Light::Online, "round {round}");
            assert_eq!(sim.desk(), Light::Online);
        }
        // "No recent news" after an hour, as without them.
        assert_eq!(sim.state.upload.last_evidence_at, start.upload.last_evidence_at);
        sim.tick(t(60));
        assert_eq!(sim.overall(), Light::NoRecentNews);
    }
}

#[test]
fn notes_set_the_overall_sign() {
    // An outage note: Offline, whatever the evidence says.
    let mut sim = online_at(0);
    sim.note = Some(note(NoteKind::Outage, t(5), t(65)));
    let events = sim.tick(t(5));
    assert_eq!(sim.overall(), Light::Offline);
    assert_eq!(sim.upload(), Light::Online, "the lights keep reading the evidence");
    assert_eq!(
        events,
        [Event {
            scope: Scope::Overall,
            from: "online".into(),
            to: "offline".into(),
            cause: Cause::NoteSet,
            reason: "An Admin set an outage note".into(),
        }]
    );
    assert_eq!(sim.state.source(), "staff");
    // It runs out at its end: the note is marked ended, waking starts.
    // (The lights went quiet at 60 minutes; the overall sign held.)
    assert!(sim.tick(t(64)).iter().all(|event| event.scope != Scope::Overall));
    assert_eq!(sim.overall(), Light::Offline);
    let inputs = Inputs { evidence: &sim.evidence, fresh: 0, note: sim.note.as_ref(), resume_relay: false, now: t(65) };
    let evaluation = evaluate(&sim.state, &inputs);
    assert_eq!(evaluation.note_ended, Some((Uuid::from_u128(0xabc), NoteEnd::Expired)));
    let events = sim.tick(t(65));
    assert_eq!(events.last().unwrap().cause, Cause::NoteExpired);
    assert_eq!(sim.note, None);
    assert_eq!(sim.overall(), Light::NoRecentNews, "the evidence is an hour old");
    assert_eq!(sim.state.waking_since, Some(t(65)));
    assert_eq!(sim.state.source(), "tries");

    // A return that got in ends an outage note ...
    let mut sim = online_at(0);
    sim.note = Some(note(NoteKind::Maintenance, t(5), t(65)));
    sim.tick(t(5));
    sim.feed(direct(landed(t(6), 1, 1)));
    assert!(sim.note.is_some(), "not a direct report");
    let events = sim.feed(landed(t(7), 1, 1));
    assert_eq!(events.last().unwrap().cause, Cause::NoteReturnGotIn);
    assert_eq!(sim.note, None);
    assert_eq!(sim.overall(), Light::Online);
    assert_eq!(sim.state.waking_since, Some(t(7)));

    // ... but not an "online" note.
    let mut sim = online_at(0);
    sim.feed_all([mailbox_down(t(10), 11, 11), mailbox_down(t(11), 12, 12), mailbox_down(t(12), 13, 13)]);
    assert_eq!(sim.overall(), Light::Offline);
    sim.note = Some(note(NoteKind::Online, t(13), t(13) + Duration::hours(72)));
    let events = sim.tick(t(13));
    assert_eq!(sim.overall(), Light::Online, "declared Online: evidence ignored");
    assert_eq!(sim.upload(), Light::Offline, "and still read");
    assert_eq!(events.last().unwrap().reason, "An Admin declared the BIR online, ignoring evidence");
    sim.feed(landed(t(14), 1, 1));
    assert!(sim.note.is_some());
    // Cleared by an Admin: the evidence speaks again.
    sim.note = None;
    let events = sim.tick(t(20));
    assert_eq!(events.last().unwrap().cause, Cause::NoteCleared);
    assert_eq!(sim.overall(), Light::Online, "the landing at 14 ended Offline");

    // A note cleared while Offline by evidence: no waking from the note.
    let mut sim = online_at(0);
    sim.feed_all([mailbox_down(t(10), 11, 11), mailbox_down(t(11), 12, 12), mailbox_down(t(12), 13, 13)]);
    sim.note = Some(note(NoteKind::Outage, t(13), t(70)));
    sim.tick(t(13));
    sim.note = None;
    sim.tick(t(14));
    assert_eq!(sim.overall(), Light::Offline);
    assert_eq!(sim.state.waking_since, None);
}

#[test]
fn an_expired_note_never_seen_changes_nothing() {
    let mut sim = online_at(0);
    sim.note = Some(note(NoteKind::Outage, t(1), t(2)));
    let inputs = Inputs { evidence: &sim.evidence, fresh: 0, note: sim.note.as_ref(), resume_relay: false, now: t(3) };
    let evaluation = evaluate(&sim.state, &inputs);
    assert!(evaluation.events.is_empty());
    assert_eq!(evaluation.note_ended, Some((Uuid::from_u128(0xabc), NoteEnd::Expired)));
}

#[test]
fn the_relay_pauses_after_three_faults_from_two_accounts() {
    let mut sim = online_at(0);
    sim.feed_all([ours(t(1), 1, 1, Kind::HostKey), ours(t(2), 1, 1, Kind::HostKey), ours(t(3), 1, 1, Kind::HostKey)]);
    assert!(!sim.state.relay.paused, "one account");
    let events = sim.feed(ours(t(4), 2, 2, Kind::UnknownHost));
    assert!(sim.state.relay.paused);
    assert_eq!(events[0].scope, Scope::Relay);
    assert_eq!(events[0].cause, Cause::RelayPaused);
    assert_eq!(
        sim.state.relay.reason.as_deref(),
        Some(
            "4 tries from 2 accounts met a fault on Buwiz's side that touches the BIR (a host key that is not pinned: 3, an unknown upload server: 1)"
        )
    );
    assert_eq!(sim.overall(), Light::Online, "the pause is the relay's, not the BIR's");
    // Only an Admin resumes it; the faults from before count no more.
    sim.tick(t(200));
    assert!(sim.state.relay.paused);
    let events = sim.run(t(201), true);
    assert_eq!(events[0].cause, Cause::RelayResumed);
    assert!(!sim.state.relay.paused);
    sim.feed_all([ours(t(202), 1, 1, Kind::Protocol), ours(t(203), 2, 2, Kind::Protocol)]);
    assert!(!sim.state.relay.paused);
    sim.feed(ours(t(204), 3, 3, Kind::Protocol));
    assert!(sim.state.relay.paused);

    // Two faults; or three spread over more than 60 minutes; or kinds that
    // touch nothing: no pause.
    let mut sim = online_at(0);
    sim.feed_all([ours(t(1), 1, 1, Kind::HostKey), ours(t(2), 2, 2, Kind::HostKey)]);
    sim.feed(ours(t(62), 3, 3, Kind::HostKey));
    assert!(!sim.state.relay.paused);
    sim.feed_all((1..=5).map(|account| ours(t(70), account, account, Kind::SenderDown)));
    assert!(!sim.state.relay.paused);
}

#[test]
fn keys_from_another_period_are_compared_only_through_the_previous_key() {
    const OLD: i32 = 7;
    const NEW: i32 = 8;
    // 23:50 and 23:55 Manila on the 11th, then 00:02 on the 12th.
    let before = datetime!(2026-11-11 15:50 UTC);
    let after = datetime!(2026-11-11 16:02 UTC);
    let in_new = |item: Evidence, account: u8, network: u8, with_previous: bool| {
        let mut keys = vec![keys_in(NEW, account + 100, network + 100)];
        if with_previous {
            keys.push(keys_in(OLD, account, network));
        }
        Evidence { keys, ..item }
    };
    let outage = |with_previous: bool| {
        let mut sim = Sim::new();
        for account in 1..=3 {
            sim.feed(landed(before - Duration::minutes(5), account, account));
        }
        sim.feed_all([mailbox_down(before, 11, 11), mailbox_down(before + Duration::minutes(5), 12, 12)]);
        // Account 11 again after midnight: under the new key and the old.
        sim.feed(in_new(mailbox_down(after, 11, 11), 11, 11, with_previous));
        sim
    };
    // With the previous key, 11 after midnight is 11 before: two accounts.
    let sim = outage(true);
    assert_ne!(sim.upload(), Light::Offline);
    // A third account after midnight, with both keys: three on three.
    let mut sim = outage(true);
    sim.feed(in_new(mailbox_down(after, 13, 13), 13, 13, true));
    assert_eq!(sim.upload(), Light::Offline);
    // After a restart the new key has no previous: the tries cannot be
    // compared, so they are not taken for three accounts.
    let mut sim = outage(false);
    sim.feed(in_new(mailbox_down(after, 13, 13), 13, 13, false));
    assert_ne!(sim.upload(), Light::Offline, "only two in either key period");
    // Fresh evidence under the new key alone makes its own quorum.
    sim.feed(in_new(mailbox_down(after + Duration::minutes(1), 14, 14), 14, 14, false));
    assert_eq!(sim.upload(), Light::Offline);

    // During an outage, midnight changes nothing: still Offline until a
    // landing.
    let mut sim = online_at(0);
    sim.feed_all([mailbox_down(t(10), 11, 11), mailbox_down(t(11), 12, 12), mailbox_down(t(12), 13, 13)]);
    let midnight = datetime!(2026-11-11 16:00 UTC);
    for minute in (0..120).step_by(20) {
        sim.feed(in_new(mailbox_down(midnight + Duration::minutes(minute), 11, 11), 11, 11, false));
    }
    assert_eq!(sim.upload(), Light::Offline);
    sim.feed(in_new(scout(landed(midnight + Duration::minutes(130), 4, 4)), 4, 4, false));
    assert_eq!(sim.upload(), Light::Online);
}

/// Each state, as a restart finds it: the stored sign read back, the tries
/// with only their recorded key and no account id.
#[test]
fn a_restart_finds_every_state_as_it_was() {
    let busy = {
        let mut sim = online_at(0);
        sim.feed_all([mailbox_down(t(1), 11, 11), mailbox_down(t(1), 11, 11), mailbox_down(t(2), 12, 12)]);
        sim
    };
    let offline = {
        let mut sim = online_at(0);
        sim.feed_all([mailbox_down(t(1), 11, 11), mailbox_down(t(2), 12, 12), mailbox_down(t(3), 13, 13)]);
        sim
    };
    let quiet = {
        let mut sim = online_at(0);
        sim.tick(t(70));
        sim
    };
    let noted = {
        let mut sim = online_at(0);
        sim.note = Some(note(NoteKind::Maintenance, t(1), t(100)));
        sim.tick(t(1));
        sim
    };
    let paused = {
        let mut sim = online_at(0);
        sim.feed_all([
            ours(t(1), 1, 1, Kind::HostKey),
            ours(t(2), 2, 2, Kind::HostKey),
            ours(t(3), 2, 2, Kind::HostKey),
        ]);
        sim
    };
    let disproved = {
        let mut sim = online_at(0);
        sim.feed_all([mailbox_down(t(1), 11, 11), landed(t(2), 1, 1)]);
        sim
    };
    for (name, sim) in [
        ("online", online_at(0)),
        ("busy", busy),
        ("offline", offline),
        ("quiet", quiet),
        ("noted", noted),
        ("paused", paused),
        ("disproved", disproved),
    ] {
        let stored: SignState = serde_json::from_value(serde_json::to_value(&sim.state).unwrap()).unwrap();
        let mut expected = sim.state.clone();
        for loss in &mut expected.vote_losses {
            loss.account_id = None;
        }
        assert_eq!(stored, expected, "{name}");
        let read_back: Vec<Evidence> =
            sim.evidence.iter().map(|item| Evidence { account_id: None, port: None, ..item.clone() }).collect();
        let now = sim.state.evaluated_at;
        let inputs = Inputs { evidence: &read_back, fresh: 0, note: sim.note.as_ref(), resume_relay: false, now };
        let evaluation = evaluate(&stored, &inputs);
        assert!(evaluation.events.is_empty(), "{name}: {:?}", evaluation.events);
        assert_eq!(evaluation.state.seq, sim.state.seq, "{name}");
        assert!(!evaluation.changed(&stored), "{name}");
    }
}

#[test]
fn seq_grows_by_one_with_every_change() {
    let mut sim = Sim::new();
    let mut last = sim.state.seq;
    let mut rng = StdRng::seed_from_u64(5);
    for minute in 0..400 {
        let account = rng.random_range(1..=8u8);
        let item = match rng.random_range(0..3) {
            0 => landed(t(minute), account, account),
            1 => mailbox_down(t(minute), account, account),
            _ => desk_down(t(minute), account, account),
        };
        let events = if rng.random_bool(0.8) { sim.feed(item) } else { sim.tick(t(minute)) };
        assert_eq!(sim.state.seq, last + i64::from(!events.is_empty()));
        last = sim.state.seq;
    }
}

/// Property: Offline ends only on a relay try that got past that window,
/// or after 3 hours without evidence on it; and every change is told.
#[test]
fn offline_never_ends_without_a_relay_landing_or_three_quiet_hours() {
    for seed in 0..40 {
        let mut rng = StdRng::seed_from_u64(seed);
        let mut sim = Sim::new();
        let mut minute = 0;
        for _ in 0..300 {
            minute += rng.random_range(0..25);
            let account = rng.random_range(1..=6u8);
            let network = rng.random_range(1..=6u8);
            let mut item = match rng.random_range(0..10) {
                0..=1 => landed(t(minute), account, network),
                2..=5 => mailbox_down(t(minute), account, network),
                6 => desk_down(t(minute), account, network),
                7 => cut(t(minute), account, network),
                8 => own_network(t(minute), account, network),
                _ => ours(t(minute), account, network, Kind::HostKey),
            };
            if rng.random_bool(0.2) {
                item = direct(item);
            }
            let before = sim.state.clone();
            let tick = rng.random_bool(0.2);
            let landing = !tick && item.source == Source::Relay && item.counted;
            let landing_on = |side: Side| landing && item.hit(side) == Some(Hit::Ok);
            let events = if tick { sim.tick(t(minute)) } else { sim.feed(item.clone()) };
            for side in Side::BOTH {
                let was = before.lamp(side);
                let now = sim.state.lamp(side);
                if was.state == Light::Offline && now.state != Light::Offline {
                    let quiet = was.last_evidence_at.is_none_or(|at| t(minute) - at >= OFFLINE_QUIET);
                    assert!(landing_on(side) || (quiet && now.state == Light::NoRecentNews), "seed {seed}");
                }
                if was.state != now.state {
                    assert!(events.iter().any(|event| event.scope == Scope::of(side)), "seed {seed}");
                }
            }
            assert_eq!(sim.overall(), overall(sim.desk(), sim.upload(), sim.state.note.as_ref()), "seed {seed}");
        }
    }
}

/// A scripted day: the event log, in order.
#[test]
fn a_scripted_day_replays_to_the_same_event_log() {
    let script = || {
        let mut sim = Sim::new();
        // 10:00 to 10:20 Manila: a quiet morning, then filings land.
        for account in 1..=6 {
            sim.feed(landed(t(account.into()), account, account));
        }
        // 10:30: the mailbox starts refusing three people on three networks.
        sim.feed_all([mailbox_down(t(30), 11, 11), mailbox_down(t(31), 12, 12), mailbox_down(t(33), 13, 13)]);
        // An hour of failures from scouts.
        for minute in (50..110).step_by(15) {
            sim.feed(scout(mailbox_down(t(minute), 14, 14)));
        }
        // 12:00: a scout gets in.
        sim.feed(scout(landed(t(120), 15, 15)));
        // A deadline rush: some get in, some don't.
        sim.feed_all([
            landed(t(121), 1, 1),
            mailbox_down(t(122), 2, 2),
            mailbox_down(t(122), 3, 3),
            mailbox_down(t(123), 2, 2),
        ]);
        for account in 20..=29 {
            sim.feed(landed(t(125), account, account));
        }
        sim.tick(t(140));
        // Quiet afternoon.
        sim.tick(t(240));
        sim.log.iter().map(|(at, event)| ((*at - T0).whole_minutes(), event.clone())).collect::<Vec<_>>()
    };
    let log = script();
    assert_eq!(log, script(), "the same day replays the same way");
    let lines: Vec<(i64, &str, &str, &str, &str)> = log
        .iter()
        .map(|(minute, event)| {
            (*minute, event.scope.as_str(), event.from.as_str(), event.to.as_str(), event.cause.as_str())
        })
        .collect();
    assert_eq!(
        lines,
        [
            (1, "dispatcher", "no_recent_news", "online", "rule"),
            (1, "upload", "no_recent_news", "online", "rule"),
            (1, "overall", "no_recent_news", "online", "rule"),
            (33, "upload", "online", "offline", "rule"),
            (33, "overall", "online", "offline", "rule"),
            (120, "upload", "offline", "online", "scout"),
            (120, "overall", "offline", "online", "scout"),
            (123, "upload", "online", "busy", "rule"),
            (123, "overall", "online", "busy", "rule"),
            (140, "upload", "busy", "online", "rule"),
            (140, "overall", "busy", "online", "rule"),
            (240, "dispatcher", "online", "no_recent_news", "quiet"),
            (240, "upload", "online", "no_recent_news", "quiet"),
            (240, "overall", "online", "no_recent_news", "quiet"),
        ]
    );
    // Reasons are fixed sentences with counts: no keys, no ids.
    for (_, event) in &log {
        assert!(!event.reason.contains("0x") && event.reason.len() < 200, "{}", event.reason);
    }
}
