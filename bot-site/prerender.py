#!/usr/bin/env python3
"""Snapshot the app's pages as plain HTML, so crawlers that do not run
JavaScript (most AI answer engines) read the same page people see.

build.sh runs this after `trunk build`. It serves dist/ the way the server
does (pages at their paths, the bundle under /bot/, the server's images
under /assets/), loads each page in headless Chrome, and writes what the
app drew inside <body> to dist/prerender/<name>.html. src/bot_site.rs puts
that into the page inside <div id="prerender">, and the app removes it
before it mounts (src/main.rs).

    python3 bot-site/prerender.py   # from anywhere; needs Google Chrome
"""

import http.server
import os
import re
import shutil
import socketserver
import subprocess
import sys
import tempfile
import threading

HERE = os.path.dirname(os.path.abspath(__file__))
DIST = os.path.join(HERE, "dist")
ASSETS = os.path.join(HERE, "..", "src", "assets")

# (path, file name): keep in step with PAGES in src/bot_site.rs.
PAGES = [("/", "home"), ("/mini-apps", "mini-apps")]

CHROME_CANDIDATES = [
    os.environ.get("CHROME", ""),
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "google-chrome",
    "chromium",
]


def chrome():
    for c in CHROME_CANDIDATES:
        if c and (os.path.exists(c) or shutil.which(c)):
            return c
    sys.exit("prerender: no Chrome found; set CHROME=/path/to/chrome")


class Handler(http.server.SimpleHTTPRequestHandler):
    def translate_path(self, path):
        path = path.split("?", 1)[0].split("#", 1)[0]
        if path in {p for p, _ in PAGES}:
            return os.path.join(DIST, "index.html")
        if path.startswith("/bot/"):
            return os.path.join(DIST, path[len("/bot/"):])
        if path.startswith("/assets/"):
            return os.path.join(ASSETS, path[len("/assets/"):])
        return os.path.join(DIST, "__missing__")

    def log_message(self, *args):
        pass


def body_of(html):
    m = re.search(r"<body[^>]*>(.*)</body>", html, re.S)
    if not m:
        sys.exit("prerender: no <body> in Chrome's output")
    body = m.group(1)
    # Scripts and the crawler fallback are the shell's, not the page's.
    body = re.sub(r"<script\b.*?</script>", "", body, flags=re.S)
    body = re.sub(r"<noscript\b.*?</noscript>", "", body, flags=re.S)
    # Links into the app only work with the app; keep them as plain links.
    return body.strip()


def main():
    httpd = socketserver.TCPServer(("127.0.0.1", 0), Handler)
    port = httpd.server_address[1]
    threading.Thread(target=httpd.serve_forever, daemon=True).start()
    out_dir = os.path.join(DIST, "prerender")
    os.makedirs(out_dir, exist_ok=True)
    profile = tempfile.mkdtemp(prefix="buwiz-prerender-")
    try:
        for path, name in PAGES:
            html = subprocess.run(
                [
                    chrome(),
                    "--headless=new",
                    "--disable-gpu",
                    "--no-first-run",
                    f"--user-data-dir={profile}",
                    "--window-size=1440,900",
                    "--virtual-time-budget=8000",
                    "--dump-dom",
                    f"http://127.0.0.1:{port}{path}",
                ],
                capture_output=True,
                text=True,
                timeout=120,
                check=True,
            ).stdout
            body = body_of(html)
            if "<footer" not in body or "<h1" not in body:
                sys.exit(f"prerender: {path} did not render (no <h1> or <footer>)")
            with open(os.path.join(out_dir, f"{name}.html"), "w") as f:
                f.write(body + "\n")
            print(f"prerender: {path} -> dist/prerender/{name}.html ({len(body)} bytes)")
    finally:
        httpd.shutdown()
        shutil.rmtree(profile, ignore_errors=True)


if __name__ == "__main__":
    main()
