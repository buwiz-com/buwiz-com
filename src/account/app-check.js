// /app-check (src/account/pages/app_check.rs): hands the Turnstile widget's
// result to the app that opened the page, two ways only: the app's own
// bridge (`window.nsh`) first, then the address of /app-check/done, which
// a host without the bridge reads. Nothing is posted to any other window.
(function () {
  "use strict";

  function deliver(message, fragment) {
    try {
      if (window.nsh && typeof window.nsh.postMessage === "function") {
        window.nsh.postMessage(JSON.stringify(message));
      }
    } catch (ignored) {}
    window.location.assign("/app-check/done#" + fragment);
  }

  function succeed(token) {
    deliver({ turnstile: token }, "token=" + encodeURIComponent(token));
  }

  function fail(code) {
    deliver({ turnstile_error: code }, "error=" + encodeURIComponent(code));
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
