// The download choices on the home page and /download. Every link and
// button works without it; this only refines what the server chose from
// User-Agent. It never starts a download by itself.
(() => {
  "use strict";

  // The processor, where the browser says (Chrome and Edge); x64 elsewhere.
  const processor = () => {
    const agent = navigator.userAgentData;
    if (!agent || !agent.getHighEntropyValues) return Promise.resolve("x64");
    return agent
      .getHighEntropyValues(["architecture", "bitness"])
      .then((values) => (values.architecture === "arm" && values.bitness === "64" ? "arm64" : "x64"))
      .catch(() => "x64");
  };

  // The home page's Download button.
  function hero(get) {
    // An iPad asking for desktop sites says Macintosh, but has a touch
    // screen: it gets the link to open on a computer, not the .dmg.
    const mac = get.querySelector('[data-get="mac"]');
    const ios = get.querySelector('[data-get="ios"]');
    if (mac && ios && /Macintosh/.test(navigator.userAgent) && navigator.maxTouchPoints > 1) {
      mac.hidden = true;
      ios.hidden = false;
    }

    // Linux on ARM64 gets the ARM64 deb, when there is one.
    const linux = get.querySelector("a[data-arm64-href]");
    if (linux) {
      processor().then((arch) => {
        if (arch === "arm64") linux.href = linux.dataset.arm64Href;
      });
    }

    const more = get.querySelector(".get-more");
    const files = document.getElementById(more.getAttribute("aria-controls"));
    more.addEventListener("click", () => {
      const open = more.getAttribute("aria-expanded") !== "true";
      more.setAttribute("aria-expanded", String(open));
      files.hidden = !open;
    });
  }

  // The Linux card: the button names the file the radios pick, and its
  // facts follow. Every Linux file is listed in the card with its facts.
  function picker(form) {
    const card = form.closest(".dl-card");
    const label = form.querySelector("[data-pick-label]");
    const button = label.closest("button");
    const facts = form.querySelector("[data-pick-facts]");
    let touched = false;

    const update = () => {
      const kind = form.elements.kind.value;
      const arch = form.elements.arch.value;
      const file = [...card.querySelectorAll("li[data-kind]")].find(
        (item) => item.dataset.kind === kind && item.dataset.arch === arch,
      );
      button.disabled = !file;
      facts.hidden = !file;
      if (!file) {
        label.textContent = "Not available for this choice yet";
        return;
      }
      label.textContent = "Download " + file.dataset.name;
      for (const fact of facts.querySelectorAll("[data-fact]")) fact.textContent = file.dataset[fact.dataset.fact];
    };

    form.addEventListener("change", () => {
      touched = true;
      update();
    });

    // Unless a link chose the processor, pick the one the browser says.
    if (!new URLSearchParams(location.search).has("arch")) {
      processor().then((arch) => {
        const radio = form.querySelector('input[name="arch"][value="' + arch + '"]');
        if (!touched && radio && !radio.checked) {
          radio.checked = true;
          update();
        }
      });
    }
  }

  // Copy buttons, shown only where the clipboard can be written. The
  // group's status line says what happened, for screen readers too.
  function copier(button) {
    if (!navigator.clipboard) return;
    button.hidden = false;
    const status = button.closest("[data-copy-group]").querySelector('[role="status"]');
    button.addEventListener("click", () => {
      navigator.clipboard.writeText(button.dataset.copy).then(
        () => {
          status.textContent = "Copied";
        },
        () => {
          status.textContent = "Could not copy; select the text instead";
        },
      );
    });
  }

  for (const get of document.querySelectorAll(".get")) hero(get);
  for (const form of document.querySelectorAll("form[data-pick]")) picker(form);
  for (const button of document.querySelectorAll("button[data-copy]")) copier(button);
})();
