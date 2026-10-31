//! Brand-colored icons for the tools a coworker's computer can carry. Their colors are the
//! tools' own, so they stay the same in light and dark.

use leptos::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Rustup,
    Rustc,
    Cargo,
    Analyzer,
    Terminal,
    Git,
    Chromium,
    Office,
    Pdf,
    Ssh,
    Python,
    FreeCad,
    LibreCad,
    Uv,
    Pip,
    Ftp,
    BuwizBooks,
    BuwizForms,
}

impl Tool {
    pub fn name(self) -> &'static str {
        match self {
            Tool::Rustup => "rustup",
            Tool::Rustc => "rustc",
            Tool::Cargo => "cargo",
            Tool::Analyzer => "rust-analyzer",
            Tool::Terminal => "Terminal",
            Tool::Git => "git",
            Tool::Chromium => "Chromium",
            Tool::Office => "LibreOffice",
            Tool::Pdf => "PDF reader",
            Tool::Ssh => "ssh",
            Tool::Python => "Python",
            Tool::FreeCad => "FreeCAD",
            Tool::LibreCad => "LibreCAD",
            Tool::Uv => "uv",
            Tool::Pip => "pip",
            Tool::Ftp => "lftp",
            Tool::BuwizBooks => "Buwiz Books",
            Tool::BuwizForms => "Buwiz Forms",
        }
    }

    /// A short line under the name: version or what it is.
    pub fn sub(self) -> &'static str {
        match self {
            Tool::Rustup => "1.28",
            Tool::Rustc => "1.91",
            Tool::Cargo => "1.91",
            Tool::Analyzer => "LSP",
            Tool::Terminal => "bash",
            Tool::Git => "2.47",
            Tool::Chromium => "browser",
            Tool::Office => "writer · calc",
            Tool::Pdf => "evince",
            Tool::Ssh => "openssh",
            Tool::Python => "3.13",
            Tool::FreeCad => "3D CAD",
            Tool::LibreCad => "2D CAD",
            Tool::Uv => "python pkgs",
            Tool::Pip => "python pkgs",
            Tool::Ftp => "FTP · SFTP",
            Tool::BuwizBooks => "ledger · invoices",
            Tool::BuwizForms => "eBIRForms filing",
        }
    }

    fn bg(self) -> &'static str {
        match self {
            Tool::Chromium => "#e8f0fe",
            Tool::Terminal => "#1f2430",
            Tool::Pdf => "#fde8e8",
            Tool::Office => "#e6f6ec",
            Tool::Rustup | Tool::Cargo | Tool::Rustc | Tool::Analyzer => "#f6e7dd",
            Tool::Python | Tool::Pip => "#e8f0fb",
            Tool::Uv => "#efe7fb",
            Tool::Ssh => "#eef1f6",
            Tool::Ftp => "#fff4e0",
            Tool::LibreCad | Tool::FreeCad => "#e6f3fb",
            Tool::Git => "#fde9e1",
            Tool::BuwizBooks => "#e7f8ee",
            Tool::BuwizForms => "#dcf4f0",
        }
    }

    /// The inside of a 24×24 SVG.
    fn svg(self) -> &'static str {
        match self {
            Tool::Chromium => r##"<circle cx="12" cy="12" r="10" fill="#db4437"/><path d="M12 2a10 10 0 0 1 8.66 5H12z" fill="#db4437"/><path d="M20.66 7A10 10 0 0 1 12 22l4.33-7.5z" fill="#f4b400"/><path d="M12 22A10 10 0 0 1 3.34 7l4.33 7.5z" fill="#0f9d58"/><circle cx="12" cy="12" r="4.4" fill="#fff"/><circle cx="12" cy="12" r="3.4" fill="#4285f4"/>"##,
            Tool::Terminal => r##"<rect x="2" y="4" width="20" height="16" rx="3" fill="#0d1117"/><path d="M6 9l3 3-3 3" stroke="#7ee787" stroke-width="2" fill="none" stroke-linecap="round"/><path d="M11 15h6" stroke="#c9d1d9" stroke-width="2" stroke-linecap="round"/>"##,
            Tool::Pdf => r##"<path d="M6 2h9l5 5v15H6z" fill="#e5484d"/><path d="M15 2v5h5" fill="#ffb3b3"/><text x="13" y="18" font-size="6" font-weight="700" text-anchor="middle" fill="#fff" font-family="sans-serif">PDF</text>"##,
            Tool::Office => r##"<path d="M5 2h10l4 4v16H5z" fill="#18a303"/><path d="M15 2v4h4" fill="#9be38e"/><rect x="8" y="10" width="8" height="1.6" fill="#fff"/><rect x="8" y="13" width="8" height="1.6" fill="#fff"/><rect x="8" y="16" width="5" height="1.6" fill="#fff"/>"##,
            Tool::Rustup => r##"<circle cx="12" cy="12" r="9" fill="none" stroke="#b7410e" stroke-width="2.4" stroke-dasharray="2.2 1.6"/><text x="12" y="16" font-size="10" font-weight="800" text-anchor="middle" fill="#b7410e" font-family="sans-serif">R</text>"##,
            Tool::Cargo => r##"<path d="M4 8l8-4 8 4v8l-8 4-8-4z" fill="#b7410e"/><path d="M4 8l8 4 8-4M12 12v8" stroke="#f6e7dd" stroke-width="1.4" fill="none"/>"##,
            Tool::Rustc => r##"<path d="M7 7l-4 5 4 5M17 7l4 5-4 5" stroke="#b7410e" stroke-width="2.2" fill="none" stroke-linecap="round"/><text x="12" y="15.5" font-size="8" font-weight="800" text-anchor="middle" fill="#b7410e" font-family="sans-serif">rs</text>"##,
            Tool::Analyzer => r##"<circle cx="10" cy="10" r="6" fill="none" stroke="#b7410e" stroke-width="2.2"/><path d="M14.5 14.5L20 20" stroke="#b7410e" stroke-width="2.4" stroke-linecap="round"/>"##,
            Tool::Python => r##"<path d="M12 2c-4 0-4 2-4 3v2h4v1H6c-2 0-3 1.5-3 4s1 4 3 4h2v-2.5c0-1.5 1-2.5 2.5-2.5h4c1.5 0 2.5-1 2.5-2.5V5c0-2-2-3-5-3z" fill="#3776ab"/><path d="M12 22c4 0 4-2 4-3v-2h-4v-1h6c2 0 3-1.5 3-4s-1-4-3-4h-2v2.5c0 1.5-1 2.5-2.5 2.5h-4C8 13 7 14 7 15.5V19c0 2 2 3 5 3z" fill="#ffd43b"/>"##,
            Tool::Uv => r##"<text x="12" y="16" font-size="11" font-weight="800" text-anchor="middle" fill="#6f2dbd" font-family="sans-serif">uv</text>"##,
            Tool::Pip => r##"<path d="M4 9l8-4 8 4-8 4z" fill="#3776ab"/><path d="M4 9v6l8 4 8-4V9" stroke="#3776ab" stroke-width="1.6" fill="none"/>"##,
            Tool::Ssh => r##"<circle cx="8" cy="12" r="4" fill="none" stroke="#475569" stroke-width="2.2"/><path d="M12 12h9M18 12v3M21 12v2" stroke="#475569" stroke-width="2.2" stroke-linecap="round"/>"##,
            Tool::Ftp => r##"<path d="M3 7h7l2 2h9v10H3z" fill="#f59e0b"/><path d="M9 15l3-3 3 3M12 12v6" stroke="#fff" stroke-width="1.8" fill="none" stroke-linecap="round"/>"##,
            Tool::LibreCad => r##"<path d="M12 3l8 4.5v9L12 21l-8-4.5v-9z" fill="none" stroke="#0e7490" stroke-width="1.8"/><path d="M4 7.5L12 12l8-4.5M12 12v9" stroke="#0e7490" stroke-width="1.8" fill="none"/>"##,
            Tool::Git => r##"<path d="M12 2l10 10-10 10L2 12z" fill="#f05033"/><circle cx="12" cy="8" r="1.6" fill="#fff"/><circle cx="12" cy="16" r="1.6" fill="#fff"/><circle cx="16" cy="12" r="1.6" fill="#fff"/><path d="M12 8v8M12 8l4 4" stroke="#fff" stroke-width="1.3"/>"##,
            Tool::FreeCad => r##"<path d="M5 4h14v4H9v3h7v4H9v5H5z" fill="#0e7490"/>"##,
            // A book with a B on it. Told apart from Buwiz Forms and LibreOffice by shape.
            Tool::BuwizBooks => r##"<rect x="4" y="3" width="16" height="18" rx="2" fill="#16a34a"/><path d="M8 3v18" stroke="#e7f8ee" stroke-width="1.4"/><text x="13" y="16" font-size="8" font-weight="800" text-anchor="middle" fill="#fff" font-family="sans-serif">B</text>"##,
            // A clipboard with a check, in Buwiz Forms' own emerald.
            Tool::BuwizForms => r##"<rect x="5" y="4" width="14" height="17" rx="2" fill="#059669"/><rect x="9" y="2" width="6" height="4" rx="1" fill="#059669"/><path d="M8.5 13l2.5 2.5L16 10" stroke="#fff" stroke-width="2" fill="none" stroke-linecap="round"/>"##,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum IconSize {
    /// 34px, for picker tiles and list rows.
    Tile,
    /// 22px, for a desktop dock.
    Dock,
    /// 20px, for summaries.
    Mini,
}

/// A tool's icon on its brand-colored square.
pub fn tool_icon(tool: Tool, size: IconSize) -> impl IntoView {
    let (box_class, svg_class) = match size {
        IconSize::Tile => ("grid h-[34px] w-[34px] shrink-0 place-items-center rounded-[9px]", "h-[21px] w-[21px]"),
        IconSize::Dock => ("grid h-[22px] w-[22px] shrink-0 place-items-center rounded-md", "h-[14px] w-[14px]"),
        IconSize::Mini => ("grid h-5 w-5 shrink-0 place-items-center rounded-[5px]", "h-[13px] w-[13px]"),
    };
    view! {
        <span class=box_class style=format!("background:{}", tool.bg()) title=tool.name()>
            <svg viewBox="0 0 24 24" class=svg_class aria-hidden="true" inner_html=tool.svg()></svg>
        </span>
    }
}
