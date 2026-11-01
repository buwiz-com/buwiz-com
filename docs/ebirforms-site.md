# ebirforms.buwiz.com: the filing site

The public site of the eBIRForms desktop app
([eBIRForms/eBIRForms](https://github.com/eBIRForms/eBIRForms)). It is served
on the host in `FORMS_ORIGIN`.

| Path | What | Code |
|---|---|---|
| `/` | The home page: the app tour, the AI coworker demos and the FAQ | `src/pages/home.rs`, `src/home.css`, `src/demo.js`, `src/demo.css` |
| `/download` | One card per platform, with one Download button and the version, size and SHA-256 | `src/pages/download.rs` |
| `/download/{platform}` | A counted redirect to the installer in R2 | `src/downloads.rs` |
| `/install-linux.sh` | The Linux install script, once a version is released | `src/downloads.rs` |
| `/assets/screens/*`, `/assets/demo/*` | The tour screenshots and the demo assets | `src/assets/` |

The page renders server-side, with no client framework:
- the tour switches screens with radio buttons;
- the FAQ uses `<details>`;
- only the AI coworker demos need JavaScript.

The hero's Download button is picked from `User-Agent` on the server.

## Releases and downloads

The installers are in the R2 bucket served at `DOWNLOADS_URL`. Each app has
its own folder:
- the release CI uploads `{app}/releases/v{version}/{file}`, with that version's `manifest.json`;
- the owner releases a version by copying its manifest to `{app}/latest.json`.

While an app has no `{app}/latest.json`, the root `latest.json` is read
instead.

Until a version is released, every card says "Coming soon" and links
nowhere. Downloads are counted per app in `download_counts`. The server keeps
`latest.json` for a minute after each fetch. A failed refresh keeps the last
good one.

## SEO

`robots.txt` and the sitemap come from `src/seo.rs`. The sitemap lists `/`
and, once a version is released, `/download` with the release date. The
account and API hosts send `X-Robots-Tag: noindex`.
