//! The public sites and which host serves what: the Bot site on the bare
//! domain ([`bot_site`]), the filing site's installers ([`downloads`]), the
//! host rules ([`canonical`]) and what search engines read ([`seo`]). The
//! public sites' CSS and JavaScript live here too. See `docs/bot-site.md`
//! and `docs/ebirforms-site.md`.

pub mod bot_site;
pub mod canonical;
pub mod downloads;
pub mod github;
pub mod seo;
