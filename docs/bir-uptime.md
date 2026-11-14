# BIR uptime and the smart send queue

How Buwiz learns whether the BIR's servers take returns, and how every app
(the website, the mini app, the desktop app and the direct-to-BIR app)
hears it. One contract, v1. It is additive to
[relay-protocol.md](relay-protocol.md): no existing field, route, frame or
code changes meaning, and an app that knows nothing of this page keeps
working ([Compatibility](#12-compatibility)).

**What runs today.** The server's `BIR_UPTIME` setting decides how much of
this is switched on ([Settings](#13-server-settings)). From `record`: every
real send through the relay leaves one classified row per try
([The per-try record](#14-the-per-try-record)); from those rows the server
keeps the sign, both lights and the overall state, by the rules of §2.1,
in memory and in its database, with an event for every change; Admins set
a note by hand with the `buwiz-cloud bir` command (§15.4; no staff screen
yet); one line of day history is kept per Manila day; and Admins and
Support are mailed when the sign goes Offline or comes back
([What the server keeps](#15-what-the-server-keeps)). Apps and people see
none of it at `record`: every response is as it was. From `public`: the
read routes of §4 (`/api/bir/status`, `/api/bir/status/mine`,
`/api/bir/history`), the `buwiz-bir` header and the `bir` and `class`
fields of §3, the public status page and the My TINs line of §4.4, and
`mode` kept on a failed or unknown submission. Everything public (the
status document, the history, the page, and the header on the answers that
need no sign-in) is made from the sign as it stood at the last 5-minute
mark, so it can be up to 5 minutes behind; signed-in answers carry the
live sign. `enforce` does nothing more yet.

**Planned, not built yet:** the sign object's `scout_at`, `wait_until`,
`admission`, `queue` and `links.manual_filing` (left out of the answers,
not sent as nulls), `detail.yours` on `/mine`, `/api/bir/history/mine`
(staff-only counts), holds on tickets and the v1 relay (§5), turns, queue
reports, scouts and admission (§6; waking starts but nothing ends it yet),
the `bir_turn` and `bir_try_anyway` ticket fields, the direct app's reports
(§7), proofs and "Another way to file" (§8), the queue numbers (§9), the
codes of §10 but `class` and `rate_limited`, the `hold` field of the relay
`result`, the staff screens, and holding anything while the relay is
paused. Their tables exist (migration 14). The sections below describe them
as they will be, so app authors can build against them; each says what is
built.

## 0. Ground rules

- **No probing, ever.** Nothing here opens a connection to the BIR to learn
  its state: not the server, not an app, not a script. The sign is learned
  only from real returns that people are sending anyway. A "scout" is one
  real queued return, sent within its owner's own schedule when the server
  gives it the turn; it is never a test connection.
- **Every queue rule sends less.** The server can make apps wait longer,
  never send more, and no user's connection ever carries more tries than its
  own schedule would. Apps clamp every number the server tunes (section 9).
- **`unknown` is never resent automatically**, by any app, in any state.
- **No TIN, no name, no IP address** in any uptime record, header or
  response (section 11). The proof of outage (section 8) names a return by
  its masked TIN only.
- **Randomness only spreads load.** The spread in section 9 exists so a
  thousand queues that failed together do not come back together. Nothing
  here hides, disguises or shapes traffic toward the BIR.
- **All of this goes to Buwiz.** The sign rides on calls apps already make,
  plus one small queue report every 5 minutes while something is waiting.
  None of it goes to the BIR.

## 1. Words

| Word | Meaning |
|---|---|
| **Try** | One real send of one return: the relay's `open` frames through to its `result`, or one direct-app send. Only `live` tries can count. |
| **Window** | One of the BIR's two servers in a send. `dispatcher` ("front desk"): says which upload server to use. `upload` ("mailbox"): takes the return over SFTP. |
| **Light** | The state of one window. |
| **Sign** | Both lights, the overall state, the times, the note, the relay state and the waits. |
| **Class** | Whose side a failed try failed on: `bir`, `user_network` (the user's own connection) or `ours` (Buwiz's). |
| **Counted try** | A `live` try that got in or failed with class `bir` (and not `no_service`). Only counted tries move the sign. |
| **Turn** | Permission from the server for one queued return to try now. Kind `scout` (while Offline) or `admit` (while Busy or waking). Given only in the answer to a queue report (§6). |
| **Admission** | How a ticket got through: `open`, `admit`, `scout`, `legacy_scout` or `try_anyway`. |
| **Slot** | An opaque random id an app gives one queued return in its queue report. Never derived from the TIN, form or period. |

## 2. The sign

### 2.1 States

`state` is one of four values. An app that reads any other value treats it
as `no_recent_news`.

| `state` | Shown as | Meaning |
|---|---|---|
| `online` | Online | Returns land: of the last 10 counted tries (one failure per account), at most 1 failed, and the newest got in; and a return got in within the hour. |
| `busy` | Busy | Some get in, some don't: failures from at least 2 accounts on at least 2 networks among the last 10, and a return got in within the hour. |
| `offline` | Offline | Nothing gets in: 3 accounts on 3 networks failed within 15 minutes and none got in since. Or an Admin set an outage or maintenance by hand. |
| `no_recent_news` | No recent news | No return got in during the last 60 minutes: nobody filed, or some tries failed but from too few people to call it Offline. So we don't know. It claims neither up nor down. |

Busy and Online change with hysteresis: a change that lets more through
(`busy` to `online` or `no_recent_news`) waits until `busy` is 10 minutes
old; a change that lets less through happens at once. Offline ends only
when a relay return gets past that window, or after 3 hours with no try
reaching it. The rules exactly as the server runs them are in
[What the server keeps](#15-what-the-server-keeps).

Each window has its own light with the same four values. The overall
`state` is `offline` if either light is (or a hand-set outage or
maintenance note is in force), else `busy` if either is, else `online` if
the upload light is `online`, else `no_recent_news`. An Admin's "declare
Online" note (`note.kind = online`) makes it `online` until the note ends.

### 2.2 The sign object

All times are UTC, RFC 3339. In the public document, times of past events
(`since`, `last_*`, `evaluated_at`) are rounded down to 5 minutes, and the
document is the sign as it stood at the last 5-minute mark, so a quiet
day's single filing cannot be read or timed off it; a note's `until` is
rounded up to 5 minutes; the planned `scout_at` and `wait_until` will be
exact. Words (`words.*`) are English, in Manila time.

```json
{
  "v": 1,
  "seq": 4182,
  "state": "offline",
  "since": "2026-11-11T07:10:00Z",
  "evaluated_at": "2026-11-11T09:00:00Z",
  "source": "tries",
  "relay": "ok",
  "windows": {
    "dispatcher": { "state": "online",  "since": "2026-11-11T01:00:00Z", "last_ok_at": "2026-11-11T08:55:00Z" },
    "upload":     { "state": "offline", "since": "2026-11-11T07:10:00Z", "last_ok_at": "2026-11-11T07:00:00Z" }
  },
  "last_success_at": "2026-11-11T07:00:00Z",
  "last_state": null,
  "scout_at": "2026-11-11T09:13:41Z",
  "wait_until": "2026-11-11T09:13:41Z",
  "admission": false,
  "note": null,
  "words": {
    "title": "BIR is offline",
    "line": "The BIR's mailbox is offline since 3:10 PM. Buwiz holds returns until it is back."
  },
  "queue": { "...": "section 9" },
  "links": {
    "status_page": "https://ebirforms.buwiz.com/bir-status",
    "manual_filing": "https://ebirforms.buwiz.com/bir-status#another-way"
  }
}
```

| Field | |
|---|---|
| `v` | Contract version, `1`. |
| `seq` | Grows with every change to the sign and survives server restarts: each change makes it one more than before, and never less than the 5-minute mark of the change in Unix seconds (so it jumps at a new mark, and every server start moves it on once). A database restored from an older backup therefore passes, at its next start, any `seq` an app holds. A 64-bit integer: compare it as a number, never assume it grows by one. A client keeps the sign with the highest `seq` it has seen and ignores an older one (a cached response can carry an old sign, §3). |
| `state`, `since` | Overall state and when it began. |
| `evaluated_at` | When the server last ran its rules (on every recorded try and every 30 s). |
| `source` | `tries` (evidence) or `staff` (a hand-set note is in force). |
| `relay` | `ok`, or `paused`: Buwiz stopped sending because of a fault on its own side that would still touch the BIR (a changed upload-server key, an unknown upload server, front-desk answers it cannot read). While `paused`, new apps send nothing and, under `enforce`, tickets are held with `relay_paused`. Planned: this release never pauses, it mails staff that the relay would pause (§15.1). |
| `windows.dispatcher`, `windows.upload` | Each light: `state`, `since`, `last_ok_at` (the last counted try that passed this window; `null` if none in 90 days). |
| `last_success_at` | The last `live` return that got in. |
| `last_news_at` | The last counted try of any outcome. Staff only (`/mine`); never in the public document, where it would time a quiet day's single try. |
| `last_state` | With `no_recent_news` only: the state before the quiet began (`"offline"` after an outage nobody retried for 3 hours). Otherwise `null`. |
| `scout_at` | With `offline` only: the earliest time the next scout may go. `null` when no scout is planned (nobody eligible is waiting, or a maintenance note is in force). Scouts are at least 15 minutes apart and happen less often when few accounts wait (§6.4). |
| `wait_until` | While `offline`: the next `scout_at` (or the note's `until`). While `busy` or waking: the earliest time a ticket without a turn can expect a place. Otherwise `null`. Exact, never rounded onto shared marks. |
| `admission` | `true` while tickets need a turn: Busy, and waking (§6.3). |
| `note` | A hand-set note in force, or `null`: `{"kind": "outage" \| "maintenance" \| "online", "text": "…", "set_at": "…", "until": "…"}`. `text` is the Admin's words, at most 200 characters, and is public. |
| `words.title`, `words.line` | The server's sentence for the state, true for every app version. A client may show its own words instead. |
| `queue` | The queue numbers the server tunes (section 9). |
| `links` | The public status page and the "Another way to file" steps (§8). |

### 2.3 The compact sign (header)

Where a full object does not fit (section 3), the sign rides as one HTTP
header, an RFC 8941 dictionary:

```
buwiz-bir: v=1, seq=4182, state=offline, dispatcher=online, upload=offline, relay=ok, since=1794381000, until=1794388421, scout=1794388421, admit=?0, note=?0
```

Times are Unix seconds (`since` rounded as in §2.2). `until` is
`wait_until`; `scout` is `scout_at`; absent members are `null`. A client
that sees a higher `seq` and wants the note text or the words reads the
full object (§4.1).

**As built:** `v`, `seq`, `state`, `dispatcher`, `upload`, `relay`, `since`
and `note`, in that order; `until`, `scout` and `admit` are left out until
they are built:

```
buwiz-bir: v=1, seq=4182, state=offline, dispatcher=online, upload=offline, relay=ok, since=1794381000, note=?0
```

It is added from memory as the response leaves, whatever its status, and
nothing else about the response changes: not its body, not its caching.
On answers that need no sign-in (the trust list, the sealing key) it is
the sign of the last 5-minute mark, as the public document; on signed-in
answers (tickets, submissions, tax profiles) the live sign. The trust list
keeps `Cache-Control: public, max-age=300`, so a copy a cache kept can
carry a sign up to 10 minutes old; its `seq` is lower than the one a
client already has from a later response, and the client keeps the
higher.

## 3. How apps hear the sign

The sign rides on calls apps already make. The only new request is the
queue report while something is waiting (§6.1), and its answer carries the
sign too. A sleeping queue makes no other request.

| Response | Carries |
|---|---|
| `GET /.well-known/buwiz-trust.json` | header `buwiz-bir`. This response is `Cache-Control: public, max-age=300`, so its sign can be up to 5 minutes old; `seq` tells. |
| `GET /.well-known/buwiz-hpke.json` | header `buwiz-bir` |
| `POST /api/tickets` (every answer, 2xx or not) | header `buwiz-bir`; on a hold also body `bir` (full object) |
| `POST /api/v2/submissions`, `POST /api/submissions` | header `buwiz-bir` |
| relay `result` frame | fields `class` and then `bir` (the sign object, live, public times), from `public`. `bir` is the sign from before this try: its record is written after the app has its result (never delaying a send), so the try itself shows in the next sign, not this one. `bir` comes with every result of a live try and every one with a `class`; a lab try, or one whose mode is unknown, gets neither. |
| `GET /api/tax-profiles` | header `buwiz-bir` |
| `POST /api/bir/queue`, `POST /api/bir/reports` | body (full object) |
| `GET /api/bir/status`, `GET /api/bir/status/mine` | body (full object) |

**What an app decides from what.** Whether a return may be sent again is
decided from the result's `status` only: `sent` is done, `failed` may be
tried again on the return's own schedule (§9), and `unknown` is never sent
again by itself, whatever else the answer says. `class` only says whose
side a failure was on (an `ours` failure is not counted as a try, §10) and
`bir` only what the sign was before this try; neither ever turns a result
into one that may be resent.

**Before its first send decision** every app process reads the sign once
(`GET /api/bir/status`, or a queue report if something waits). It stores
the last sign and any turn in its settings table, so a restarted process,
or a one-shot sender on the same database, starts from them.

**How late can it be?**

| Where | Worst case |
|---|---|
| A ticket | None: the server holds or admits it at that instant. |
| The sign as computed | 30 s for changes that come from the clock alone (quiet hours, a note expiring); none for changes caused by a try (one that comes while a run holds the rules waits for the next, at most 30 s). |
| The public sign (the document, the history, the page, the header without sign-in) | Up to 5 minutes more: it is the sign as it stood at the last 5-minute mark. |
| A sleeping queue | 5 minutes (`report_s`): it learns from its next queue report. |
| A screen (cached read, §4.2) | About 60 s (`sign_read_s`); up to 5 minutes if its newest sign came from the cached trust list. |
| An app that cannot reach Buwiz | Shows the last sign with its age for `stale_after_s` (10 minutes, wall-clock time, so a sign read before the computer slept is not taken as fresh); after that "Can't reach Buwiz", treated as `no_recent_news`. |

## 4. Reading the sign

### 4.1 Public summary: `GET /api/bir/status`

- No sign-in. The same document for everyone. Served on every host.
- Answer `200` with the sign object (§2.2), an `ETag`, and
  `Cache-Control: public, max-age=15, stale-while-revalidate=30`.
  `If-None-Match` with the current `ETag` answers `304`.
- Rate limit: 120 requests a minute per IP address, held in memory only
  and never stored. Over it: `429 rate_limited` with `Retry-After`.
- Safe to expose: no counts, no account, no TIN, no host name, no IP; past
  times rounded to 5 minutes.
- `404` while the server's `BIR_UPTIME` setting is below `public`: apps
  treat that as "this server has no sign". Below `public` the route does
  not exist at all: the answer is the site's own 404, byte for byte what
  any unknown address gets.

**As built.** The document is the sign as it stood at the last 5-minute
mark (Manila's marks are UTC's), made once a mark: it changes only on
those marks, so polling it cannot time a single try, and it can be up to
5 minutes behind the live sign. The `ETag` is an opaque hash of its bytes
(24 hex digits in quotes); nothing else is in it. `If-None-Match` with it
(or `*`, or a weak `W/` form, or in a list) answers `304` with the same
`ETag` and caching. No `last_news_at`. The address counted is
`CF-Connecting-IP`, an IPv6 address by its /64; the history and the status
page count in the same 120. A `429` is `Cache-Control: no-store`, so no
cache keeps it for others. No `Vary`: the answer is the same for everyone.
Example:

```json
{"v":1,"seq":4182,"state":"offline","since":"2026-11-11T07:10:00Z","evaluated_at":"2026-11-11T09:00:00Z",
 "source":"tries","relay":"ok",
 "windows":{"dispatcher":{"state":"online","since":"2026-11-11T01:00:00Z","last_ok_at":"2026-11-11T08:55:00Z"},
            "upload":{"state":"offline","since":"2026-11-11T07:10:00Z","last_ok_at":"2026-11-11T07:00:00Z"}},
 "last_success_at":"2026-11-11T07:00:00Z","last_state":null,"note":null,
 "words":{"title":"The BIR is offline",
          "line":"The BIR's mailbox is offline since Nov 11, 3:10 PM. Returns can't get in until it's back."},
 "links":{"status_page":"https://ebirforms.buwiz.com/bir-status"}}
```

`note`, when one is in force: `{"kind":"outage","text":"…","set_at":"…"
(rounded down),"until":"…" (rounded up to 5 minutes)}`; `source` is then
`staff`. `words` name times in Manila time with the date, so they change
only with the sign.

### 4.2 Signed-in detail: `GET /api/bir/status/mine`

`Authorization: Bearer` (any app scope) or a web session. Rate limit: 60 a
minute per account. `Cache-Control: no-store`. The sign object with exact
times, plus (staff only, below):

```json
"detail": {
  "last_10": { "got_in": 7, "of": 10 },
  "today": { "tries": 214, "got_in": 190, "accounts": 61 },
  "yours": { "waiting": 3, "paused": false, "turn": null }
}
```

`yours` comes from this account's own queue reports (§6.1): its waiting
count across its devices, and a turn if one of its devices holds one.

**As built:** the live sign (not the 5-minute mark's). Everyone signed in
gets it with public times (5-minute marks) and no counts. Staff (Admin or
Support) get its times to the minute, `last_news_at` and `detail`:
`last_10` (the mailbox's last 10, as the rules last read them) and `today`
(counted tries of the Manila day, Buwiz-wide; `accounts` the larger of the
day's set in memory and the distinct keys within one key period); `yours`
is planned. A Bearer token of any scope (the BIR apps, Books, AI agents) or
the website session; none answers `401`, a banned account `403
account_banned`, by its token or its session alike. Every answer is
`Cache-Control: no-store`; no `ETag`, and never a `304`, whatever
`If-None-Match` says.

**The small cached read** (for screens): each client keeps one in memory:
one request shared by concurrent callers; answered from memory when younger
than `sign_read_s`; a failure kept 30 s; the last good sign answered for
`stale_after_s` when offline, marked stale. Ages are wall-clock time. Every
sign seen on any response (§3) replaces it when its `seq` is higher.

### 4.3 History: `GET /api/bir/history` and `GET /api/bir/history/mine`

`?from=YYYY-MM-DD&to=YYYY-MM-DD` (Manila days, at most 400; default the
last 90). The public path is the same for everyone, cacheable 5 minutes,
limited per IP as in §4.1; `/mine` (signed in, limited per account) adds
counts.

```json
{
  "v": 1,
  "days": [
    { "day": "2026-11-11", "state": "offline", "hours": "oooooooooooooooxxxxxxxxx", "offline_minutes": 530, "note": "outage" }
  ],
  "last_24h": "oooo…96 characters…xxxx"
}
```

- `hours`: 24 characters, one per Manila hour, the worst state in that
  hour: `o` online, `b` busy, `x` offline, `m` hand-set outage or
  maintenance, `n` no recent news, `-` no data.
- `last_24h`: 96 characters, one per 15 minutes, oldest first, ending at
  the current 15-minute mark.
- `day.state`: the worst state of the day, spelled out. `note`:
  `"outage"`, `"maintenance"`, `"online"` or absent.
- `/mine` adds per day `tries`, `got_in`, `failed` (counted tries).

Days come from the one-line-per-day history kept forever; today's line is
computed live.

**As built** (`/history/mine` is planned): at most **90** days a request
(the default: the 90 up to today); `from` after `to`, `to` after today, a
year outside 2000 to 2100, not a date, or `from` or `to` given twice: `400
bad_range` (`{"error":{"code":"bad_range","message":…}}`).
`Cache-Control: public, max-age=300`. Like the status document it is as at
the last 5-minute mark, and it comes from memory: made once a mark (a
failure tried again at most once a minute, the last one made served
meanwhile), so a request costs no query (a range older than the last 90
days reads only the day lines). Every day in the range has a line; a day
before the history began is `state: "none"` with `-` every hour. Each line
gives the worst state (and `not_watched` for a day Buwiz watched none of),
the hours (with `w`, an hour not watched at all), `online_minutes`,
`busy_minutes`, `offline_minutes`, `no_recent_news_minutes`,
`note_minutes` and `not_watched_minutes` (each rounded to 5 minutes), the
shares of the watched time in whole percent (`online_percent` and so on),
computed from those rounded minutes and null for a day watched less than
an hour, and `note`. No count of tries, accounts or networks: on a quiet
day they would point at one person. The last of the 96 letters of
`last_24h` is the quarter hour now running, or, right on a quarter-hour
mark, the quarter just past.

```json
{"v":1,"from":"2026-08-17","to":"2026-11-14",
 "days":[{"day":"2026-11-11","state":"offline","hours":"nnnnnnnnoooooooxxxxxxxxx","online_minutes":420,
          "busy_minutes":0,"offline_minutes":530,"no_recent_news_minutes":480,"note_minutes":0,
          "not_watched_minutes":10,"online_percent":29,"busy_percent":0,"offline_percent":37,
          "no_recent_news_percent":34}, …],
 "last_24h":"nnnn…96 letters…xxxx"}
```

### 4.4 Pages

| Address | Who |
|---|---|
| `https://ebirforms.buwiz.com/bir-status` | Public status page: the sign, the 24-hour bar and the month calendar (the same calendar view staff see, without counts), and "Another way to file" (§8). **Built** from `public`: the sign in large words with since when, both windows, a hand-set note, the last 24 hours, a calendar of the last 90 days (one link per day, `?day=YYYY-MM-DD`, so it works by keyboard and without script, each day named for screen readers; a word and a shape with every colour; "not watched" hatched and "no data" dashed), what each state means for a return and how Buwiz knows. Indexed; nothing personal. As at the last 5-minute mark, from the same sign and history in memory as the API (a view costs no query), up to 5 minutes behind; limited per address with the reads (120 a minute); `Cache-Control: public, max-age=60` when the request carries no cookie (nothing on it is the viewer's), else `no-store`. The site footer links it; the Bot host sends it there; the account host answers 404. "Another way to file" is planned. |
| `https://account.buwiz.com/tins` | A banner at the top of My TINs while the state is `busy` or `offline`, or a hand-set outage or maintenance note is in force. Not for `online` (a "declare Online" note too) or `no_recent_news`. Built, from `public`: the live sign's words (My TINs is signed in), what it means for queued returns, the note's text, and a link to the status page; the sign is read once per request, so the banner and its stylesheet always agree. |
| `/staff/bir` | The staff "BIR status" page. |

## 5. Holds on tickets

**Planned, not built yet.** No ticket or v1 relay is held at any level
today. Holds will be switched on with `BIR_UPTIME=enforce`, and only once no mini app
0.3.2 install remains ([Compatibility](#12-compatibility)). Until then the
server runs at `public` at most: the sign, queue reports, turns and scouts
all work, new apps follow them by themselves, and no ticket is ever refused
for the BIR's state.

### 5.1 When a ticket is held (under `enforce`)

| Sign | A ticket without a turn | With a valid turn (§6.2) | With `bir_try_anyway` (§5.3) |
|---|---|---|---|
| `online` without admission; `no_recent_news` | admitted (`open`) | admitted (`admit` or `scout`) | admitted (the flag is ignored) |
| `busy`, or `online` with `admission: true` (waking) | admitted (`open`) only into a free place of its share of the doorman's limit (§6.3); else held `bir_busy` | admitted | admitted, within its limits |
| `offline` | held `bir_offline`, except the legacy scout (§6.4) | admitted | admitted, within its limits |
| any, with `relay: paused` | held `relay_paused` | held `relay_paused` | held `relay_paused` |

A hold creates no ticket, so it never counts toward the ticket quotas
(30/hour per account and device, 50/day per TIN). The server keeps a record
of each held request for the account (so a proof of outage can show the
person tried, §8).

### 5.2 The hold answer

```http
HTTP/1.1 503 Service Unavailable
Retry-After: 841
buwiz-bir: v=1, seq=4182, state=offline, …

{"error": {"code": "bir_offline",
           "message": "The BIR is offline, so Buwiz held this return. Nothing reached the BIR. Wait until about 5:15 PM.",
           "wait_until": "2026-11-11T09:13:41Z",
           "retry_after": 841,
           "try_anyway": {"allowed": true, "next_at": null}},
 "bir": { …the sign object… }}
```

The words are true for every app version, including old ones that retry on
their own clock and the ones that put the return back to Draft (§12): no
hold message promises that the return "goes by itself".

Why `503` with a new code: every app version that can send treats a ticket
error whose code is not a TIN refusal as "not sent". What each version then
does is in §12.

A v1 submission (no ticket) is held when its relay opens: the `result` is
`failed` with the same message and **no code in brackets** (so v1 clients
may retry it), plus `"hold": {"code": "bir_offline", "wait_until": "…"}`,
`class: "ours"` and `bir`.

### 5.3 "Try anyway" for one return

A person, in the app, may push one return while it is held or asleep:

```json
POST /api/tickets   (signed)
{ …the usual fields…, "bir_try_anyway": true }
```

- **Only a person's own press in the app.** Never the queue, never an AI
  agent: no MCP or assistant tool can set it. The app refuses it for a
  return that is claimed (being sent) or has an `unknown` outcome.
- The app never repeats it: after one Try anyway, the button stays off for
  that return until the sign changes or 30 minutes pass.
- Server limits: one per account per 30 minutes, three per account per
  day, 30 across Buwiz per 15 minutes. Over a limit: `429
  try_anyway_limited` with `try_anyway.next_at`.
- It does not lift `relay: paused`.
- It is a real try: it counts as evidence and, if it lands, ends Offline
  like a scout. If it fails, it counts on that return's own schedule like
  any try.

## 6. Turns, queue reports and scouts

**Planned, not built yet**, as is `admission` (§6.3): today the server
gives no turn, ignores `bir_turn` and `bir_try_anyway`, and every ticket's
`bir_admission` stays NULL (recorded as `open`).

The server does not know what is waiting in apps. Apps tell it, with no TIN
or name, so it can hand out turns: scouts while Offline, and "a few at a
time, due soonest first" while Busy or waking.

### 6.1 The queue report: `POST /api/bir/queue`

Signed like `/api/tickets` (device key) for the desktop and mini app. The
direct app, which has no device key, sends it with its Bearer token
(client `buwiz-direct`).

```json
{
  "v": 1,
  "client": "desktop",
  "paused": false,
  "sending": null,
  "waiting": [
    { "slot": "q_3kV9Yq2n4m1ZbCwQx8sQ", "due": "2026-11-10", "queued_at": "2026-11-09T02:15:00Z", "next_try_at": "2026-11-11T09:30:00Z" },
    { "slot": "q_Hc0pT7wR2aZy1M9eVn4k", "due": "2026-11-25", "queued_at": "2026-11-10T03:00:00Z", "next_try_at": null }
  ]
}
```

| Field | |
|---|---|
| `client` | `desktop`, `mini` or `direct`. |
| `paused` | The person switched the queue to Pause. A paused queue is never a scout and gets no turns; its returns count as waiting (paused). |
| `sending` | The slot being sent right now, or `null`. |
| `waiting[].slot` | 16 random bytes, base64url, prefixed `q_`. Made when the return is queued, new on every requeue. |
| `waiting[].due` | The return's BIR due date (Manila date). Refused if more than a day before today. |
| `waiting[].queued_at` | When it was queued. |
| `waiting[].next_try_at` | When this return may next try on its own schedule, or `null` if now. A return is eligible for a turn only once this has passed. |

At most 20 slots per device. Send it when the queue changes (a return
queued, sent, cancelled, paused, resumed), and otherwise every `report_s`
(at most 300 s) while anything waits, or sooner when the last answer's
`next_report_by` says so. When the queue empties, send one final report
with `"waiting": []`. Nothing is sent while nothing waits. A report
replaces this device's earlier one; the server forgets a report 20 minutes
after it arrives.

Answer `200`:

```json
{
  "bir": { …the sign object… },
  "turn": { "id": "t_9QnZ…", "slot": "q_3kV9Yq2n4m1ZbCwQx8sQ", "kind": "scout",
            "not_before": "2026-11-11T09:13:41Z", "expires_at": "2026-11-11T09:18:41Z" },
  "next_report_by": "2026-11-11T09:10:00Z"
}
```

- `turn` is `null` or one turn for one of this device's eligible slots.
  Turns are given only here.
- `next_report_by`: report again by then. The server shortens it (down to
  60 s) while it plans to give this device a turn; apps clamp it to
  60–300 s.
- Errors: `400 bad_queue_report`; `401`/`403` as for any signed call.

### 6.2 Using a turn

Ask for the ticket with the turn's id, at or after `not_before`, before
`expires_at`:

```json
POST /api/tickets   (signed)
{ …the usual fields…, "bir_turn": "t_9QnZ…" }
```

The turn is spent on that ticket. An expired, spent or unknown turn is
ignored and the request is treated as one without a turn. The 201 answer
adds `"admission"` (§1). A turn always wins over a stored `wait_until`.
Once a ticket is admitted, the app completes the send whatever the sign
does meanwhile.

The direct app, which takes no tickets, uses a turn by trying at the turn's
time and naming it in its report (§7).

### 6.3 Admission: a few at a time, due soonest first

While `admission` is `true`:

- The server keeps a limit L of returns in flight: turns granted and not
  yet expired or used, plus tickets admitted in the last 3 minutes (the
  longest a failing try can take) whose send has no result. L starts at 3,
  goes up by 1 for every return that lands (at most 20), and halves (at
  least 1) for a BIR-side failure from an account or network that got a
  return in within the last hour. Failures from accounts and networks with
  no recent success do not shrink it.
- Of L, a share (a third, at least 1) is reserved for tickets from devices
  that have never sent a queue report, so old app versions are not starved.
  The rest goes to reported slots, due soonest first (earliest `due`, then
  earliest `queued_at`), one turn per device at a time, only to slots whose
  `next_try_at` has passed.
- A ticket without a turn is admitted only into a free place of the
  reserved share; otherwise it is held `bir_busy` (under `enforce`).
- `admission` turns on when the sign enters `busy`, when Offline ends, and
  when a hand-set outage or maintenance note ends ("waking"). Waking ends
  when fewer than L slots are waiting.

### 6.4 Scouts (while Offline)

- At most one scout in flight, and scouts at least 15 minutes apart
  (`scout_at`), the first 15 minutes after Offline began.
- **Who:** a slot from a device whose report is under 10 minutes old, not
  paused, whose own `next_try_at` has passed. An account scouts at most
  once per its own current wait (a failed scout counts on that return's own
  schedule, so its `next_try_at` moves on). Among those: an account other
  than the previous scout's; then the account scouted fewest times in this
  outage; then the slot due soonest; then the earliest report. The previous
  scout's account may go again only when it is the only eligible one.
  Relay apps come before the direct app.
- **Few accounts waiting:** scouts simply happen less often than every 15
  minutes, because nobody is eligible sooner. No user's connection ever
  carries more tries than its own schedule would.
- **How:** the server picks 5 minutes before `scout_at` and gives the turn
  in that device's next report answer (shortening its `next_report_by` to
  60 s). If the turn is not used by `expires_at`, the next eligible
  candidate gets one at its next report.
- **Legacy scout** (old apps send no reports): if no reported device is
  eligible, the first ticket request at or after `scout_at` is admitted as
  the scout (`legacy_scout`), from an account other than the previous
  scout's unless it is the only one. Ticket requests held in the last 30
  minutes count as "someone waiting", so `scout_at` stays set for them.
- **Nobody waiting:** no scout, `scout_at` is `null`, and the sign stays
  `offline` until a try lands. After 3 hours with no try at all, it reads
  `no_recent_news` with `last_state: "offline"` and holds stop.
- **Maintenance note:** no scouts until its `until`. An outage note keeps
  scouting.
- A scout that lands ends Offline at once and starts waking (§6.3).

### 6.5 "Tell me when it is back"

No server endpoint: the app remembers the request locally and tells the
person when the sign it reads turns from `offline` to anything else. The
server keeps no list of who asked.

## 7. The direct app's reports: `POST /api/bir/reports`

The direct-to-BIR app talks to the BIR itself, so the server sees none of
its traffic. It reads the sign before every front-desk request (§4.1),
follows sleep, waits and turns itself, and reports its tries.

```json
POST /api/bir/reports
Authorization: Bearer <access token of the buwiz-direct client, app scope>
buwiz-client: buwiz-forms/1.5.0

{
  "v": 1,
  "tries": [
    {
      "id": "0b8f6e6c-1e4d-4b9a-8f2d-6c1e8f0b11aa",
      "started_at": "2026-11-11T07:12:00Z",
      "ended_at": "2026-11-11T07:12:31Z",
      "outcome": "failed",
      "dispatcher": "ok",
      "dispatcher_tls": true,
      "upload": "fail",
      "failure_kind": "timeout",
      "turn": null,
      "try_anyway": false
    }
  ]
}
```

| Field | Values |
|---|---|
| `id` | A random UUID per try (dedupe). |
| `outcome` | `sent`, `failed` (nothing reached the BIR), `unknown` (the upload broke midway). |
| `dispatcher` | `ok`, `fail`, `none` (not reached). |
| `dispatcher_tls` | Whether the front-desk answer (or the last front-desk failure) was over HTTPS. Only HTTPS counts as front-desk evidence. |
| `upload` | `ok`, `fail`, `maybe` (the `unknown` case), `none`. |
| `failure_kind` | The kinds of relay tries ([Kinds](#142-classes-and-kinds)): `dns`, `refused`, `timeout`, `reset`, `tls`, `http`, `ssh`, `login`, `cut`, `no_service`, `host_key`, `unknown_host`, `protocol`, `local`, `other`. The server maps them to the same three classes. |
| `turn` | The turn id this try used, if any. |

- The server stamps each try with the network of the request. A try sent
  more than 2 minutes after it ended is marked late, and a late try's
  network does not count toward "3 networks".
- At most 50 tries per request, one request a minute, 60 tries an hour per
  account; tries older than 24 hours or in the future are ignored.
- Counted only for accounts with an approved, real (not test) TIN; others
  get `200` and are ignored.
- Answer `200 {"accepted": n, "ignored": m, "bir": { …sign… }}`. Errors:
  `400 bad_report`, `429 report_limited`.

**Why the server trusts it only partly.** A report is the app's word: the
server cannot see the connection. So a direct-app try:

- is at most one of the three accounts that turn the sign Offline;
- never counts as "a return got in since" (it cannot hold Offline off) and
  never ends Offline;
- counts in Busy and Online like any try, at most 2 per account in the
  last 10.

## 8. Due dates: the alarm and the proof of outage

**The alarm** is the app's: it knows its returns' due dates. When a waiting
return is due today or earlier (Manila) and the sign is `offline`, or
`busy` for more than an hour, the app shows "N returns are due today and
the BIR is down" with **Filing steps** (opens `links.manual_filing`) and
**Proof of outage**. Until the steps are approved, that part of the status
page is a placeholder that says so.

**The proof:** `POST /api/bir/proofs` (Bearer, app scope, or web session):

```json
{ "day": "2026-11-10", "tin": "123-456-789-00000", "form": "1601C", "period": "102026",
  "queued_at": "2026-11-09T02:15:00Z" }
```

- The TIN must be one the caller holds or held (a claim on the account);
  it is used only to find the return's records and is never stored in the
  proof record or any uptime table.
- `201` with a signed record:
  `{"id": "p_…", "url": "https://ebirforms.buwiz.com/bir-status/proof/p_…", "jws": "<compact JWS>"}`.
- The JWS (`alg: EdDSA`, `typ: buwiz-outage-proof+jwt`, header `kid`; keys
  at `GET /.well-known/buwiz-outage-keys.json`) carries `iss`, `jti`,
  `iat`, `day`, and:
  - `return`: `tin_masked` (`***-***-789-00000`), `form` and `period` as
    stored;
  - `observed`: what Buwiz observed that day: the outages (`{from, to,
    window, source}`), Buwiz-wide counted tries (`tries`, `failed`,
    `accounts`, `networks`), this account's own tries of that return
    (times and outcomes, from its submissions) and its held requests for it
    (times and codes);
  - `reported_by_app`: `queued_at` as the app gave it, marked as the app's
    word.
- Every claim on the page reads "Buwiz observed …" or "Your app reported
  …". The `url` page is `noindex`, shows the same facts for printing, and
  lets anyone with the link check the signature.
- `409 no_outage_on_day` when Buwiz observed no outage that day; `403
  tin_not_active` for a TIN that is not the caller's; `429 proof_limited`
  after 10 proofs per account per day.

## 9. The queue numbers

The sign's `queue` object. The server may change these without an app
release (`BIR_QUEUE_POLICY`), only toward sending less. Apps clamp every
value and use the default when a value is missing or outside its range.

```json
"queue": { "waits_s": [300, 1800, 3600, 7200], "repeat_s": 7200, "spread": 0.5,
           "notice_after_s": 86400, "sign_read_s": 60, "report_s": 300, "stale_after_s": 600 }
```

| Key | Default | Client clamp | Meaning |
|---|---|---|---|
| `waits_s` | 300, 1800, 3600, 7200 | exactly 4 entries; each at least its default and at most 21600 | The wait after a return's 1st, 2nd, 3rd and 4th failed try. |
| `repeat_s` | 7200 | 7200–21600 | Every wait after that. |
| `spread` | 0.5 | 0.2–0.5 | Each wait is multiplied by a random factor, uniform between `1 − spread` and `1 + spread`. With 0.5: 5 minutes becomes anywhere from 2.5 to 7.5. |
| `notice_after_s` | 86400 | 3600–604800 | Tell the person a return is still waiting after this long, and again each time this long passes. |
| `sign_read_s` | 60 | 30–600 | Screens' cached sign read. Not used by a sleeping queue. |
| `report_s` | 300 | 60–300 | Queue report interval while anything waits. |
| `stale_after_s` | 600 | at least 2 × `sign_read_s`, at most 3600 | How long an unreachable server's last sign is still believed. |

**Fixed in every app** (not tunable):

- One return in flight at a time, per database.
- A return's wait starts when its try ends. If a `wait_until` is later,
  the try goes at `wait_until` plus a random share (0 to `spread`) of the
  time that was left until it, so held queues do not all return at once.
- A try counts on the return's schedule when it failed with class `bir` or
  `user_network`. A hold, a sign-out, Buwiz being unreachable, a TIN
  refusal or a failure of class `ours` is never a try.
- `unknown` is never resent by itself.
- While the sign is `offline` or `relay` is `paused`, no try except with a
  scout turn or a person's Try anyway; the sleep is not counted.
- Never before `wait_until`, unless a turn says so.
- When Offline ends, every return's back-off from before the outage resets.
- Try anyway only from a person; Pause stops everything.

## 10. Codes

New codes. The `message` is what every app version shows, so it is true
for all of them.

| HTTP | `code` | Where | `message` |
|---|---|---|---|
| 503 | `bir_offline` | tickets; v1 relay `hold` | The BIR is offline, so Buwiz held this return. Nothing reached the BIR. Wait until about {h:mm AM}. |
| 503 | `bir_offline` (maintenance note) | same | The BIR has planned maintenance until about {h:mm AM}, so Buwiz held this return. Nothing reached the BIR. |
| 503 | `bir_busy` | tickets; v1 relay `hold` | The BIR is busy, so Buwiz lets returns in a few at a time. Nothing reached the BIR. Wait until about {h:mm AM}. |
| 503 | `relay_paused` | tickets; v1 relay `hold` | Buwiz has paused sending because of a problem on our side. Nothing reached the BIR. We are fixing it. |
| 503 | `restarting` (built, every level) | tickets, submissions, the relay upgrade, while the server stops (§15.3); with `Retry-After: 30` | Buwiz is restarting. Nothing was sent; try again in a moment. |
| 429 | `try_anyway_limited` | tickets | You used Try anyway a moment ago. You can use it again at {h:mm AM}. |
| 429 | `rate_limited` | `/api/bir/status`, `/api/bir/history` (and `/mine`) | Too many requests. Try again in a minute. |
| 429 | `report_limited` | `/api/bir/reports` | Too many reports. Send them again in a minute. |
| 429 | `proof_limited` | `/api/bir/proofs` | Too many proofs today. Try again tomorrow. |
| 400 | `bad_queue_report` | `/api/bir/queue` | The queue report is not valid: {what}. |
| 400 | `bad_report` | `/api/bir/reports` | The report is not valid: {what}. |
| 409 | `no_outage_on_day` | `/api/bir/proofs` | Buwiz observed no BIR outage on {day}. A proof is only made for a day it did. |

Times in messages are Manila time, rounded to the next 5 minutes for
reading; the `wait_until` field is exact. None of these codes is a TIN
refusal.

**`class`** appears on the relay `result` (`bir`, `user_network` or
`ours`; absent for `sent` and `dry_run`, and for a try in `lab` mode) and
on `500 internal` and `relay_paused` error bodies (`ours`). Apps do not
count an `ours` failure as a try.

App-side words (shared by the desktop and mini app):

| Situation | Queue line |
|---|---|
| Offline, asleep | BIR is offline · goes by itself when it's back |
| Busy, waiting for a turn | BIR is busy · tries again in about 6 min |
| Waiting behind another | Goes right after the one above |
| Failed on the BIR's side | The BIR's mailbox didn't answer. Tries again in about 30 min. |
| Others get in, this device does not | Returns are getting in for others. The problem may be your own connection. Tries again in about 2 h. |
| No upload service for this TIN | The BIR has no upload service for this TIN right now. It tries again later; you can file another way. |
| Buwiz paused sending | Buwiz paused sending · nothing was sent |
| Paused | Your queue is paused · press Auto to send |
| Still waiting after 24 h | Still waiting after 24 hours. It keeps trying by itself; you can cancel it or file another way. |

## 11. Privacy

- No uptime record (`bir_tries`, `bir_days`, `bir_events`, `bir_notes`,
  `bir_queue_reports`, `bir_turns`, `bir_held`, `bir_host_keys`,
  `bir_proofs`) holds a TIN, a name or an IP address. Four columns hold a
  staff member's email, for the audit of what an Admin did:
  `bir_events.actor_email` (who set or cleared a note, or resumed the
  relay), `bir_notes.set_by_email` and `bir_notes.cleared_by_email`, and
  `bir_host_keys.confirmed_email` (who confirmed a key). No column holds a
  user's email. `bir_tries` times are kept to the minute, and its rows 90
  days (deleted by the hourly upkeep).
- **The app label.** `bir_tries.client` keeps the `buwiz-client` header
  only as a known app name (`desktop`, `mini`, `direct`, `buwiz-forms`,
  `buwiz-desktop`, `buwiz-ebirforms`, `buwiz-direct`, else `other`) and the
  version's `major.minor` (`desktop/0.2`), so nothing else a header holds
  is kept.
- **Network key.** The server sees the caller's address only in the
  Cloudflare tunnel's `CF-Connecting-IP` header (nothing on the instance
  listens publicly, [deploy.md](deploy.md)). It reduces the address to its
  network (IPv4 /24, IPv6 /48) and keeps `HMAC-SHA256(day_key, network)`
  cut to 8 bytes. `day_key` is random, held only in memory, with a random
  id (its key period, `key_epoch`), and replaced at the first try recorded
  after Manila midnight and on restart. Keys are taken when a try is
  recorded and only turn forward, so a try that started before midnight
  and ended after it gets the new day's. The previous key is kept in memory
  for 3 hours after it was replaced, so a new try can be compared with
  recent ones. After that, no one can tell which network a key was. The
  address and the network are never written anywhere.
- **A missing `CF-Connecting-IP`.** In production every request carries
  it. A request without it is counted as one network ("missing"), which
  would make every try one network; the server logs it as an error at most
  once an hour and mails Admins and Support (§15.6).
- **Staff mail** carries Manila times, states and counts only: never a
  TIN, a name, an address or an account of anyone who filed. A host-key
  fingerprint in it is marked "relay-reported, unverified: do not pin from
  this mail".
- **Account key.** Uptime tries keep `HMAC-SHA256(day_key, account_id)`
  (8 bytes), never the account id. Live turns and queue reports keep the
  account id for at most a day (scouts rotate between accounts); held
  requests keep it, with the claim they were for, for 90 days, for proofs.
- Rate limits per IP (an IPv6 address counted by its /64) and the day's
  distinct-account and distinct-network counts are kept in memory only. A
  limiter holds at most 100,000 addresses a minute; past that, new
  addresses share one overflow bucket with the same limit, so a flood of
  new addresses never locks out those who come after it. The address is
  `CF-Connecting-IP`: the zone's Cloudflare Pseudo IPv4 setting must stay
  "Off" or "Add header", since "Overwrite headers" would replace an IPv6
  visitor's address with a made-up IPv4 one, so a /64 would no longer count
  once and every network key would be wrong.
- Public answers carry no counts, their times are on 5-minute marks, and
  they are made from the sign as it stood at the last mark, so nobody
  polling them can time a single try; their `ETag` is a hash of their
  bytes and `seq` moves on nothing finer than 5-minute marks. A hand-set note's text is public; the form says so and asks for
  no personal data.
- The proof of outage is made only by the person, for their own return,
  and names it by its masked TIN.

## 12. Compatibility

**What each released version does with a hold**:

| Version | With a `503 bir_*` on the ticket |
|---|---|
| Desktop v0.1.0 | Cannot send at all (it has no trust key, so no trust list verifies), so it never meets a hold. |
| Mini 0.3.0, 0.3.1 | Cannot send at all (no trust key). |
| Mini 0.3.2 | A queued **1601C or 2551Q** is stranded: an error after the claim keeps the claim, so the return shows "BIR may already have this return" and never retries. Other forms back off and retry. |
| Mini 0.3.3 | A plain failed try: retries after 1, 2, 4, 8 minutes, and after the fifth goes back to Draft with the message (about 15 minutes of holds), in Busy and waking too. |
| Mini 0.4.0 | Same as 0.3.3. |

So `enforce` waits until no mini 0.3.2 install remains, and its release
gate runs 0.3.2, 0.3.3 and 0.4.0 against a local server. New builds send
`buwiz-client: <app>/<version>` on every request.

**New apps on an old server** (no sign; `/api/bir/*` answer `404`): the
smart queue without the sign: the waits of section 9, the spread, one at a
time, stay queued, no sleeping. No queue reports for an hour after a
`404`. **At `record`:** the same. **At `public` and `enforce` in this
release:** the sign can be read (§4) and rides on responses (§3); queue
reports, turns and holds come with the releases that build them.

**Versioning:** fields and codes may be added to v1. A client ignores
fields it does not know and treats an unknown `state` as `no_recent_news`,
an unknown turn `kind` as no turn, an unknown `class` as `bir`, and an
unknown error code as a plain failed try. A change that would break this
goes to `/api/bir/v2/…` and `v=2`.

**Relay frames:** the relay `Control` type does not change. The `result`
frame's new `class` field (and the planned `bir` and `hold`), and an app's `refused.kind`
(`"dns" | "refused" | "timeout" | "unreachable" | "denied" | "not_listed"`),
are extra JSON keys that every `Control` reader in use ignores (none denies
unknown fields; an extra key on `refused` and `result` parses, an unknown
`type` fails and is ignored). The server reads them from the raw JSON with
its own types. The Sender's `sender` frame stops at the Gatekeeper and
never reaches an app ([relay-protocol.md](relay-protocol.md#6-the-sender)).

## 13. Server settings

Gatekeeper (`buwiz-cloud`):

| Setting | Default | |
|---|---|---|
| `BIR_UPTIME` | `record` | `off`: nothing new runs (the Sender's `sender` frame is still dropped); no sign is kept and no staff mail is sent. `record`: every relay try is recorded and classified (§14), the sign is kept with its events, the day history is written and Admins and Support are mailed (§15); clients see no change. `public`: plus the read routes of §4, the `buwiz-bir` header, `class` and `bir` on the relay `result`, `mode` on failed and unknown submissions, the status page, the My TINs line and the footer link; below it none of these exist (their addresses are the site's own 404, every other response as it was). Planned: the routes of §6 to §8. `enforce`: today the same as `public`; planned: holds on tickets and the v1 relay, the legacy scout and Try anyway. Read in any case, spaces around it ignored. A value it does not know is logged and read as `record`; it never stops the server from starting. |
| `BIR_UPTIME_UPLOAD_PORTS` | `22` | Comma-separated ports whose relay streams are the upload window; every other stream is the front desk. A value it cannot read is logged and the default is used. |

Going down a level is a setting and a restart. `off` and back on again is
a restart after a gap: the time in between is "not watched" (§15.3).

## 14. The per-try record

The Gatekeeper sees every frame of a relay send (it bridges the app to the
Sender, [relay-protocol.md](relay-protocol.md) §6), so it records each try
without touching the BIR or the private relay code: `src/uptime/observe.rs`
watches the frames, `src/uptime/classify.rs` names the outcome,
`src/uptime/sshkex.rs` reads the upload server's host key from the SSH
handshake, which passes in clear before encryption starts, and
`src/uptime/store.rs` writes one row. Recording never fails or delays a
send: the app gets its `result` first, and the row is written afterwards
in a task of its own (a stopping server still takes and waits up to 10 s
for those, §15.3). An error is logged without the try's details and the
try is lost.

### 14.1 What the observer reads

| Frame (in the Gatekeeper's bridge) | Read as |
|---|---|
| The Sender's first text frame `{"type":"sender","mode":"live"}` | The relay mode (`live`, `lab`, `dry_run`). Dropped there, never forwarded to the app, whatever its other fields hold. A try without it is `mode: unknown`, treated like `lab`: no class, never counted, nothing told to the app. |
| Sender `open{stream,port}` | A stream: `upload` if its port is in `BIR_UPTIME_UPLOAD_PORTS`, else `dispatcher` (front desk), which is TLS on port 443. In `lab` every stream is the upload. Host names are not kept. |
| App `opened` / `refused{reason[,kind]}` | The stream's answer. Forwarded unchanged. A frame is known by its `type`; a field of an unexpected type reads as absent (a `refused` with an odd `kind` is read from its `reason`). |
| App binary data | Bytes from the far end; on an upload stream, the SSH handshake (the first 64 KB at most). |
| Neither answer before the Sender's next `open` or its `result` | The stream had no answer (the Sender's 20 s timeout). Every app answers within 10 s, so this is the app's side. |
| The app's socket closing before `result` | The app went away. |
| No `result` within 60 s after the app went away | The outcome wait expired: the app's side, not the Sender's. |
| Sender `result` | The outcome and its error text. |

A text frame longer than 4096 bytes, from either side, is not read at all:
it is forwarded exactly as before, and the observer neither builds it nor
takes anything from it. Every control frame is under 1 KB, while an app may
send a text frame of up to 64 MiB.

### 14.2 Classes and kinds

Precedence: `sent` wins; then an `unknown` result from the Sender (the
user's side when the app also went away); then the Sender being lost (the
user's side when the outcome wait expired after the app went away); then
the app going away; then the table.

| Seen | `stage` | `dispatcher` | `upload` | class / kind |
|---|---|---|---|---|
| `sent` | `uploaded` | `ok` if the front-desk stream that answered was TLS, else `none` (lab: `none`) | `ok` | none |
| `unknown` from the Sender | `upload` | as reached | `maybe` | `bir` / `cut`: counts toward Busy, never toward Offline. When the app also went away: the outcome stays `unknown` and the upload `maybe`, but `user_network` / `app_gone`, not counted |
| The Sender stopped without a result | as reached | `none` | `maybe` if the upload was opened, else `none` | `ours` / `sender_down` |
| The app went away, and no result came within the 60 s wait | as reached | `none` | `maybe` if the upload was opened, else `none` | `user_network` / `app_gone` (outcome `unknown` once a stream was opened) |
| The app went away before `result` | as reached | `none` | `none` | `user_network` / `app_gone` |
| No `open` at all | `none` | `none` | `none` | `ours` / `sender_down` (Sender unreachable; recorded with mode `unknown` but still classified, as Buwiz's own side), `not_built_in`, `refusal` (a TIN refusal), `mismatch` (`return_mismatch`), `ticket` (ticket or envelope), `other` |
| A `refused` "is not on the signed trust list" or "is not a BIR server" (or `kind: not_listed`), when no later stream got further (opened, or was the upload) | as reached | `none` | `none` | `ours` / `trust_list` |
| A `refused` with `kind: denied` or `unreachable`, or an OS error that is the user's own (permission denied, network unreachable or down, by number or words), or a reason it cannot read; when no later stream got further | as reached | `none` | `none` | `user_network` / `local` |
| A stream the app never answered (front desk or upload) | as reached | `none` | `none` | `user_network` / `no_answer`, when it is the stream that decides |
| Every front-desk stream refused or failed; no upload `open` | `dispatcher` | `fail` if a TLS stream was tried, else `none` | `none` | `bir` / `dns`, `refused`, `timeout`, `reset`, `tls`, `http`: the first TLS stream's, else the first stream's (local and trust-list refusals a later stream got past are skipped). "No route to host" and "Host is down" are `refused`; `timeout` only when the app's reason says it timed out. |
| A TLS front-desk attempt said "no upload server for this TIN (mode 0)" | `dispatcher` | `none` | `none` | `bir` / `no_service`: evidence for no light |
| A TLS front-desk attempt's answer "was not understood" or "did not unwrap" | `dispatcher` | `none` | `none` | `ours` / `protocol`: touched the BIR |
| A plain-HTTP (port 80) attempt said either of those two | | | | Proves nothing (a middlebox can answer on port 80): read as `user_network` / `local` for that stream; a TLS stream decides when there is one |
| An upload `open` happened | `upload` | `ok` if the front-desk stream that answered (the last with bytes before it) was TLS, else `none` | the rows below | |
| Upload `refused` | `upload` | | `fail` | `bir` / `refused`, `timeout`, `dns` |
| Upload opened; "host key is not pinned" | `upload` | | `none` | `ours` / `host_key`: touched the BIR; the key offered is recorded |
| Upload opened; SSH found "no common … algorithm" | `upload` | | `none` | `ours` / `protocol`: Buwiz's own SSH settings; touched the BIR |
| Upload opened; "invalid SSH version string" | `upload` | | `none` | `user_network` / `local`: what answered is not an SSH server (a proxy or portal on the user's way) |
| The front desk named an upload server not on the Sender's list ("unknown upload server"; nothing is opened to it) | `dispatcher` | `ok` if the stream that answered was TLS, else `none` | `none` | `ours` / `unknown_host`: touched the BIR |
| Upload opened; any other failure (SSH failed, login refused, before the upload, timed out) | `upload` | | `fail` | `bir` / `ssh`, `login`, `timeout` |
| "could not encrypt", "no lab SFTP server set" | any | | | `ours` / `other` |

An app's refusal is read by its `kind` when it sends one, then by the OS
error's number in "(os error N)", which reads the same in every language
(macOS, Linux and Windows numbers: 61/111/10061 refused, 60/110/10060
timed out, 65/113/10065 and 64/112/10064 no route or host down, 54/104/10054
reset, 51/101/10051 and 50/100/10050 the user's network, 13/1/10013 the
user's own system, 11001/11002/11004 a name that does not resolve), and
only then by the English words of each OS's message.

`counted = mode = 'live' AND (outcome = 'sent' OR failure_class = 'bir') AND failure_kind IS DISTINCT FROM 'no_service'`
(`IS DISTINCT FROM`, because `failure_kind` is NULL for a try that got
in and `<>` would read that as not counted).
A DNS failure is `bir`: the user reached Buwiz, so their own resolver
works; only the 3-networks rule separates "the BIR's name broke" from "one
ISP's DNS broke". Front-desk evidence comes only from TLS (port 443)
streams: a plain-HTTP answer can come from a middlebox.

Tries in `lab`, `dry_run` and mode `unknown` are recorded with their mode
and stages, no class or kind (but for an unreachable Sender, above), and
never counted. A return for a test TIN is never sent (the gate refuses it
before the relay claims it), so never recorded.

**What the record cannot tell apart.** Some blocks on the user's own side
look like the BIR's and are recorded as counted BIR-side failures: an
office network that blocks port 22 (the upload is refused or times out),
a TLS-inspecting proxy (the front desk's TLS fails), a filtering resolver
(the BIR's name does not resolve). One such network moves nothing on its
own; only the rule that needs 3 accounts on 3 networks (§15.1)
separates them from the BIR being down.

### 14.3 The table

`bir_tries` (migration 14), one row per try, kept 90 days (the hourly
upkeep deletes older rows):

| Column | |
|---|---|
| `started_at`, `ended_at` | To the minute. |
| `source` | `relay`; `direct` for the direct app's reports (§7). |
| `mode` | `live`, `lab`, `dry_run` or `unknown`. |
| `outcome` | `sent`, `dry_run`, `failed` or `unknown`. |
| `stage` | `none`, `dispatcher`, `upload` or `uploaded`. |
| `dispatcher`, `upload` | The windows: `ok`, `fail`, `none`; the upload also `maybe`. |
| `failure_class`, `failure_kind` | §14.2; both null for a try that got in, and for `lab` and `dry_run`. |
| `touched_bir` | A stream was opened (never in `lab`). |
| `opens` | How many streams the Sender asked for. |
| `admission` | The ticket's admission (§1); `open` for v1. |
| `counted` | §14.2. |
| `key_epoch`, `account_key`, `network_key` | The day key's period (a random id, new with each key) and the 8-byte keys of section 11, taken when the try was recorded. |
| `late`, `report_id` | The direct app's (§7, planned). |
| `client` | The app's name and `major.minor` from `buwiz-client` (`desktop/0.2`; §11), if it reads as `<app>/<major>.<minor>`. |

`bir_host_keys` keeps each upload-server host key a `live` try was offered:
its `SHA256:` fingerprint, when it was first and last seen with something
new, and the `{epoch, account_key, network_key, kex_done}` of the tries
that saw it (the newest 50), where `kex_done` says whether the Sender went
on to complete the key exchange (a try that reached the login or the
upload did; "host key is not pinned", "wrong server signature" and earlier
failures did not). A sighting is written only when it adds something (a
new key, or a new account, network or `kex_done` in this key period), so a
pinned key's row is not rewritten by every upload. Distinct accounts and
networks are counted only within one key period. A sighting is read from
bytes the user's app relays, so it can be forged: it is information only
and never grounds to pin. Pinning a new key needs an Admin and a second
source outside the relay
([relay-protocol.md](relay-protocol.md#7-server-settings)).

The fingerprint is SHA-256 over the key bytes exactly as the server sent
them in its handshake. The Sender fingerprints the key it parsed; for a key
in a non-canonical encoding (say, an RSA key with a leading zero byte the
Sender's library drops) the two could differ, so a recorded fingerprint
that matches no pin is not by itself proof the key changed.

## 15. What the server keeps

Built in this release, from `record` up; apps and people read it from
`public` (§4). `src/uptime/sign.rs` is the rule engine: a pure function of the
previous sign, the recent tries, the open note, the key periods in memory
and the clock, so it is tested rule by rule and replayed.
`src/uptime/live.rs` runs it after each recorded try (in the recorder's
own task, after the app has its result: a send is never delayed or broken
by it) and every 30 seconds for what the clock alone changes, writes every
change, and holds the sign in memory for every read. A single Gatekeeper
process is assumed: two would each keep, write and mail their own sign.

### 15.1 The rules

Evidence is a `live` try that is counted (§14.2). A try is evidence for a
window only if it reached it: the front desk reads `dispatcher` `ok` or
`fail` (TLS only), the mailbox `upload` `ok`, `fail` or `maybe`; and a
relay return that got in (`sent`: its upload server came from the front
desk and its key is pinned) is `ok` for the front desk whatever scheme
reached it. A try of class `user_network` or `ours` is never evidence; the
engine keeps its kind and port in memory so a later release can tell a
device its own network blocks the upload port (buwiz-com#58), and counts
nothing from it.

Times are exact in memory (a try's end to the nanosecond) and evidence is
ordered by when each try ended, not by when its record came; what is
stored or shown is to the minute. Every run reads all of the last 3
hours' evidence (at most 20,000 tries), so what it says does not depend on
which tries were new to it.

| Rule | As built |
|---|---|
| One account, one failure | For every threshold an account counts at most one failure, and one account's failure cannot by itself say "the newest did not get in". |
| The last 10 | A window's newest evidence within 60 minutes, newest first by end time, from when it last left Offline, linked to a key period in memory (below), at most 2 per account and at most 1 failure per account. |
| Online | Among the last 10 a return got in, the failures newer than the newest one that got in are one account's at most, and at most 1 failed (`fail` or `maybe`). |
| Busy | From Online: the Online rule no longer holds, a return got past the window within 60 minutes, and the failures among the last 10 come from at least 2 accounts on 2 networks. From No recent news: among the last 10 (all within 60 minutes) at least 1 got in and at least 3 failed, from at least 2 accounts on 2 networks. Busy is shown only while some return got in within the hour. |
| Offline | `fail` (not `maybe`) from at least 3 accounts on 3 networks within the last 15 minutes (a failure exactly 15 minutes old counts, 15 minutes and 1 second does not), one failure per account, all ending after the newest relay try that got past this window, from accounts that kept their vote, linked tries only. The accounts are kept with the light as its quorum. |
| "Accounts on networks" | The most tries with pairwise different accounts and pairwise different networks (one person on three networks and two people on one network are 2, not 3), counted within one key period, searched only until the threshold is reached. At most one of them may be a direct-app report (of the first 20 different ones), and a late direct report (no network) adds none. |
| Key periods | Two tries are the same account when they share an account key of one key period. A try recorded in the 3 hours after a new day key also carries the previous key, which is how tries across midnight compare. A try with no key of a period still in memory (one read back after a restart, or from before midnight once the previous key is gone) is neither the same account as another nor a different one: it is left out of the last 10 and every quorum. It still counts as news, and a landing among them still ends an Offline. |
| Lost votes | Only when a relay landing ends an Offline: every account of that Offline's quorum whose failure ended before the landing loses its Offline vote on that window for 24 hours from the landing, however old its failure. Never in Online or Busy, and never for a failure that ended after the landing. One entry per account and window, holding the latest end; at most 2,000, the latest ending kept. Held by account id in memory, and by its keys in `bir_sign`. **Lost votes do not outlast a restart:** the account ids live in memory only and the keys of a key period die with the process, so after a restart no new try can be matched to a lost vote, and an account whose vote was lost votes again. |
| Entering | Offline, at once, from any state. Busy at once from Online or No recent news. From No recent news, Online when the Online rule holds. |
| Leaving Online | First, to No recent news when nothing got past the window in the last 60 minutes (`rule`), or nothing reached it at all (`quiet`), whatever failures arrive (the Offline rule aside): Online is never shown unless a real return got past within the hour, and this is checked before Busy, so Online never goes Busy on failures alone. Then to Busy at once, as above. Two accounts failing on one network keep Online until the hour is out; one account's failures never leave it. |
| Leaving Busy | Once Busy is 10 minutes old (the dwell): to Online when the Online rule holds; to No recent news after 60 minutes without evidence (`quiet`), or when nothing got past the window in the last 60 minutes (`rule`), however many failures keep coming (one account failing every 30 minutes no longer holds Busy: it leaves within the hour). Otherwise Busy stays. |
| No recent news, in words | "Nobody filed in the last hour" only when no counted try reached a window; when tries reached one and none got past it: "No return has got in during the last hour: some tries failed, but too few people to call it Offline". `last_state` is what the lights said before the quiet (never a note's state), so a note that ends over quiet lights says nothing went offline. |
| Leaving Offline | To Online at once when a relay try (a scout, an admitted ticket, a Try anyway) that got past the window ended after the quorum's first failure, unless the failures after it are still a quorum: that is checked in the same run, so several tries taken in together give no Offline, Online, Offline pair. The event says which kind got in. To No recent news after 3 hours without evidence on that window (each window its own), with `last_state: offline`. A direct-app report never ends Offline and never counts as "got in since". |
| Overall | Offline if either light is, or an outage or maintenance note is in force; else Busy if either is; else Online if the mailbox is; else No recent news. A "declare Online" note makes it Online. Leaving Offline (and the end of an outage or maintenance note) starts waking; nothing ends waking yet. |
| Notes | An `outage` note ends when a counted relay return got in with its exact end after the note's start. A `maintenance` note ends at its end or by an Admin; a return that got in during it is mailed to staff, once per note. |
| Failures in a row | 5 counted failures from 5 accounts (linked tries) with no return got in since: staff are mailed, once per run of failures, whatever the light says. |
| Relay pause | Not in this release (nothing could resume it): when 3 relay tries of the kinds `host_key`, `unknown_host` or `protocol` (the front desk's answer not understood, or no common SSH algorithm) come from 2 accounts on 2 networks within 60 minutes, staff are mailed "the relay would pause: …", and again at most once a day while it holds. Nothing is held and `relay.paused` is never written true; a stored sign that says paused is read as not paused. The resume function stays for the later staff page. |

The thresholds are constants in `sign.rs`.

### 15.2 Events and the stored sign

Every change is a `bir_events` line, its time to the minute: the scope
(`dispatcher`, `upload`, `overall`, `relay`, `watch`), the old and new
state, the cause (`rule`, `scout`, `try_anyway`, `quiet`, `note_set`,
`note_cleared`, `note_expired`, `note_return_got_in`, `relay_paused`,
`relay_resumed`, `not_watched`, `reset`), a sentence with counts ("3
accounts on 3 networks didn't get past the mailbox within 15 min", "7 of
the last 10 got in"; an Admin's note text or clearing reason where they
gave one), and for an Admin's action their id and email. Each line carries
the sign's `seq` and its place in that run (`sign_seq`, `ord`, unique): a
run written again because its commit's acknowledgement was lost adds
nothing twice. The first start writes one line `none` to `no_recent_news`
("Buwiz began keeping the sign"), where the day history begins.

`bir_sign` holds the sign: a `version`, `seq`, `revision`, the overall
state with `since` and `last_state`, each light's state, `since`,
`last_ok_at`, last evidence, where its last 10 begin and (while Offline) its
quorum by key, the note in force, the relay, `last_success_at`,
`last_news_at`, `waking_since`, the lost votes by key, and which run of
failures and which maintenance note staff were told of; every time to the
minute. Every field has a default, so a sign another release wrote, with a
field fewer or more, reads whole (a missing time is its last write's, a
time after now is now, logged). It is written in the same transaction as
the events, at once for any change but the times of news and of the
evaluation, which alone are written at most once a minute (and at least
once a minute while the ticker runs).

**`seq`.** Every change moves it to `max(seq + 1, the change's 5-minute
mark in Unix seconds)` (a gap in watching is a change, and so is every
start, below), so it only grows, tells nothing finer than 5 minutes about
when it moved, and passes any `seq` handed out before even after the
database is restored from an older backup: at the next start the mark is
later than any value given out then. The events' unique key (`sign_seq`,
`ord`) holds because `seq` never repeats; every write takes the sign's
lock and compares the stored `seq` with its own (below). `revision`
(which follows what a reader sees) is internal: it is part of that check
and never exposed; the public `ETag` is a hash of the document's bytes.

A stored sign that cannot be read at all starts again at No recent news (or
what the note it held says), keeps its note, and writes a `reset` line,
with `seq` moved on as at any start.

No try is lost on the way to the rules: the recorder puts each try in the
rules' inbox and then runs them only if their lock is free (it never waits
for them); a try that finds them busy is taken in by the run that holds
them or the next tick, at most 30 seconds later, and stays in memory
whatever that run does. A timeout bounds the database work (10 s for a run
or the load, 5 s for a try's row); a try whose row could not be written
still reaches the rules. A run that cannot be written (the database is
down) is still shown: the sign keeps aging in memory (Online cannot stay
up for hours on no evidence), and the next run that can write makes and
writes the same change. If the stored sign cannot be read, the run goes on
from memory.

### 15.3 Starting, restarting and stopping

The sign never claims what it has not seen. A start or restart is Buwiz's
event, not the BIR's.

- **The first start ever** (no stored sign): every light is No recent
  news, which claims neither up nor down; apps send normally in it, so
  nothing is held and nothing is assumed. The first real return that gets
  in turns the light Online; the Offline rule turns it Offline. History
  begins at that moment; nothing before it is shown as up or down.
- **A start** (a deploy or a crash): before the server binds its port, and
  for at most 10 seconds, the stored sign is restored as it was, with its
  original "since" times, the `live` tries of the last 3 hours and the open
  note are read, and it is written back under the sign's lock with `seq`
  moved on once; then it is aged by the clock with the ordinary quiet
  rules: Online or Busy with no return in for 60 minutes becomes No recent
  news; Offline stays Offline until a return lands, or becomes No recent
  news after 3 quiet hours; an expired note is ended. A restart never turns
  Offline into Online or Online into Offline. Tries from before it carry
  only the key they were recorded under, of a key period no longer in
  memory, so a quorum needs fresh tries, and lost votes are gone (§15.1).
  If the load fails or takes longer, the server starts at No recent news
  with `seq` 0 (which claims nothing) and the ticker tries again every 30
  seconds. The day history is written after the port is bound, by the
  ticker, in the background, once its first run of the rules worked.
  Until the sign is loaded, everything public shows that placeholder; the
  first sign shown after a start is public at once, not from the next mark.
- **Time Buwiz was not watching** (the server was stopped, or the sign was
  not evaluated for more than 5 minutes) is written as a pair of `watch`
  lines ("Buwiz stopped watching" at the last evaluation, "Buwiz was not
  watching for N min" when it starts again) and is its own value in the
  day history. It is never given to a BIR state, never extends the state
  before it, counts in no duration, and splits any outage it falls in. Its
  start is the last time the sign was evaluated: exact within a running
  process; across a restart the stored one, to the minute and at most a
  minute old, so a restart of under 5 minutes writes no gap (and one of 5
  to 7 minutes may not either). One gap is at most 31 days: a clock that
  jumps forward writes no absurd span (logged).
- **A clock that steps back** more than 2 minutes behind the last run does
  not hold the rules until it catches up: the sign's and the tries' times
  are brought back to it (logged) and the rules go on from it.
- **`BIR_UPTIME=off` and on again** is the same as a restart after a gap.
- **Online is never shown without a real return having got in; Offline
  never without the accounts-and-networks rule or an Admin's note.** There
  is no "assume running" and no "assume down".
- **Stopping** (SIGTERM, what a deploy sends): the server is first marked
  stopping, and from then on a request that would start a send (a ticket,
  a submission, the relay upgrade) is answered `503 restarting` with
  `Retry-After: 30` before anything starts, so no send begins on a process
  about to exit and none that had not started becomes `unknown`. Requests
  already under way are still served, and the records of tries that end
  meanwhile are still written. After at most 10 seconds of waiting for
  those records the process exits at once, so blocking work cannot hold
  the restart; a send still open then is cut, as before.

### 15.4 Notes and resuming the relay

`src/uptime/notes.rs`: functions the staff screens will call. Admins only
(`Power::SetBirStatus`): checked before the engine's lock is taken, so
nobody else waits on it, and again inside the transaction, where the
Admin's email is read again for the record; Support and everyone else are
refused, and an Admin demoted while their request waited does nothing.
Each writes its audit line (a `bir_events` line with the Admin's id and
email) with the change. Every path that changes the sign or a note takes
the sign's lock (a Postgres advisory lock) before it touches a note row,
so the server and a note change can never wait on each other.

- **Set** an `outage`, `maintenance` or `online` ("ignore evidence until …,
  declare Online", its text the reason) note: text 1 to 200 characters
  (public), an end in the future and at most 72 hours ahead, refused while
  another is in force. A note past its end has ended and never blocks a new
  one; it is marked ended with a `note_expired` line.
- **Clear** the note in force, with a reason (1 to 200 characters, staff
  only, `bir_notes.cleared_reason`).
- A note also ends when its end passes (`note_expired`) and, for an outage,
  when a counted relay return gets in after it began
  (`note_return_got_in`).
- **Resume sending** after the relay paused: for the later staff page;
  nothing in this release pauses it, so it answers "not paused".

Until the staff page exists an Admin uses the server command `buwiz-cloud
bir declare online|offline|maintenance`, `bir clear` and `bir status`
(`src/uptime/cli.rs`; the runbook is in [deploy.md](deploy.md)). It makes
the same checks and writes the same note row and audit line (with the
Admin's reason) as the functions above (`notes::record_set`,
`record_clear`), but runs no rule: under the sign's lock it writes only
the note, its audit line (`overall`, the stored state to itself) and the
stored `seq` moved on, and never the stored sign, so it can neither end an
Offline nor write over what the server holds and has not written yet (its
times, the told-once flags, the lost votes). `bir status` only reads.
`--as` names the Admin for the audit line; it is not authentication:
whoever can run the command on the server, with its settings, is trusted.
It refuses before any server has kept a sign.

The running server takes the change in without a restart: every run of the
rules first reads the stored `seq`, and a higher one (written by the
command) makes it read the open note and take that `seq`, keeping its own
sign whole; the run then makes the note's change (`note_set`,
`note_cleared`), writes it and mails staff, with the Admin's own words
from the audit line. With no server running, the note takes effect and is
mailed when one starts. A note set and cleared again before the server's
next run changes nothing shown; the audit lines keep both. A server's
write that finds the stored `seq` moved writes nothing (no line, no `seq`)
and its next run takes the change in.

### 15.5 The day history

One `bir_days` line per Manila day (`src/uptime/days.rs`), kept forever:
the worst state (Offline over Busy over Online over No recent news), the
minutes in each state overall and per window (`windows`), the minutes a
note was in force, the minutes Buwiz was not watching
(`unwatched_minutes`, in no other count), the hours (`w` for an hour not
watched at all, `-` before the history began and for hours still to come),
the outages to the minute (each window's Offline stretches, and hand-set
outage or maintenance, split where Buwiz was not watching), counted tries
that got in, failed and broke midway, `ours` and `user_network` tries, held
requests, and the distinct accounts and networks of the day's counted
tries. A day with no tries is No recent news; a day Buwiz did not watch at
all is `w` every hour, worst `none`. The lines not final yet are rewritten
every 10 minutes and at start: every day from the one after the last final
line, so a day the server was down for still gets its line; each is
marked final 10 minutes after the day ends, once the sign was evaluated
after the day ended (or a gap in watching covers its end), and never
computed again: a day whose last stretch nobody judged (the server
stopped before midnight and has not run since) stays open until it is.
Running it twice gives the same line.

Distinct accounts and networks come from the raw sets held in memory
(yesterday's kept until its last roll-up). A restart empties them, so the
line keeps the largest of what was counted before the restart, what is
counted after it, and the distinct keys within one key period: never more
than the truth, possibly less.

### 15.6 Retention

`bir_tries` 90 days; `bir_held` 90 days; queue reports when they expire;
turns a day after they were given. `bir_days`, `bir_events`, `bir_notes`,
`bir_host_keys` and `bir_proofs` are kept.

### 15.7 Staff mail

`src/uptime/alerts.rs`, through the existing mailer (`Mailer::send_notice`,
plain text; logged with `MAIL_TRANSPORT=log`), to every Admin and Support
not banned who has signed in at least once (a role given to an address
nobody has proved is theirs mails nobody) and every address in
`ADMIN_EMAILS`. This release mails them. It goes through the same mail
provider as sign-in codes and shares its quota.
What is mailed:

- the overall sign going Offline, and coming back from Offline to anything
  else (with the windows' reasons); a window going Offline while a "declare
  Online" note is in force (the overall sign does not show it); every note
  set, cleared or ended; and a reset of the stored sign. Other changes of
  the sign (Online, Busy and No recent news among themselves) and of a
  window alone are in the event log only, not mailed (the owner's rule of
  2026-11-14);
- 5 failures in a row, and a return that got in during maintenance;
- "the relay would pause" (at most once a day while it holds);
- a missing `CF-Connecting-IP` in production (at most once an hour);
- a fault on Buwiz's side that needs a person: an upload-server host key
  that is not pinned (its fingerprint marked "relay-reported, unverified:
  do not pin from this mail", once a day per key), an unknown upload
  server, an answer Buwiz could not read, or a Sender that cannot be
  reached (each at most once an hour).

The first alert mails at once; alerts in the next 10 minutes go together
at the end of them. Each kind has its own budget a Manila day, so one
cannot use up another's: the sign, windows and notes 48 alerts, host keys
3 across every fingerprint, everything else 8 (five failures in a row and
a return got in during maintenance among them); the mail that reaches a
budget says so. An alert past its budget is not mailed and not counted as
told, so the same alert the next day is mailed (the event log keeps every
change). An alert counts as told, spends its budget and marks its events'
`bir_events.mailed_at` once its mail reached one address. Each address
gets each mail once: the addresses a mail reached are kept with it, so a
slow or failing provider never makes the first ones get it again; the
rest are tried again after 1, 2, 4 … minutes, at most an hour apart, and
the mail is given up (logged) a day after its first try. Each request to
the mail provider has a 10 s connect and 30 s overall timeout, and a round
of mail at most 60 s; a round cut short counts as failed for the addresses
it did not reach. The queue is in memory: alerts not yet mailed when the
server stops are lost, but a note set while the server was stopped is
mailed after the next start (its change is made then). Under a "declare
Online" note a window going Offline and coming back from it are mailed
(the overall sign shows neither). With `BIR_UPTIME=off` nothing is mailed.

What a recorded try sends: nothing to the app beyond what §14 says, and,
when it changes the sign or is one of the alerts above, at most one line
in the next staff mail.

The ticker runs each part (the rules, the mail, the day history) on its
own: a panic in one is logged and the next part, and the next tick, go
on. The day history runs in a task of its own (one at a time), only after
a run of the rules that worked, so a long roll-up never holds the rules.
