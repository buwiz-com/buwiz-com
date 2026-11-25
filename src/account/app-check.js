// /app-check (src/account/pages/app_check.rs): hands the Turnstile widget's
// result to the app that opened the page. In this order: the app's own
// bridge (`window.nsh`), a message to a parent or opener window, the
// document title, and last the address of /app-check/done, which a host
// that saw none of the others reads.
(function () {
  "use strict";

  function deliver(message, typed, title, fragment) {
    try {
      if (window.nsh && typeof window.nsh.postMessage === "function") {
        window.nsh.postMessage(JSON.stringify(message));
      }
    } catch (ignored) {}
    try {
      if (window.parent && window.parent !== window) {
        window.parent.postMessage(typed, "*");
      }
    } catch (ignored) {}
    try {
      if (window.opener) {
        window.opener.postMessage(typed, "*");
      }
    } catch (ignored) {}
    document.title = title;
    window.location.assign("/app-check/done#" + fragment);
  }

  function succeed(token) {
    deliver(
      { turnstile: token },
      { type: "buwiz-turnstile", token: token },
      "buwiz-turnstile:" + token,
      "token=" + encodeURIComponent(token)
    );
  }

  function fail(code) {
    deliver(
      { turnstile_error: code },
      { type: "buwiz-turnstile", error: code },
      "buwiz-turnstile-error:" + code,
      "error=" + encodeURIComponent(code)
    );
  }

  window.buwizTurnstileDone = succeed;
  window.buwizTurnstileError = function (code) {
    fail(code ? "error_" + String(code).replace(/[^A-Za-z0-9_-]/g, "") : "error");
    // Stops the widget retrying on its own: the app asks again.
    return true;
  };
  window.buwizTurnstileExpired = function () {
    fail("expired");
  };
  window.buwizTurnstileTimeout = function () {
    fail("timeout");
  };

  function start() {
    var box = document.getElementById("app-check");
    if (box && box.getAttribute("data-state") === "not-configured") {
      fail("not_configured");
    }
  }
  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", start);
  } else {
    start();
  }
})();
