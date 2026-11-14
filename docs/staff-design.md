# The staff console's design

The staff pages under `/staff` have their own look, separate from the
owner-facing pages: their own document head (`src/account/pages/staff/shell.rs`),
stylesheet (`src/account/staff.css`), script (`src/account/staff.js`) and
icons (`src/account/staff-icons.svg`). Owner pages (sign-in, My TINs, home,
downloads) never load them, and the console loads none of theirs.

## Tokens

All in `:root` at the top of `staff.css`, as CSS custom properties. Nothing
below them names a colour, size or radius of its own.

- **Accent**, defined once: `--accent` (Buwiz green `#0f5b46`) and
  `--accent-dark` (`#3fb58f`), each with the text that sits on it
  (`--accent-ink`, `--accent-dark-ink`). The hover shade, the soft tint and
  the status tints are derived from it with `color-mix()`. Change those four
  to recolour the console.
- **Colour**, light and soft dark, following `prefers-color-scheme`:
  `--color-bg`, `--color-surface`, `--color-surface2`, `--color-nav`,
  `--color-line`, `--color-line2`, `--color-text`, `--color-muted`,
  `--color-faint`, the status colours `--color-ok`, `--warn`, `--bad`,
  `--info`, `--test` (with derived `-bg` tints), `--color-hot` for the
  waiting count, the document viewer's `--color-viewer*`, `--color-scrim`,
  `--color-brand`. Text keeps at least 4.5:1 in both themes (`--color-faint`
  is darker than the picker's value for that reason).
- **Type**: Inter (variable) for text, IBM Plex Mono for TINs, self-hosted
  under `/assets/fonts`. Sizes `--text-xs` 11.5, `-sm` 12.5, `-md` 14,
  `-lg` 15.5, `-xl` 20, `-2xl` 26 (px).
- **Spacing**: `--space-1` to `--space-8`: 4, 6, 8, 12, 16, 20, 24, 32 px.
- **Corners**: `--radius-sm` 4, `-md` 6, `-lg` 10, `-pill` 999.
- **Sizes**: `--row-height` 38, `--control-height` 32, `--touch` 44 (phone
  targets), `--sidebar-width` 232, `--rail-width` 60, `--inspector-width` 360.
- **Shadows**: `--shadow-1` (unused by flat panels), `--shadow-2` for what
  floats: dialogs, sheets, the inspector over a tablet's list.

## Regions and breakpoints

| Width | Navigation | Inspector |
|---|---|---|
| Desktop, 1200px and up | Left sidebar: search, New user, grouped links with live counts, My account | Always there on the right |
| Tablet, 768 to 1199px | Icon rail; "Show names" opens the sidebar over the page | Slides over the list when a row is opened |
| Phone, below 768px | Top bar and a tab bar at the bottom (Overview, Queue, Users, Me) | A sheet that slides up, with the main actions in a bar at the thumb |

A page renders `<main id="main" class="sx-main">` and, where it lists
things, the inspector beside it (`inspector()` in `pages/staff.rs`).

## Addresses

The console opens on the Overview ("Overview first"), so the review queue has
an address of its own.

| URL | What it is |
|---|---|
| `/staff` | The Overview. Staff only (Support and Admins). |
| `/staff?…` (any query at all) | Old queue links: staff are redirected (307) to `/staff/queue?…` with the whole query kept, so `?notice=`, `?status=`, `?open=` and `?q=` links, bookmarks and emails still work. Checked after the staff check, so non-staff get what they get for `/staff`. |
| `/staff/queue` | The review queue, with `?status=`, `?q=`, `?open=` and `?notice=`. Every decision, delete and revoke redirects here with its notice. |
| `/staff/overview` | Redirects (307) to `/staff`. |
| `/staff/claims`, `/admin`, `/admin/claims` | Redirect (308) to `/staff/queue`. |
| `/staff/claims/{id}` | A claim's own page (its "back" link is the queue). |

Non-staff see a 404 on all of these (signed-out visitors are sent to
`/login?next=…`, and after signing in with `next=/staff` staff land on the
Overview). In the navigation, `aria-current` marks Overview on `/staff` and
Queue (the sidebar's Waiting, Sent back and Activated automatically, the
rail's icons, the phone's Queue tab) on `/staff/queue` and on a claim's page.

## State in the URL

Everything works without script. The row in the inspector is `?open=<id>` (on `/staff/queue`)
(desktop shows the list's first row when none is chosen); the user page's
tab is `?tab=account|tins|sign|history`; the New user dialog is
`/staff/users?new=1`. `staff.js` then swaps the inspector in place (and
keeps the URL), opens and closes the phone sheet and the tablet's names,
moves actions that need a reason into a modal dialog, and switches tabs.
Pages send a Content-Security-Policy that allows only this site's own
files: no inline script, no inline style.

## Components

`staff.css` sections, with their Rust helpers in `pages/staff/ui.rs`:
buttons (`.sx-btn`, `pri`, `dan`, `sm`, `block`) and icon buttons
(`.sx-ib`); icons (`ic()`, symbols in `staff-icons.svg`); pills
(`sx_pill`, `status_pill`, `flag_pill`) and counts (`.sx-count`, `hot`);
tabs (`.sx-tabs`) and filter chips (`.sx-chips`); the page head
(`page_head`); notices (`sx_notice`, `.sx-notice error|info|warn`); compact
tables (`.sx-table`, which become cards on a phone through the cell areas
`a-main`, `a-side`, `a-sub`, `a-foot`); cards and sections (`.sx-card`,
`.sx-sec`, `.sx-sub`); Overview tiles (`.sx-tile`); the activity feed and
history (`.sx-feed`, `.sx-hist`); facts rows (`.sx-fact`); the document
viewer (`.sx-viewer`, radio tabs); the claim comparison (`.sx-cmp`); actions
with a reason (`act`, `reason_act`, `reason_field`: a `<details>` without
script, a `<dialog>` with it); the New user modal (`.sx-modal`).

## The Overview's tiles

Counts only, the same for Admin and Support (no TIN, name or account). "Today"
is since midnight in Manila (UTC+8); "this week" is the last 7 days, today
included, or for returns from midnight six days ago. Queries are in
`src/account/staff.rs`; no index or migration is involved.

- **Active TINs**: approved, not paused, not a test TIN (`000-000-000`). The
  small line under it names `N paused` (approved real TINs a staff member
  paused) and `N test` (approved test TINs), leaving out a zero; then
  `+N this week` (activated in the last 7 days) always.
- **Returns sent today**: `submissions` with `status = 'sent'` and `sent_at`
  today, with the hourly spark. Under it, in the danger colour `N failed`
  (`status = 'failed'`) and in the warning colour `N waiting` (`ready`, that
  is queued, `sending`, or `unknown`), a zero left out and the line left out
  when both are zero. Failed and waiting rows have no outcome time, so they
  count when their last attempt (`sending_since`) or, with none, their
  `created_at` is today. `dry_run` and `expired` are in none of the parts.
  Last line: `N in the last 7 days`, `sent` by `sent_at`.

Each sub line keeps a count and its word together (no-break space) so a
narrow tile wraps between parts, not inside one.

## Adding a screen

1. Add a `#[page("/staff/…")]` in `src/account/pages/staff.rs` (or a module
   beside it), starting with `staff_member(cx, Power::…)` so non-staff get
   a 404, and register it in `pages.rs`. Do not add a query-carrying use of
   `/staff` itself: `/staff?…` is the queue's old address and redirects.
2. Render `<main id="main" class="sx-main" tabindex="-1">` with
   `page_head(…)`, a `<div class="sx-notices">` for `sx_notice`, and the
   content in `<div class="sx-scroll">`. Add an `inspector(…)` beside it
   if the screen selects rows, with the selection as `?open=`.
3. Use the components above; new styles go in `staff.css` as `sx-` classes
   on the tokens, with tablet and phone rules in the two media blocks.
4. Give it a title in `head_for` (`pages.rs`), a sidebar link in
   `shell.rs` if it is a destination, and add its route to the roles matrix
   in `tests/staff_powers.rs`.
