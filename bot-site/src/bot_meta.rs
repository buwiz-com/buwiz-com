//! What the server tells the page about Buwiz Bot (src/bot_site.rs): its
//! released installers and its GitHub stars. Read once at start from the
//! `buwiz-bot-download` and `buwiz-bot-stars` metas in index.html; both are
//! empty while unknown.

use crate::app::meta_content;
use js_sys::Reflect;
use wasm_bindgen::JsValue;

/// Buwiz Bot's source code.
pub const REPO_URL: &str = "https://github.com/buwiz-bot/buwiz-bot";

/// Star counts below this are not shown.
const STARS_SHOWN_FROM: u64 = 25;

/// A desktop the app has installers for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Desktop {
    Mac,
    Windows,
    Linux,
}

impl Desktop {
    pub fn name(self) -> &'static str {
        match self {
            Desktop::Mac => "macOS",
            Desktop::Windows => "Windows",
            Desktop::Linux => "Linux",
        }
    }

    /// Its key in the download meta.
    fn key(self) -> &'static str {
        match self {
            Desktop::Mac => "mac",
            Desktop::Windows => "windows",
            Desktop::Linux => "linux",
        }
    }
}

/// One installer: where it is and how big.
#[derive(Clone, Debug)]
pub struct Installer {
    pub url: String,
    pub size: u64,
}

/// The released version and its default installer per desktop.
#[derive(Clone, Debug)]
pub struct BotRelease {
    pub version: String,
    mac: Option<Installer>,
    windows: Option<Installer>,
    linux: Option<Installer>,
}

impl BotRelease {
    pub fn installer(&self, desktop: Desktop) -> Option<&Installer> {
        match desktop {
            Desktop::Mac => self.mac.as_ref(),
            Desktop::Windows => self.windows.as_ref(),
            Desktop::Linux => self.linux.as_ref(),
        }
    }
}

/// Both metas, provided as context by the app.
#[derive(Clone, Debug, Default)]
pub struct BotMeta {
    pub release: Option<BotRelease>,
    pub stars: Option<u64>,
}

impl BotMeta {
    pub fn read() -> Self {
        Self {
            release: parse_release(&meta_content("buwiz-bot-download")),
            stars: meta_content("buwiz-bot-stars").trim().parse().ok(),
        }
    }

    /// `★ 1,234`, once there are enough stars to show.
    pub fn star_label(&self) -> Option<String> {
        self.stars.filter(|count| *count >= STARS_SHOWN_FROM).map(|count| format!("★ {}", grouped(count)))
    }
}

fn parse_release(raw: &str) -> Option<BotRelease> {
    if raw.trim().is_empty() {
        return None;
    }
    let value = js_sys::JSON::parse(raw).ok().filter(JsValue::is_object)?;
    let version = Reflect::get(&value, &"version".into()).ok()?.as_string()?;
    let installer = |desktop: Desktop| {
        let entry = Reflect::get(&value, &desktop.key().into()).ok().filter(JsValue::is_object)?;
        let url = Reflect::get(&entry, &"url".into())
            .ok()?
            .as_string()
            .filter(|url| url.starts_with("https://") || url.starts_with("http://"))?;
        let size = Reflect::get(&entry, &"size".into()).ok()?.as_f64()?;
        Some(Installer { url, size: size.max(0.0) as u64 })
    };
    Some(BotRelease {
        mac: installer(Desktop::Mac),
        windows: installer(Desktop::Windows),
        linux: installer(Desktop::Linux),
        version,
    })
}

/// The visitor's desktop, from `navigator.userAgent`. Phones, tablets,
/// Chromebooks and anything else give `None`.
pub fn guess_desktop() -> Option<Desktop> {
    let navigator = web_sys::window()?.navigator();
    let agent = navigator.user_agent().ok()?;
    if ["Android", "iPhone", "iPad", "iPod", "CrOS", "Mobile"].iter().any(|name| agent.contains(name)) {
        return None;
    }
    if agent.contains("Windows") {
        Some(Desktop::Windows)
    } else if agent.contains("Macintosh") || agent.contains("Mac OS X") {
        // An iPad asking for the desktop site says Macintosh too.
        (navigator.max_touch_points() <= 1).then_some(Desktop::Mac)
    } else if agent.contains("Linux") || agent.contains("X11") {
        Some(Desktop::Linux)
    } else {
        None
    }
}

/// `84.2 MB`, in decimal units, as the filing site's download page writes it.
pub fn size(bytes: u64) -> String {
    if bytes >= 1_000_000 {
        format!("{:.1} MB", bytes as f64 / 1_000_000.0)
    } else {
        format!("{} KB", bytes.div_ceil(1000))
    }
}

/// `1,234`.
fn grouped(count: u64) -> String {
    let digits = count.to_string();
    let mut out = String::new();
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}
