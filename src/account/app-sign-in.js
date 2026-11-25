// /app-sign-in (src/account/pages/app_sign_in.rs): reads the request and
// the link token from the fragment and shows a button that opens the app
// with them. Nothing happens until the person presses it: no redirect, no
// request to the server.
(function () {
  var params = new URLSearchParams(window.location.hash.slice(1));
  var request = params.get("r");
  var token = params.get("t");
  var safe = /^[A-Za-z0-9_-]{1,128}$/;
  if (!request || !token || !safe.test(request) || !safe.test(token)) {
    return;
  }
  var button = document.getElementById("open-app");
  if (!button) {
    return;
  }
  button.setAttribute(
    "href",
    "ebirforms://signin?r=" + encodeURIComponent(request) + "&t=" + encodeURIComponent(token)
  );
  button.hidden = false;
})();
