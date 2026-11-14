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

/// `minutes` and `seconds` after 10:00: a clock that is not on the minute.
fn ts(minutes: i64, seconds: i64) -> OffsetDateTime {
    t(minutes) + Duration::seconds(seconds)
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
    note: Option<Note>,
    periods: Vec<i32>,
    /// Whether the clock was watched between runs (the ticker's 30
    /// seconds): `false` lets a gap show.
    watched: bool,
    log: Vec<(OffsetDateTime, Event)>,
    notices: Vec<Notice>,
}

impl Sim {
    fn new() -> Self {
        Self {
            state: SignState::new(T0 - Duration::hours(4)),
            evidence: Vec::new(),
            note: None,
            periods: vec![EPOCH],
            watched: true,
            log: Vec::new(),
            notices: Vec::new(),
        }
    }

    fn inputs(&self, now: OffsetDateTime, resume_relay: bool) -> Inputs<'_> {
        Inputs { evidence: &self.evidence, periods: &self.periods, note: self.note.as_ref(), resume_relay, now }
    }

    fn evaluation(&mut self, now: OffsetDateTime, resume_relay: bool) -> Evaluation {
        self.evidence.retain(|item| item.at >= now - EVIDENCE_KEPT);
        if self.watched {
            self.state.evaluated_at = self.state.evaluated_at.max(now - Duration::seconds(30));
        }
        let evaluation = evaluate(&self.state, &self.inputs(now, resume_relay));
        // The same inputs, the same answer.
        assert_eq!(evaluate(&self.state, &self.inputs(now, resume_relay)), evaluation);
        evaluation
    }

    fn run(&mut self, now: OffsetDateTime, resume_relay: bool) -> Vec<Event> {
        let evaluation = self.evaluation(now, resume_relay);
        if let Some((id, _)) = evaluation.note_ended
            && self.note.as_ref().is_some_and(|note| note.id == id)
        {
            self.note = None;
        }
        self.state = evaluation.state;
        self.log.extend(evaluation.events.iter().map(|event| (now, event.clone())));
        self.notices.extend(evaluation.notices);
        evaluation.events
    }

    /// A try recorded when it ended, in order of its end.
    fn feed(&mut self, item: Evidence) -> Vec<Event> {
        let now = item.at.max(self.state.evaluated_at);
        self.add(item);
        self.run(now, false)
    }

    fn add(&mut self, item: Evidence) {
        let at = self.evidence.partition_point(|other| other.at <= item.at);
        self.evidence.insert(at, item);
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

/// Offline on the mailbox: 11, 12 and 13 refused at 10, 11 and 12 minutes.
fn offline_at_12() -> Sim {
    let mut sim = online_at(0);
    sim.feed_all([mailbox_down(t(10), 11, 11), mailbox_down(t(11), 12, 12), mailbox_down(t(12), 13, 13)]);
    assert_eq!(sim.upload(), Light::Offline);
    sim
}

fn reading_of(evidence: &[Evidence], periods: &[i32], now: OffsetDateTime) -> Reading {
    let inputs = Inputs { evidence, periods, note: None, resume_relay: false, now };
    read(Side::Upload, &Lamp::new(T0 - Duration::hours(1)), &inputs, &same_accounts(evidence), &[])
}

#[test]
fn the_first_landing_after_quiet_says_online() {
    let mut sim = Sim::new();
    assert_eq!(sim.overall(), Light::NoRecentNews);
    let events = sim.feed(landed(ts(0, 17), 1, 1));
    assert_eq!(sim.upload(), Light::Online);
    assert_eq!(sim.desk(), Light::Online);
    assert_eq!(sim.overall(), Light::Online);
    let scopes: Vec<&str> = events.iter().map(|event| event.scope.as_str()).collect();
    assert_eq!(scopes, ["dispatcher", "upload", "overall"]);
    assert_eq!(events[1].reason, "1 of the last 1 got in");
    assert_eq!(sim.state.seq, 1);
    // Exact in memory; stored to the minute.
    assert_eq!(sim.state.last_success_at, Some(ts(0, 17)));
    assert_eq!(sim.state.upload.last_ok_at, Some(ts(0, 17)));
    let stored = serde_json::to_value(&sim.state).unwrap();
    assert_eq!(stored["last_success_at"], "2026-11-11T02:00:00Z");
    assert_eq!(stored["upload"]["last_ok_at"], "2026-11-11T02:00:00Z");
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

    // Two failures (two accounts, one network) and the newest got in: not
    // Online, and not Busy either.
    let mut sim = Sim::new();
    sim.feed(mailbox_down(t(0), 1, 20));
    sim.feed(mailbox_down(t(0), 2, 20));
    for account in 3..=10 {
        sim.feed(landed(t(1), account, account));
    }
    assert_eq!(sim.upload(), Light::NoRecentNews);

    // The newest failed, but it is one account's: that cannot by itself
    // say the newest did not get in.
    let mut evidence: Vec<Evidence> = (1..=9).map(|account| landed(t(0), account, account)).collect();
    evidence.push(mailbox_down(t(1), 11, 11));
    let reading = reading_of(&evidence, &[EPOCH], t(2));
    assert_eq!((reading.oks, reading.failures, reading.head_failures), (9, 1, 1));
    assert!(reading.online());
    // The same account again changes nothing: one failure per account.
    evidence.push(mailbox_down(t(1), 11, 11));
    let reading = reading_of(&evidence, &[EPOCH], t(2));
    assert_eq!((reading.failures, reading.head_failures), (1, 1));
    assert!(reading.online());
    // A second account newer than every landing: not Online.
    evidence.push(mailbox_down(t(1), 12, 12));
    let reading = reading_of(&evidence, &[EPOCH], t(2));
    assert_eq!((reading.failures, reading.head_failures), (2, 2));
    assert!(!reading.newest_in() && !reading.online());

    // Online with one failure stays Online: one failure moves nothing.
    let mut sim = online_at(0);
    sim.feed(mailbox_down(t(1), 11, 11));
    assert_eq!(sim.upload(), Light::Online);
}

#[test]
fn online_leaves_once_its_own_rule_fails() {
    // Two accounts on two networks fail: the Online rule no longer holds,
    // and the failures are spread: Busy, at once.
    let mut sim = online_at(0);
    sim.feed(mailbox_down(t(1), 11, 11));
    sim.feed(mailbox_down(t(1), 11, 11));
    let events = sim.feed(mailbox_down(t(2), 12, 12));
    assert_eq!(sim.upload(), Light::Busy);
    assert_eq!(sim.overall(), Light::Busy);
    assert_eq!(events[0].reason, "5 of the last 7 got in; 2 didn't, from 2 accounts on 2 networks");

    // One account, however many failures (one of them counts).
    let mut sim = online_at(0);
    for minute in 1..=6 {
        sim.feed(mailbox_down(t(minute), 11, 11));
    }
    assert_eq!(sim.upload(), Light::Online);

    // One account on three networks.
    let mut sim = online_at(0);
    sim.feed_all([mailbox_down(t(1), 11, 20), mailbox_down(t(1), 11, 21), mailbox_down(t(2), 11, 22)]);
    assert_eq!(sim.upload(), Light::Online);

    // A broken upload (`maybe`) is a failure for Busy.
    let mut sim = online_at(0);
    sim.feed_all([cut(t(1), 11, 11), cut(t(1), 11, 11), mailbox_down(t(2), 12, 12)]);
    assert_eq!(sim.upload(), Light::Busy);

    // Two accounts on one network: not spread enough for Busy, so Online
    // holds while a return got in within the hour, then No recent news,
    // though failures keep coming.
    let mut sim = online_at(0);
    for minute in (1..=70).step_by(3) {
        sim.feed(mailbox_down(t(minute), 11, 20));
        sim.feed(mailbox_down(t(minute), 12, 20));
        let expected = if minute < 60 { Light::Online } else { Light::NoRecentNews };
        assert_eq!(sim.upload(), expected, "minute {minute}");
    }
    let left = sim.log.iter().find(|(_, event)| event.scope == Scope::Upload && event.to == "no_recent_news").unwrap();
    assert_eq!(left.0, t(61));
    assert_eq!(left.1.cause, Cause::Rule);
    assert_eq!(left.1.reason, "Nothing got past the mailbox in the last 60 min");

    // The same at the edge, off the minute: 59 min 59 s after the landing
    // it holds; at 60 min it leaves.
    let mut sim = Sim::new();
    sim.feed(landed(ts(0, 25), 1, 1));
    sim.feed_all([mailbox_down(ts(30, 0), 11, 20), mailbox_down(ts(30, 1), 12, 20)]);
    sim.feed(mailbox_down(ts(60, 24), 13, 20));
    assert_eq!(sim.upload(), Light::Online);
    sim.feed(mailbox_down(ts(60, 25), 14, 20));
    assert_eq!(sim.upload(), Light::NoRecentNews);

    // From No recent news, Busy needs a landing among the last 10 and 3
    // failures from 2 accounts on 2 networks.
    let mut sim = Sim::new();
    sim.feed_all([mailbox_down(t(1), 11, 11), mailbox_down(t(1), 12, 12), mailbox_down(t(2), 13, 11)]);
    assert_eq!(sim.upload(), Light::NoRecentNews, "nothing got in among the last 10");
    let mut sim = Sim::new();
    sim.feed_all([landed(t(0), 1, 1), mailbox_down(t(0), 1, 1)]);
    assert_eq!(sim.upload(), Light::Online);
    let mut sim = Sim::new();
    sim.feed_all([
        mailbox_down(t(0), 2, 2),
        landed(t(0), 1, 1),
        mailbox_down(t(1), 11, 11),
        mailbox_down(t(1), 12, 12),
        mailbox_down(t(2), 13, 11),
    ]);
    assert_eq!(sim.upload(), Light::Busy);
}

#[test]
fn one_account_failing_all_day_never_holds_busy_or_leaves_online() {
    // Account 50 fails every 3 minutes while 20 others land.
    let mut sim = online_at(0);
    let mut minute = 0;
    for round in 0..60 {
        minute += 3;
        sim.feed(mailbox_down(t(minute), 50, 50));
        sim.feed(landed(ts(minute, 30), 21 + (round % 20) as u8, 21 + (round % 20) as u8));
        assert_eq!(sim.upload(), Light::Online, "minute {minute}");
    }
    assert!(sim.log.iter().all(|(_, event)| event.to != "busy"));

    // Nor does it keep Busy once the others land again.
    let mut sim = online_at(0);
    sim.feed_all([mailbox_down(t(1), 11, 11), mailbox_down(t(2), 12, 12)]);
    assert_eq!(sim.upload(), Light::Busy);
    for minute in 3..=13 {
        sim.feed(mailbox_down(t(minute), 50, 50));
        sim.feed(landed(ts(minute, 30), 20 + minute as u8, 20 + minute as u8));
    }
    assert_eq!(sim.upload(), Light::Online, "back once the dwell ran out");
}

#[test]
fn one_account_fills_at_most_two_of_the_last_ten_and_one_failure() {
    let mut evidence: Vec<Evidence> = (0..12).map(|_| landed(t(1), 1, 1)).collect();
    evidence.extend((2..=9).map(|account| mailbox_down(t(2), account, account)));
    evidence.extend((0..5).map(|_| mailbox_down(t(2), 9, 9)));
    let reading = reading_of(&evidence, &[EPOCH], t(3));
    assert_eq!(reading.last.len(), 10);
    assert_eq!(reading.oks, 2, "account 1 fills two places, however many it sent");
    assert_eq!(reading.failures, 8, "account 9 counts one failure");
    assert!(reading.busy());
    // The newest first, within 60 minutes.
    let reading = reading_of(&evidence, &[EPOCH], t(62));
    assert_eq!(reading.last.len(), 8, "the landings of minute 1 are older than 60 minutes");
}

#[test]
fn evidence_no_key_period_links_counts_nowhere() {
    // Tries recorded under a key this process no longer holds (a restart,
    // or the previous key gone): not in the last 10, in no quorum.
    let old: Vec<Evidence> = (11..=16).map(|account| mailbox_down(t(1), account, account)).collect();
    let reading = reading_of(&old, &[8], t(2));
    assert!(reading.last.is_empty());
    assert!(reading.quorum.is_empty());
    assert_eq!(reading.offline_spread, 0);
    // But a landing among them still ends an Offline, and still counts as
    // news.
    let mut evidence = old.clone();
    evidence.push(landed(t(1), 1, 1));
    let reading = reading_of(&evidence, &[8], t(2));
    assert_eq!(reading.newest_landing, Some(6));
    assert_eq!(reading.newest_ok_at, Some(t(1)));
}

#[test]
fn offline_needs_three_accounts_on_three_networks_within_15_minutes() {
    // 3 on 3, within 15 minutes, at once (Busy on the way).
    let mut sim = online_at(0);
    sim.feed_all([mailbox_down(t(10), 11, 11), mailbox_down(t(15), 12, 12)]);
    assert_eq!(sim.upload(), Light::Busy);
    let events = sim.feed(mailbox_down(t(25), 13, 13));
    assert_eq!(sim.upload(), Light::Offline);
    assert_eq!(sim.overall(), Light::Offline);
    assert_eq!(events[0].reason, "3 accounts on 3 networks didn't get past the mailbox within 15 min");
    assert_eq!(events[0].cause, Cause::Rule);
    assert_eq!(sim.desk(), Light::Online, "the front desk answered every one");
    let quorum: Vec<i64> = sim.state.upload.quorum.iter().map(|voter| (voter.at - T0).whole_minutes()).collect();
    assert_eq!(quorum, [25, 15, 10]);

    // The 15-minute edge: the first failure 16 minutes before the third.
    let mut sim = online_at(0);
    sim.feed_all([mailbox_down(t(10), 11, 11), mailbox_down(t(15), 12, 12), mailbox_down(t(26), 13, 13)]);
    assert_ne!(sim.upload(), Light::Offline);
    // Off the minute: 15 min exactly counts, 15 min 1 s does not.
    let mut sim = online_at(0);
    sim.feed_all([mailbox_down(ts(10, 30), 11, 11), mailbox_down(ts(15, 0), 12, 12)]);
    sim.feed(mailbox_down(ts(25, 31), 13, 13));
    assert_ne!(sim.upload(), Light::Offline);
    let mut sim = online_at(0);
    sim.feed_all([mailbox_down(ts(10, 30), 11, 11), mailbox_down(ts(15, 0), 12, 12)]);
    sim.feed(mailbox_down(ts(25, 30), 13, 13));
    assert_eq!(sim.upload(), Light::Offline);

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
    let items = [(11, 20), (11, 21), (11, 22), (12, 20), (13, 20)]
        .map(|(account, network)| mailbox_down(t(10), account, network));
    assert_eq!(spread(&items.iter().collect::<Vec<_>>(), &[EPOCH], 3), 2);
    let mut sim = online_at(0);
    sim.feed_all(items);
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
    let mut sim = offline_at_12();
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
    assert!(sim.state.upload.quorum.is_empty());
    assert_eq!(sim.state.waking_since, Some(t(122)), "the end of Offline starts waking");
    // A Try anyway says so.
    let mut sim = offline_at_12();
    let events = sim.feed(Evidence { admission: Admission::TryAnyway, ..landed(t(14), 9, 9) });
    assert_eq!(events[0].cause, Cause::TryAnyway);
}

#[test]
fn a_landing_that_proves_the_front_desk_ends_its_offline() {
    // The front desk timed out over TLS for three people; then returns
    // land through a scheme that is not TLS (the front-desk window reads
    // `none`): they got their upload server from it, so it is up.
    let mut sim = online_at(0);
    sim.feed_all([desk_down(t(10), 11, 11), desk_down(t(11), 12, 12), desk_down(t(12), 13, 13)]);
    assert_eq!(sim.desk(), Light::Offline);
    let plain = Evidence { dispatcher: Window::None, ..landed(t(14), 2, 2) };
    assert_eq!(plain.hit(Side::Dispatcher), Some(Hit::Ok));
    let events = sim.feed(plain);
    assert_eq!(sim.desk(), Light::Online);
    assert_eq!(events[0].scope, Scope::Dispatcher);
    // A direct report that says it got in does not.
    let report = direct(Evidence { dispatcher: Window::None, ..landed(t(14), 2, 2) });
    assert_eq!(report.hit(Side::Dispatcher), None);
}

#[test]
fn a_landing_is_ordered_by_when_it_ended_not_when_it_came() {
    // 11, 12 and 13 refused at 10, 11 and 12: Offline. Then comes a long
    // send that got in at 11:30, between 12's failure and 13's.
    let mut sim = offline_at_12();
    let events = sim.feed(landed(ts(11, 30), 2, 2));
    assert_eq!(sim.upload(), Light::Online, "after it only 13 failed");
    assert_eq!(events[0].reason, "A return got in past the mailbox");
    // 11 and 12 failed before it and lose their vote; 13 failed after it,
    // which a landing does not disprove.
    let lost: Vec<u8> = sim.state.vote_losses.iter().map(|loss| loss.keys[0].1[0]).collect();
    assert_eq!(lost, [12, 11]);
    assert!(
        sim.state.vote_losses.iter().all(|loss| loss.side == Side::Upload && loss.until == ts(11, 30) + VOTE_LOST_FOR)
    );
}

#[test]
fn a_landing_outvoted_in_the_same_run_keeps_offline() {
    // Offline; then one run takes in a landing and, after it, three more
    // accounts on three networks failing: no Offline, Online, Offline pair.
    let mut sim = offline_at_12();
    let seq = sim.state.seq;
    sim.add(landed(ts(13, 10), 2, 2));
    for account in 14..=16 {
        sim.add(mailbox_down(ts(13, 20), account, account));
    }
    let events = sim.tick(t(14));
    assert!(events.is_empty(), "{events:?}");
    assert_eq!(sim.upload(), Light::Offline);
    assert_eq!(sim.state.seq, seq);
    // The quorum is the failures after the landing; the next landing ends
    // it, and those three lose their vote.
    let mut quorum: Vec<u8> = sim.state.upload.quorum.iter().map(|voter| voter.keys[0].1[0]).collect();
    quorum.sort();
    assert_eq!(quorum, [14, 15, 16]);
    assert!(sim.state.vote_losses.is_empty());
    sim.feed(landed(t(15), 3, 3));
    assert_eq!(sim.upload(), Light::Online);
    assert_eq!(sim.state.vote_losses.len(), 3);
}

#[test]
fn offline_turns_to_no_recent_news_after_three_quiet_hours() {
    let mut sim = offline_at_12();
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
    // Per window: the front desk's own quiet hour is not the mailbox's.
    let mut sim = offline_at_12();
    sim.feed(mailbox_down(ts(100, 40), 14, 14));
    sim.tick(ts(280, 39));
    assert_eq!(sim.upload(), Light::Offline);
    sim.tick(ts(280, 40));
    assert_eq!(sim.upload(), Light::NoRecentNews);
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
    // Off the minute.
    let mut sim = Sim::new();
    sim.feed(landed(ts(0, 45), 1, 1));
    assert!(sim.tick(ts(60, 44)).is_empty());
    assert!(!sim.tick(ts(60, 45)).is_empty());
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
    // And straight back to Busy, at once: tightening never waits.
    sim.feed_all([mailbox_down(t(13), 31, 31), mailbox_down(t(13), 31, 31), mailbox_down(t(13), 32, 32)]);
    assert_eq!(sim.upload(), Light::Busy);
    // Offline from Busy, at once too.
    sim.feed(mailbox_down(t(14), 33, 33));
    assert_eq!(sim.upload(), Light::Offline);

    // Busy to No recent news once nothing got in for an hour (always past
    // the dwell), however recent the last failure.
    let mut sim = online_at(0);
    sim.feed_all([mailbox_down(t(1), 11, 11), mailbox_down(t(1), 11, 11), mailbox_down(t(2), 12, 12)]);
    sim.tick(ts(59, 59));
    assert_eq!(sim.upload(), Light::Busy);
    sim.tick(t(60));
    assert_eq!(sim.upload(), Light::NoRecentNews);
    // ... but not before the dwell: Busy at 55 from landings at 0 and 50,
    // nothing got in after 50: Busy holds to 65, then No recent news.
    let mut sim = online_at(0);
    sim.feed(landed(t(50), 1, 1));
    sim.feed_all([mailbox_down(t(55), 11, 11), mailbox_down(t(55), 12, 12)]);
    assert_eq!(sim.upload(), Light::Busy);
    sim.tick(ts(64, 59));
    assert_eq!(sim.upload(), Light::Busy, "the dwell");
    sim.tick(t(110));
    assert_eq!(sim.upload(), Light::NoRecentNews);

    // The dwell off the minute: Busy at 2:40, Online not before 12:40.
    let mut sim = online_at(0);
    sim.feed_all([mailbox_down(ts(1, 10), 11, 11), mailbox_down(ts(2, 40), 12, 12)]);
    for account in 21..=30 {
        sim.feed(landed(t(3), account, account));
    }
    sim.tick(ts(12, 39));
    assert_eq!(sim.upload(), Light::Busy);
    sim.tick(ts(12, 40));
    assert_eq!(sim.upload(), Light::Online);
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
fn a_landing_that_ends_offline_takes_its_quorums_votes() {
    // A landing while Online or Busy takes nobody's vote.
    let mut sim = online_at(0);
    sim.feed_all([mailbox_down(t(10), 11, 11), mailbox_down(t(11), 12, 12), landed(t(12), 1, 1)]);
    assert!(sim.state.vote_losses.is_empty());
    // Offline: 11, 12 and 13 after that landing. A landing at 20 ends it:
    // the three lose their vote, whatever the age of their failure.
    sim.feed_all([mailbox_down(t(13), 11, 11), mailbox_down(t(13), 12, 12), mailbox_down(t(14), 13, 13)]);
    assert_eq!(sim.upload(), Light::Offline);
    sim.feed(landed(t(40), 2, 2));
    assert_eq!(sim.upload(), Light::Online);
    assert_eq!(sim.state.vote_losses.len(), 3);
    // Now 11, 12, 13 and 14 fail: only 14 votes.
    sim.feed_all([
        mailbox_down(t(41), 11, 11),
        mailbox_down(t(41), 12, 12),
        mailbox_down(t(42), 13, 13),
        mailbox_down(t(42), 14, 14),
    ]);
    assert_ne!(sim.upload(), Light::Offline);
    // 15 and 16 make three with votes.
    sim.feed_all([mailbox_down(t(43), 15, 15), mailbox_down(t(44), 16, 16)]);
    assert_eq!(sim.upload(), Light::Offline);

    // The loss lasts 24 hours from the landing, then the vote is back.
    let mut sim = offline_at_12();
    sim.feed(landed(t(13), 1, 1));
    let back = t(13) + VOTE_LOST_FOR;
    sim.feed(landed(back - Duration::minutes(30), 2, 2));
    sim.feed_all([
        mailbox_down(back - Duration::minutes(2), 11, 11),
        mailbox_down(back - Duration::minutes(2), 14, 14),
        mailbox_down(back - Duration::minutes(1), 15, 15),
    ]);
    assert_ne!(sim.upload(), Light::Offline, "11 has no vote yet");
    sim.feed(mailbox_down(back, 11, 11));
    assert_eq!(sim.upload(), Light::Offline);

    // A direct report that got in ends nothing and takes no vote.
    let mut sim = offline_at_12();
    sim.feed(direct(landed(t(13), 1, 1)));
    assert!(sim.state.vote_losses.is_empty());

    // Read back after a restart (no account id), the keys still match.
    let mut sim = offline_at_12();
    sim.feed(landed(t(13), 1, 1));
    let stored: SignState = serde_json::from_value(serde_json::to_value(&sim.state).unwrap()).unwrap();
    assert_eq!(stored.vote_losses[0].account_id, None);
    let reloaded = Evidence { account_id: None, ..mailbox_down(t(14), 13, 13) };
    assert!(stored.vote_losses[0].covers(Side::Upload, &reloaded));
    assert!(!stored.vote_losses[0].covers(Side::Dispatcher, &reloaded));
}

#[test]
fn a_lost_vote_is_one_entry_per_account_and_window_with_the_latest_end() {
    let voter = Voter::of(&mailbox_down(t(1), 11, 11));
    let mut losses = Vec::new();
    lose_vote(&mut losses, Side::Upload, &voter, t(100));
    lose_vote(&mut losses, Side::Upload, &voter, t(50));
    // The same account under the next key period too (both keys).
    let later =
        Voter::of(&Evidence { keys: vec![keys_in(8, 111, 111), keys_in(EPOCH, 11, 11)], ..mailbox_down(t(2), 11, 11) });
    lose_vote(&mut losses, Side::Upload, &Voter { account_id: None, ..later }, t(200));
    lose_vote(&mut losses, Side::Dispatcher, &voter, t(60));
    assert_eq!(losses.len(), 2);
    assert_eq!(losses[0].until, t(200));
    assert_eq!(losses[0].keys, [(EPOCH, [11; 8]), (8, [111; 8])]);
    assert_eq!(losses[1].until, t(60));

    // Past the most kept, those that end last stay.
    let mut state = SignState::new(t(0));
    for n in 0..(MAX_VOTE_LOSSES + 5) {
        let voter = Voter { account_id: None, keys: vec![(EPOCH, (n as u64).to_be_bytes())], at: t(0) };
        lose_vote(&mut state.vote_losses, Side::Upload, &voter, t(100) + Duration::minutes(n as i64));
    }
    let mut sim = Sim::new();
    sim.state = state;
    sim.tick(t(1));
    assert_eq!(sim.state.vote_losses.len(), MAX_VOTE_LOSSES);
    assert!(sim.state.vote_losses.iter().all(|loss| loss.until >= t(105)));
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
fn only_the_first_twenty_direct_reports_are_tried() {
    // Two relay accounts on two networks, then direct reports that add
    // nothing (all on the first relay network), then one that would make
    // three: beyond the first 20, it is not tried.
    let mut items = vec![mailbox_down(t(10), 11, 11), mailbox_down(t(10), 12, 12)];
    let mut directs: Vec<Evidence> =
        (0..DIRECT_CANDIDATES).map(|n| direct(mailbox_down(t(10), 30 + n as u8, 11))).collect();
    directs.push(direct(mailbox_down(t(10), 99, 99)));
    items.extend(directs.iter().cloned());
    assert_eq!(spread(&items.iter().collect::<Vec<_>>(), &[EPOCH], 3), 2);
    // Within the first 20 it is.
    items.remove(2);
    assert_eq!(spread(&items.iter().collect::<Vec<_>>(), &[EPOCH], 3), 3);
    // The same report twice is tried once.
    let twice: Vec<Evidence> = [11, 12]
        .map(|account| mailbox_down(t(10), account, account))
        .into_iter()
        .chain((0..30).map(|_| direct(mailbox_down(t(10), 40, 11))))
        .chain([direct(mailbox_down(t(10), 99, 99))])
        .collect();
    assert_eq!(spread(&twice.iter().collect::<Vec<_>>(), &[EPOCH], 3), 3);
}

#[test]
fn matching_finds_the_augmenting_path_and_stops_at_enough() {
    let pair = |account: u8, network: u8| ([account; 8], [network; 8]);
    // A takes n1 first; B has only n1, so A must move to n2.
    assert_eq!(matching(&[pair(1, 1), pair(1, 2), pair(2, 1)], 3), 2);
    // A chain of three moves: A {1,2}, B {2,3}, C {3,4}... D {1} last.
    let chain = [pair(1, 1), pair(1, 2), pair(2, 2), pair(2, 3), pair(3, 3), pair(3, 4), pair(4, 1)];
    assert_eq!(matching(&chain, 10), 4);
    // Stops at enough.
    assert_eq!(matching(&chain, 3), 3);
    assert_eq!(matching(&[], 3), 0);
    // Many on one network: one.
    let crowd: Vec<_> = (0..200).map(|account| pair(account, 7)).collect();
    assert_eq!(matching(&crowd, 3), 1);
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
fn five_failures_in_a_row_are_told_once() {
    let mut sim = online_at(0);
    // One account five times is one failure; four accounts are four.
    for _ in 0..5 {
        sim.feed(mailbox_down(t(1), 11, 20));
    }
    sim.feed_all((12..=14).map(|account| mailbox_down(t(2), account, 20)));
    assert!(sim.notices.is_empty());
    assert_eq!(sim.upload(), Light::Online, "the light does not move: one network");
    sim.feed(desk_down(t(3), 15, 20));
    assert_eq!(sim.notices, [Notice::FailuresInARow { failures: 5, since: Some(t(0)) }]);
    // More of the same run are not told again ...
    sim.feed_all((16..=20).map(|account| mailbox_down(t(4), account, 20)));
    sim.tick(t(5));
    assert_eq!(sim.notices.len(), 1);
    // ... but a new run after a landing is.
    sim.feed(landed(t(6), 1, 1));
    sim.feed_all((21..=25).map(|account| mailbox_down(t(7), account, 20)));
    assert_eq!(sim.notices.len(), 2);
    assert_eq!(sim.notices[1], Notice::FailuresInARow { failures: 5, since: Some(t(6)) });
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
    let evaluation = evaluate(&sim.state, &sim.inputs(t(65), false));
    assert_eq!(evaluation.note_ended, Some((Uuid::from_u128(0xabc), NoteEnd::Expired)));
    let events = sim.tick(t(65));
    assert_eq!(events.last().unwrap().cause, Cause::NoteExpired);
    assert_eq!(sim.note, None);
    assert_eq!(sim.overall(), Light::NoRecentNews, "the evidence is an hour old");
    assert_eq!(sim.state.waking_since, Some(t(65)));
    assert_eq!(sim.state.source(), "tries");

    // A relay return that got in after it began ends an outage note; one
    // that ended earlier in the same minute does not, nor a direct report.
    let mut sim = online_at(0);
    sim.note = Some(note(NoteKind::Outage, ts(5, 30), t(65)));
    sim.tick(ts(5, 30));
    sim.feed(landed(ts(5, 10), 1, 1));
    sim.feed(direct(landed(ts(5, 40), 2, 2)));
    assert!(sim.note.is_some());
    let events = sim.feed(landed(ts(5, 40), 1, 1));
    assert_eq!(events.last().unwrap().cause, Cause::NoteReturnGotIn);
    assert_eq!(sim.note, None);
    assert_eq!(sim.overall(), Light::Online);
    assert_eq!(sim.state.waking_since, Some(ts(5, 40)));

    // Maintenance: a return that got in during it does not end it; staff
    // hear of it, once.
    let mut sim = online_at(0);
    sim.note = Some(note(NoteKind::Maintenance, t(5), t(65)));
    sim.tick(t(5));
    sim.feed(landed(t(7), 1, 1));
    sim.feed(landed(t(8), 2, 2));
    assert!(sim.note.is_some());
    assert_eq!(sim.overall(), Light::Offline);
    assert_eq!(sim.notices, [Notice::LandedInMaintenance]);
    assert_eq!(sim.tick(t(65)).last().unwrap().cause, Cause::NoteExpired);

    // ... nor an "online" note.
    let mut sim = offline_at_12();
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
    let mut sim = offline_at_12();
    sim.note = Some(note(NoteKind::Outage, t(13), t(70)));
    sim.tick(t(13));
    sim.note = None;
    sim.tick(t(14));
    assert_eq!(sim.overall(), Light::Offline);
    assert_eq!(sim.state.waking_since, None);
}

#[test]
fn an_expired_note_never_seen_is_logged_and_changes_nothing() {
    let mut sim = online_at(0);
    sim.note = Some(note(NoteKind::Outage, t(1), t(2)));
    let evaluation = evaluate(&sim.state, &sim.inputs(t(3), false));
    assert_eq!(evaluation.note_ended, Some((Uuid::from_u128(0xabc), NoteEnd::Expired)));
    assert_eq!(
        evaluation.events,
        [Event {
            scope: Scope::Overall,
            from: "online".into(),
            to: "online".into(),
            cause: Cause::NoteExpired,
            reason: "A note ran out before Buwiz saw it in force".into(),
        }]
    );
    assert_eq!(evaluation.state.state, Light::Online);
    assert_eq!(evaluation.state.waking_since, None);
}

#[test]
fn the_relay_would_pause_is_told_and_nothing_pauses() {
    let mut sim = online_at(0);
    sim.feed_all([ours(t(1), 1, 1, Kind::HostKey), ours(t(2), 1, 1, Kind::HostKey), ours(t(3), 1, 1, Kind::HostKey)]);
    assert!(sim.notices.is_empty(), "one account");
    // Two accounts on one network: not yet.
    sim.feed(ours(t(4), 2, 1, Kind::UnknownHost));
    assert!(sim.notices.is_empty());
    sim.feed(ours(t(5), 3, 3, Kind::UnknownHost));
    assert_eq!(
        sim.notices,
        [Notice::RelayWouldPause {
            reason: "5 tries from 2 accounts on 2 networks met a fault on Buwiz's side that touches the BIR (a host key that is not pinned: 3, an unknown upload server: 2)".into()
        }]
    );
    assert!(!sim.state.relay.paused, "nothing pauses in this release");
    assert_eq!(serde_json::to_value(&sim.state).unwrap()["relay"]["paused"], false);
    assert_eq!(sim.overall(), Light::Online, "the faults are Buwiz's, not the BIR's");
    // Said again while it holds (the staff mail tells it at most daily).
    sim.tick(t(6));
    assert_eq!(sim.notices.len(), 2);
    // Two faults; three spread over more than 60 minutes; or kinds that
    // touch nothing: not.
    let mut sim = online_at(0);
    sim.feed_all([ours(t(1), 1, 1, Kind::HostKey), ours(t(2), 2, 2, Kind::HostKey)]);
    sim.feed(ours(t(62), 3, 3, Kind::HostKey));
    sim.feed_all((1..=5).map(|account| ours(t(70), account, account, Kind::SenderDown)));
    assert!(sim.notices.is_empty());

    // The resume, for the later staff page: a paused relay (written by a
    // later release) resumes, and faults from before count no more.
    let mut sim = online_at(0);
    sim.feed_all([
        ours(t(1), 1, 1, Kind::Protocol),
        ours(t(2), 2, 2, Kind::Protocol),
        ours(t(3), 3, 3, Kind::Protocol),
    ]);
    sim.state.relay = Relay { paused: true, since: Some(t(3)), reason: Some("made-up".into()), resumed_at: None };
    sim.notices.clear();
    let events = sim.run(t(4), true);
    assert_eq!(events[0].cause, Cause::RelayResumed);
    assert!(!sim.state.relay.paused);
    sim.tick(t(5));
    assert!(sim.notices.is_empty());
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
        sim.periods = vec![OLD];
        for account in 1..=3 {
            sim.feed(landed(before - Duration::minutes(5), account, account));
        }
        sim.feed_all([mailbox_down(before, 11, 11), mailbox_down(before + Duration::minutes(5), 12, 12)]);
        // Midnight: a new key, and the previous one kept (or, after a
        // restart, gone).
        sim.periods = if with_previous { vec![NEW, OLD] } else { vec![NEW] };
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
    // After a restart the new key has no previous: the tries before cannot
    // be compared, so they count nowhere.
    let mut sim = outage(false);
    sim.feed(in_new(mailbox_down(after, 13, 13), 13, 13, false));
    assert_ne!(sim.upload(), Light::Offline, "only 11 and 13 in the new key period");
    // Fresh evidence under the new key alone makes its own quorum.
    sim.feed(in_new(mailbox_down(after + Duration::minutes(1), 14, 14), 14, 14, false));
    assert_eq!(sim.upload(), Light::Offline);
    // Once the previous key is gone, the tries before midnight count
    // nowhere either.
    let mut sim = outage(true);
    sim.periods = vec![NEW];
    sim.feed(in_new(mailbox_down(after, 13, 13), 13, 13, true));
    assert_ne!(sim.upload(), Light::Offline);

    // During an outage, midnight changes nothing: still Offline until a
    // landing.
    let mut sim = offline_at_12();
    let midnight = datetime!(2026-11-11 16:00 UTC);
    sim.periods = vec![NEW];
    for minute in (0..120).step_by(20) {
        sim.feed(in_new(mailbox_down(midnight + Duration::minutes(minute), 11, 11), 11, 11, false));
    }
    assert_eq!(sim.upload(), Light::Offline);
    sim.feed(in_new(scout(landed(midnight + Duration::minutes(130), 4, 4)), 4, 4, false));
    assert_eq!(sim.upload(), Light::Online);
}

#[test]
fn a_gap_in_watching_is_its_own_change() {
    let mut sim = online_at(0);
    sim.watched = false;
    let seq = sim.state.seq;
    // Five minutes exactly is no gap; a second more is.
    let evaluation = sim.evaluation(ts(5, 0), false);
    assert_eq!(evaluation.not_watched, None);
    let evaluation = sim.evaluation(ts(5, 1), false);
    assert_eq!(evaluation.not_watched, Some((t(0), ts(5, 1))));
    assert_eq!(evaluation.state.seq, seq + 1);
    assert!(evaluation.events.is_empty(), "no state was changed by it");
    assert!(evaluation.must_write(&sim.state));
}

/// Each state, as a restart finds it: the stored sign read back, the tries
/// with only their recorded key (of a period no longer in memory), to the
/// minute and with no account id; then the clock runs on. Only the quiet
/// rules move it: never Offline to Online, never Online to Offline or Busy.
#[test]
fn a_restart_finds_every_state_as_it_was_and_ages_it() {
    let busy = {
        let mut sim = online_at(0);
        sim.feed_all([mailbox_down(ts(1, 20), 11, 11), mailbox_down(ts(2, 40), 12, 12)]);
        sim
    };
    let offline = {
        let mut sim = online_at(0);
        sim.feed_all([
            mailbox_down(ts(1, 5), 11, 11),
            mailbox_down(ts(2, 15), 12, 12),
            mailbox_down(ts(3, 25), 13, 13),
        ]);
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
    let disproved = {
        let mut sim = online_at(0);
        sim.feed_all([mailbox_down(t(1), 11, 11), mailbox_down(t(2), 12, 12), mailbox_down(t(3), 13, 13)]);
        sim.feed(landed(ts(4, 50), 1, 1));
        sim
    };
    for (name, sim, expect) in [
        ("online", online_at(0), Light::Online),
        ("busy", busy, Light::Busy),
        ("offline", offline, Light::Offline),
        ("quiet", quiet, Light::NoRecentNews),
        ("noted", noted, Light::Offline),
        ("disproved", disproved, Light::Online),
    ] {
        assert_eq!(sim.overall(), expect, "{name}");
        let stored: SignState = serde_json::from_value(serde_json::to_value(&sim.state).unwrap()).unwrap();
        assert_eq!(stored.visible(), sim.state.visible(), "{name}");
        assert_eq!(stored.vote_losses.len(), sim.state.vote_losses.len(), "{name}");
        let read_back: Vec<Evidence> = sim
            .evidence
            .iter()
            .map(|item| Evidence { account_id: None, port: None, at: to_minute(item.at), ..item.clone() })
            .collect();
        let mut state = stored;
        let restart = sim.state.evaluated_at + Duration::seconds(40);
        let mut changes = Vec::new();
        let mut minute = 0;
        while minute <= 240 {
            let now = restart + Duration::minutes(minute);
            let inputs = Inputs {
                evidence: &read_back,
                periods: &[EPOCH + 100],
                note: sim.note.as_ref(),
                resume_relay: false,
                now,
            };
            let evaluation = evaluate(&state, &inputs);
            for event in &evaluation.events {
                changes.push((minute, event.scope, event.from.clone(), event.to.clone(), event.cause));
                assert!(
                    event.to == "no_recent_news" || event.cause == Cause::NoteExpired,
                    "{name} at {minute}: {event:?}"
                );
            }
            assert_eq!(evaluation.not_watched, None);
            state = evaluation.state;
            minute += if minute < 5 { 1 } else { 4 };
        }
        let windows: Vec<(i64, Scope, String)> = changes
            .iter()
            .filter(|change| change.1 != Scope::Overall)
            .map(|(minute, scope, _, to, _)| (*minute, *scope, to.clone()))
            .collect();
        match name {
            // The last landing at 0: an hour later No recent news.
            "online" | "busy" | "noted" => {
                assert!(windows.iter().all(|(minute, ..)| (57..=64).contains(minute)), "{name}: {windows:?}")
            }
            // The front desk went quiet an hour after the last try; the
            // mailbox stays Offline until 3 hours have passed.
            "offline" => {
                assert_eq!(windows.len(), 2, "{windows:?}");
                assert!((57..=64).contains(&windows[0].0) && windows[0].1 == Scope::Dispatcher, "{windows:?}");
                assert!((177..=184).contains(&windows[1].0) && windows[1].1 == Scope::Upload, "{windows:?}");
            }
            "quiet" => assert!(windows.is_empty()),
            _ => {}
        }
        // Nothing goes back up or down by itself.
        assert_ne!(state.state, Light::Online, "{name}");
    }
}

#[test]
fn a_stored_sign_missing_fields_still_reads() {
    let sim = offline_at_12();
    let mut value = serde_json::to_value(&sim.state).unwrap();
    for field in ["version", "revision", "vote_losses", "streak_told", "maintenance_told", "waking_since"] {
        value.as_object_mut().unwrap().remove(field);
    }
    value["upload"].as_object_mut().unwrap().remove("quorum");
    value["relay"].as_object_mut().unwrap().remove("resumed_at");
    // And one a later release added.
    value["a_later_field"] = serde_json::json!({ "made": "up" });
    let read: SignState = serde_json::from_value(value).unwrap();
    assert_eq!((read.seq, read.state, read.upload.state), (sim.state.seq, Light::Offline, Light::Offline));
    assert_eq!(read.since, to_minute(sim.state.since));
    assert!(read.upload.quorum.is_empty());
    assert_eq!(read.version, SIGN_VERSION);
    // Without its quorum, an Offline is ended by a landing after the 15
    // minutes before it began.
    assert_eq!(read.upload.quorum_from(), read.upload.since - OFFLINE_WITHIN);
}

#[test]
fn the_revision_follows_what_a_reader_sees() {
    let mut sim = online_at(0);
    let revision = sim.state.revision;
    assert!(revision >= T0.unix_timestamp() * 1000, "at least the time of the change, in ms");
    // The clock alone moves nothing a reader sees.
    sim.tick(ts(0, 20));
    assert_eq!(sim.state.revision, revision);
    // A try in the same minute neither ...
    let before = sim.state.clone();
    sim.feed(landed(ts(0, 30), 6, 6));
    assert_eq!(sim.state.revision, revision);
    // ... but in the next minute "last got in" moves: a new revision, not a
    // stored change that must wait for nothing.
    sim.feed(landed(ts(1, 30), 7, 7));
    assert!(sim.state.revision > revision);
    let evaluation = sim.evaluation(ts(1, 40), false);
    assert!(!evaluation.must_write(&sim.state));
    assert!(!evaluate(&before, &sim.inputs(ts(1, 40), false)).must_write(&before), "times of news alone wait");
    // A change of state is written at once.
    sim.feed_all([mailbox_down(t(2), 11, 11)]);
    let evaluation = {
        sim.add(mailbox_down(t(2), 12, 12));
        sim.evaluation(t(2), false)
    };
    assert!(evaluation.must_write(&sim.state));
    assert!(evaluation.state.revision > sim.state.revision);
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
/// or after 3 hours without evidence on it; Online holds only while a
/// return got past within the hour; and every change is told.
#[test]
fn offline_never_ends_without_a_relay_landing_or_three_quiet_hours() {
    for seed in 0..40 {
        let mut rng = StdRng::seed_from_u64(seed);
        let mut sim = Sim::new();
        let mut at = t(0);
        for _ in 0..300 {
            at += Duration::seconds(rng.random_range(0..25 * 60));
            let account = rng.random_range(1..=6u8);
            let network = rng.random_range(1..=6u8);
            let mut item = match rng.random_range(0..10) {
                0..=1 => landed(at, account, network),
                2..=5 => mailbox_down(at, account, network),
                6 => desk_down(at, account, network),
                7 => cut(at, account, network),
                8 => own_network(at, account, network),
                _ => ours(at, account, network, Kind::HostKey),
            };
            if rng.random_bool(0.2) {
                item = direct(item);
            }
            let before = sim.state.clone();
            let tick = rng.random_bool(0.2);
            let landing = !tick && item.source == Source::Relay && item.counted;
            let landing_on = |side: Side| landing && item.hit(side) == Some(Hit::Ok);
            let events = if tick { sim.tick(at) } else { sim.feed(item.clone()) };
            for side in Side::BOTH {
                let was = before.lamp(side);
                let now = sim.state.lamp(side);
                if was.state == Light::Offline && now.state != Light::Offline {
                    let quiet = was.last_evidence_at.is_none_or(|last| at - last >= OFFLINE_QUIET);
                    assert!(landing_on(side) || (quiet && now.state == Light::NoRecentNews), "seed {seed}");
                }
                if now.state == Light::Online || now.state == Light::Busy && at - now.since >= DWELL {
                    let got_in = sim
                        .evidence
                        .iter()
                        .any(|item| item.hit(side) == Some(Hit::Ok) && at - item.at < QUIET && item.at <= at);
                    assert!(got_in, "seed {seed}: {:?} with no return past it within the hour", now.state);
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
            (31, "upload", "online", "busy", "rule"),
            (31, "overall", "online", "busy", "rule"),
            (33, "upload", "busy", "offline", "rule"),
            (33, "overall", "busy", "offline", "rule"),
            (120, "upload", "offline", "online", "scout"),
            (120, "overall", "offline", "online", "scout"),
            (122, "upload", "online", "busy", "rule"),
            (122, "overall", "online", "busy", "rule"),
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

/// Busy never outlasts the hour after the last return got in, however one
/// account keeps failing; and Online never goes Busy on failures alone.
#[test]
fn busy_leaves_once_nothing_got_in_for_an_hour() {
    // Two accounts on two networks push Online to Busy ...
    let mut sim = online_at(0);
    sim.feed_all([mailbox_down(t(1), 11, 11), mailbox_down(t(2), 12, 12)]);
    assert_eq!(sim.upload(), Light::Busy);
    // ... then one account fails every 30 minutes for a day, nothing gets
    // in: the light leaves Busy within about an hour, and stays off it.
    let mut left_at = None;
    for minute in (30..24 * 60).step_by(30) {
        sim.feed(mailbox_down(t(minute), 11, 11));
        for tick in 1..=59 {
            sim.tick(t(minute) + Duration::seconds(30 * tick));
        }
        if sim.upload() != Light::Busy && left_at.is_none() {
            left_at = Some(minute);
        }
        if left_at.is_some() {
            assert_eq!(sim.upload(), Light::NoRecentNews, "minute {minute}");
        }
    }
    let left = sim.log.iter().find(|(_, event)| event.scope == Scope::Upload && event.from == "busy").unwrap();
    assert_eq!((left.0, left.1.to.as_str(), left.1.cause), (t(60), "no_recent_news", Cause::Rule));
    assert!(left_at.is_some_and(|minute| minute <= 60));
    // The words stay true: failures, but no return got in.
    let words = crate::uptime::public::words(&sim.state, crate::uptime::public::Times::Public);
    assert!(words.line.starts_with("No return has got in during the last hour: some tries failed"), "{}", words.line);
    let overall = sim.log.iter().rev().find(|(_, event)| event.scope == Scope::Overall).unwrap();
    assert!(overall.1.reason.starts_with("No return got in during the last 60 min"), "{}", overall.1.reason);

    // Online whose last landing is an hour old goes to No recent news, not
    // Busy, when two accounts on two networks fail.
    let mut sim = online_at(0);
    sim.feed(mailbox_down(t(30), 11, 11));
    sim.feed(mailbox_down(ts(60, 10), 12, 12));
    assert_eq!(sim.upload(), Light::NoRecentNews);
    assert!(sim.log.iter().all(|(_, event)| event.to != "busy"));
    // Nor does No recent news go Busy without a landing within the hour.
    for minute in 61..90 {
        sim.feed(mailbox_down(t(minute), 13 + (minute % 5) as u8, 13 + (minute % 5) as u8));
    }
    assert_ne!(sim.upload(), Light::Busy);
}

/// A note that ends while the lights read No recent news says nothing went
/// offline: the state before the quiet is the evidence's, never a note's.
#[test]
fn a_note_ending_over_quiet_lights_says_nothing_went_offline() {
    let mut sim = Sim::new();
    sim.note = Some(note(NoteKind::Outage, t(0), t(30)));
    sim.tick(t(1));
    assert_eq!(sim.overall(), Light::Offline);
    sim.tick(t(30));
    assert_eq!(sim.overall(), Light::NoRecentNews);
    assert_eq!(sim.state.last_state, None, "the lights were quiet: no state before");
    let words = crate::uptime::public::words(&sim.state, crate::uptime::public::Times::Public);
    assert!(!words.line.contains("offline"), "{}", words.line);
    let document = crate::uptime::public::document(&sim.state, "", crate::uptime::public::Times::Public);
    assert_eq!(document.last_state, None);
    // An Offline the evidence made, quiet for 3 hours, still says so.
    let mut sim = offline_at_12();
    sim.tick(t(12) + OFFLINE_QUIET);
    assert_eq!((sim.upload(), sim.state.last_state), (Light::NoRecentNews, Some(Light::Offline)));
}
