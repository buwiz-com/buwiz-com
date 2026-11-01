# buwiz.com: the Buwiz Bot site

The bare domain serves the Buwiz Bot site. It has two pages:

- `/`: Meet Buwiz Bot.
- `/mini-apps`: eBIRForms, Books, Word and Spreadsheet.

Every other path on this host either:

- answers with a 301 to the filing site, for `/download`, `/install-linux.sh` and the demo assets;
- answers with a 301 to the account host, for `/login`, `/signup` and `/tins`;
- or falls through to the shared routes: `robots.txt`, `sitemap.xml`, `/healthz`, `/.well-known/*`.

See `moved_to` in `src/canonical.rs`.

## How it is built

`bot-site/` is a Leptos app (client-side rendering), a crate of its own.
Trunk builds it into `bot-site/dist/`. The server embeds `dist/` with
`rust-embed` (`src/bot_site.rs`). It serves:

- the pages at their paths;
- the bundle and images under `/bot/`, clear of the server's `/assets/`.

The Docker build has no Trunk, so **`dist/` is committed**. After you change
anything in `bot-site/`, rebuild it and commit the result:

```sh
bot-site/build.sh     # needs trunk, the wasm32-unknown-unknown target, and Google Chrome
```

`build.sh` runs `trunk build --release`, then `bot-site/prerender.py`.

## Prerendering

Crawlers that do not run JavaScript, which includes most AI answer engines,
would see an empty `<body>`. So each page is served already drawn:

1. `prerender.py` serves `dist/` the way the server does. It loads each page in
   headless Chrome and writes the rendered body to `dist/prerender/<page>.html`.
   Set `CHROME=/path` if Chrome is somewhere else.
2. `bot_site::index` puts that body into the page inside `<div id="prerender">`.
   It also sets the page's title, description and canonical address in the
   head.
3. `bot-site/src/main.rs` removes `#prerender` before the app mounts. Nothing
   shows twice.

**To add a page:**
- Add it to `PAGES` in `src/bot_site.rs`, with its title, description and prerender file.
- Add it to `PAGES` in `prerender.py`.
- Add it to `BOT_PAGES` in `src/seo.rs` (the sitemap).
- Add the route in `bot-site/src/app.rs`.

## SEO

- `bot-site/index.html` carries the home page's title, description, Open
  Graph and Twitter tags. It also carries JSON-LD for the Organization, the
  WebSite and the Buwiz Bot SoftwareApplication. Other pages get their own
  title, description and canonical address from `bot_site::PAGES`.
- The JSON-LD URLs leave off the home page's trailing slash.
  `bot_site::index` rewrites the exact `https://buwiz.com/"` string to each
  page's canonical address.
- There is no share image yet. `src/assets/brand/og-image.png` is the filing
  site's card.
- `robots.txt` and the sitemap come from `src/seo.rs`. The sitemap lists only
  `BOT_PAGES`. It leaves out `/download`, which is a 301 to the filing site.
- Tests: `bot_site::tests`, plus `once_split_*` in `tests/account_site.rs`.
