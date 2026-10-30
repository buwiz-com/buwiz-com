// Progressive enhancement for the account and staff pages. Every page works
// without it: forms post as plain HTML forms. Loaded in <head> without
// `defer`, so the `js` class is on <html> before the first paint (the
// add-a-TIN steps would otherwise flash stacked), and the rest waits for
// the page.
document.documentElement.classList.add("js");
(() => {
  "use strict";
  const MAX_FILE_BYTES = 10 * 1024 * 1024;

  function ready() {
    for (const input of document.querySelectorAll("input[data-code]")) codeBoxes(input);
    for (const form of document.querySelectorAll("form[data-addtin]")) addTin(form);
    for (const viewer of document.querySelectorAll(".viewer")) documentViewer(viewer);

    // Files chosen in a drop zone: show the name and size, flag big files.
    document.addEventListener("change", (event) => {
      const input = event.target;
      if (input.type === "file" && input.closest(".slot")) showFiles(input);
    });

    // Forms that end something ask first.
    document.addEventListener("submit", (event) => {
      const message = event.target.dataset && event.target.dataset.confirm;
      if (message && !window.confirm(message)) event.preventDefault();
    });

    // The account menu closes on a click elsewhere or Escape.
    const closeMenus = (except) => {
      for (const menu of document.querySelectorAll("details.top-menu[open]")) {
        if (menu !== except) menu.open = false;
      }
    };
    document.addEventListener("click", (event) => closeMenus(event.target.closest("details.top-menu")));
    document.addEventListener("keydown", (event) => {
      if (event.key === "Escape") closeMenus(null);
    });
  }

  function sizeText(bytes) {
    return bytes >= 1024 * 1024 ? `${(bytes / 1024 / 1024).toFixed(1)} MB` : `${Math.max(1, Math.round(bytes / 1024))} KB`;
  }

  function showFiles(input) {
    const slot = input.closest(".slot");
    const files = Array.from(input.files);
    const tooBig = files.some((file) => file.size > MAX_FILE_BYTES);
    input.setCustomValidity(tooBig ? "Each file must be 10 MB or smaller." : "");
    slot.classList.toggle("has-file", files.length > 0 && !tooBig);
    slot.classList.toggle("too-big", tooBig);
    slot.querySelector("[data-file]").textContent = files.map((file) => `${file.name} · ${sizeText(file.size)}`).join(", ");
    slot.querySelector("[data-action]").textContent = files.length ? "Replace" : "Choose a file";
    if (tooBig) input.reportValidity();
  }

  // The sign-in code: one field becomes six boxes. Typing fills the next box,
  // a paste or the phone's code suggestion fills them all, and the form sends
  // itself once the sixth digit is in. The original field stays, hidden, and
  // carries the code.
  function codeBoxes(field) {
    const form = field.form;
    const boxes = document.createElement("div");
    boxes.className = "code-boxes";
    const cells = [];
    for (let index = 0; index < 6; index++) {
      const cell = document.createElement("input");
      cell.type = "text";
      cell.inputMode = "numeric";
      cell.className = "code-box";
      cell.autocomplete = index === 0 ? "one-time-code" : "off";
      cell.setAttribute("aria-label", `Digit ${index + 1}`);
      cells.push(cell);
      boxes.append(cell);
    }
    cells[0].id = field.id;
    field.id = `${field.id}-value`;
    field.required = false;
    field.tabIndex = -1;
    field.setAttribute("aria-hidden", "true");
    field.classList.add("visually-hidden");
    field.after(boxes);

    let sent = false;
    const sync = () => {
      const code = cells.map((cell) => cell.value).join("");
      field.value = code;
      cells.forEach((cell) => cell.classList.toggle("filled", cell.value !== ""));
      if (code.length === 6 && !sent) {
        sent = true;
        if (form.requestSubmit) form.requestSubmit();
        else form.submit();
      }
    };
    const fill = (start, text) => {
      const digits = text.replace(/\D/g, "");
      let index = start;
      for (const digit of digits) {
        if (index > 5) break;
        cells[index].value = digit;
        index++;
      }
      cells[Math.min(index, 5)].focus();
      sync();
    };
    cells.forEach((cell, index) => {
      cell.addEventListener("input", () => {
        const typed = cell.value;
        cell.value = "";
        if (typed.replace(/\D/g, "")) fill(index, typed);
        else sync();
      });
      cell.addEventListener("keydown", (event) => {
        if (event.key === "Backspace" && cell.value === "" && index > 0) {
          event.preventDefault();
          cells[index - 1].value = "";
          cells[index - 1].focus();
          sent = false;
          sync();
        } else if (event.key === "ArrowLeft" && index > 0) {
          cells[index - 1].focus();
        } else if (event.key === "ArrowRight" && index < 5) {
          cells[index + 1].focus();
        }
      });
      cell.addEventListener("paste", (event) => {
        event.preventDefault();
        fill(index, (event.clipboardData || window.clipboardData).getData("text"));
      });
      cell.addEventListener("focus", () => cell.select());
    });
    fill(0, field.value);
    cells[0].focus();
    const submit = form.querySelector(".auth-submit");
    if (submit) submit.hidden = true;
    const hint = form.querySelector(".code-hint");
    if (hint) hint.hidden = false;
  }

  // Add a TIN: one step at a time, the TIN typed with dashes added, only the
  // picked kind's papers required, and a summary above the send button.
  function addTin(form) {
    const steps = Array.from(form.querySelectorAll(".addtin-step"));
    const tin = form.querySelector("[data-tin]");
    const branch = form.querySelector("#branch");
    const name = form.querySelector("#registered_name");
    const tinHelp = form.querySelector("[data-tin-help]");
    const summary = form.querySelector("[data-summary]");
    const kind = () => (form.querySelector('input[name="kind"]:checked') || {}).value || "personal";
    let current = 0;

    const show = (index) => {
      current = Math.max(0, Math.min(index, steps.length - 1));
      steps.forEach((step, at) => step.classList.toggle("current", at === current));
      window.scrollTo({ top: 0 });
    };
    // Only fields that show count, so the other kind's papers never block.
    const stepIsValid = (step) => {
      for (const input of step.querySelectorAll("input, textarea")) {
        if (input.type === "hidden" || input.closest("[hidden]")) continue;
        if (getComputedStyle(input.closest(".for-personal, .for-business") || input).display === "none") continue;
        if (!input.checkValidity()) {
          input.reportValidity();
          return false;
        }
      }
      return true;
    };

    const digits = (value) => value.replace(/\D/g, "");
    const formatTin = () => {
      const before = digits(tin.value.slice(0, tin.selectionStart || 0)).length;
      let all = digits(tin.value);
      // A TIN pasted with its branch code: the rest goes to the branch field.
      if (all.length > 9) {
        const rest = all.slice(9);
        branch.value = rest.length === 3 ? `00${rest}` : rest.slice(0, 5);
        all = all.slice(0, 9);
      }
      const formatted = all.replace(/(\d{3})(?=\d)/g, "$1-");
      if (formatted !== tin.value) {
        tin.value = formatted;
        let caret = 0;
        for (let seen = 0; caret < formatted.length && seen < Math.min(before, 9); caret++) {
          if (/\d/.test(formatted[caret])) seen++;
        }
        tin.setSelectionRange(caret, caret);
      }
      const complete = all.length === 9;
      tin.setCustomValidity(complete ? "" : "Enter the 9 digits of the TIN.");
      tinHelp.textContent = complete ? `Typed ${all}. Dashes added for you.` : "Nine digits. Dashes are added for you.";
      tinHelp.classList.toggle("ok", complete);
    };

    const update = () => {
      for (const input of form.querySelectorAll("input[data-required]")) {
        const only = input.closest(".for-personal, .for-business");
        input.required = !only || only.classList.contains(`for-${kind()}`);
      }
      const tinText = tin.value ? `${tin.value}-${digits(branch.value).padStart(5, "0") || "00000"}` : "—";
      for (const echo of form.querySelectorAll('[data-echo="tin"]')) echo.textContent = tinText;
      for (const echo of form.querySelectorAll('[data-echo="name"]')) echo.textContent = name.value.trim() || "—";
      const files = Array.from(form.querySelectorAll('input[type="file"]'))
        .filter((input) => !input.closest(`.for-${kind() === "personal" ? "business" : "personal"}`))
        .reduce((count, input) => count + input.files.length, 0);
      const label = kind() === "personal" ? "Personal" : "Business";
      const who = name.value.trim() || "Your TIN";
      summary.textContent = `${who} · ${label} · ${files === 1 ? "1 file" : `${files} files`} ready`;
    };

    tin.addEventListener("input", () => {
      formatTin();
      update();
    });
    branch.addEventListener("input", () => {
      branch.value = digits(branch.value).slice(0, 5);
      update();
    });
    form.addEventListener("input", update);
    form.addEventListener("change", update);
    form.addEventListener("click", (event) => {
      if (event.target.closest("[data-next]") && stepIsValid(steps[current])) show(current + 1);
      else if (event.target.closest("[data-back]")) show(current - 1);
    });
    // Enter in a field moves on instead of sending half a form.
    form.addEventListener("keydown", (event) => {
      if (event.key === "Enter" && event.target.tagName === "INPUT" && current < steps.length - 1) {
        event.preventDefault();
        if (stepIsValid(steps[current])) show(current + 1);
      }
    });

    for (const button of form.querySelectorAll("[data-next], [data-back]")) button.hidden = false;
    form.classList.add("stepped");
    formatTin();
    update();
    show(0);
  }

  // Staff document viewer: zoom and rotate the photo on screen. The tabs
  // themselves are radio buttons and need no script.
  function documentViewer(viewer) {
    const tools = viewer.querySelector("[data-viewer-tools]");
    if (!tools || !viewer.querySelector("img[data-zoomable]")) return;
    tools.hidden = false;
    const looks = new Map();
    const visible = () =>
      Array.from(viewer.querySelectorAll("img[data-zoomable]")).find((image) => image.offsetParent !== null);
    tools.addEventListener("click", (event) => {
      const button = event.target.closest("button");
      const image = visible();
      if (!button || !image) return;
      const look = looks.get(image) || { zoom: 1, turn: 0 };
      if (button.dataset.zoom) look.zoom = Math.min(4, Math.max(0.5, look.zoom + Number(button.dataset.zoom) * 0.25));
      if (button.hasAttribute("data-rotate")) look.turn = (look.turn + 90) % 360;
      looks.set(image, look);
      image.style.transform = `rotate(${look.turn}deg) scale(${look.zoom})`;
    });
  }

  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", ready);
  else ready();
})();
