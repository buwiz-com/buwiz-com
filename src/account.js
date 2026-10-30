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

    // Forms that end something ask first, in a dialog that matches the site
    // (never the browser's own confirm box). `data-confirm` is the question,
    // `data-confirm-detail` the line under it, `data-confirm-label` the
    // button, and `data-confirm-tone="danger"` paints that button red.
    let confirmBox = null;
    const askFirst = (form, submitter) => {
      if (!confirmBox) {
        confirmBox = document.createElement("dialog");
        confirmBox.className = "home confirm-dialog";
        confirmBox.setAttribute("aria-labelledby", "confirm-title");
        confirmBox.setAttribute("aria-describedby", "confirm-detail");
        confirmBox.innerHTML =
          '<form method="dialog" class="confirm-box">' +
          '<h2 class="confirm-title" id="confirm-title"></h2>' +
          '<p class="confirm-detail" id="confirm-detail"></p>' +
          '<div class="confirm-actions">' +
          '<button class="btn btn-ghost" value="cancel">Cancel</button>' +
          '<button class="btn btn-solid confirm-ok" value="ok"></button>' +
          "</div></form>";
        document.body.append(confirmBox);
        // A click on the backdrop cancels.
        confirmBox.addEventListener("click", (event) => {
          if (event.target === confirmBox) confirmBox.close("cancel");
        });
      }
      const data = form.dataset;
      confirmBox.querySelector(".confirm-title").textContent = data.confirm;
      const detail = confirmBox.querySelector(".confirm-detail");
      detail.textContent = data.confirmDetail || "";
      detail.hidden = !data.confirmDetail;
      const ok = confirmBox.querySelector(".confirm-ok");
      ok.textContent = data.confirmLabel || "Confirm";
      ok.classList.toggle("confirm-danger", data.confirmTone === "danger");
      confirmBox.returnValue = "cancel";
      confirmBox.onclose = () => {
        if (confirmBox.returnValue !== "ok") return;
        form.dataset.confirmed = "yes";
        if (form.requestSubmit) form.requestSubmit(submitter || undefined);
        else form.submit();
      };
      confirmBox.showModal();
      // The safe choice has focus, so Enter never destroys anything by accident.
      confirmBox.querySelector('[value="cancel"]').focus();
    };
    document.addEventListener("submit", (event) => {
      const form = event.target;
      if (!form.dataset || !form.dataset.confirm) return;
      if (form.dataset.confirmed === "yes") {
        delete form.dataset.confirmed;
        return;
      }
      // Browsers without <dialog> (none current) just send the form.
      if (typeof HTMLDialogElement !== "function") return;
      event.preventDefault();
      askFirst(form, event.submitter);
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
    const send = form.querySelector("[data-send]");
    const sendLabel = form.querySelector("[data-send-label]");
    const kind = () => (form.querySelector('input[name="kind"]:checked') || {}).value || "personal";
    const other = () => `.for-${kind() === "personal" ? "business" : "personal"}`;
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
      const inputs = Array.from(form.querySelectorAll('input[type="file"]')).filter((input) => !input.closest(other()));
      const files = inputs.reduce((count, input) => count + input.files.length, 0);
      const label = kind() === "personal" ? "Personal" : "Business";
      const who = name.value.trim() || "Your TIN";
      summary.textContent = `${who} · ${label} · ${files === 1 ? "1 file" : `${files} files`} ready`;
      // The send button waits for the required papers and says how many
      // are still missing.
      const missing = inputs.filter((input) => input.required && input.files.length === 0).length;
      send.disabled = missing > 0;
      sendLabel.textContent = missing ? `Add ${missing} more ${missing === 1 ? "file" : "files"}` : "Send for review";
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

    // Papers of the other kind stay out of the upload.
    form.addEventListener("submit", () => {
      for (const input of form.querySelectorAll(`${other()} input[type="file"]`)) input.disabled = true;
    });
    window.addEventListener("pageshow", () => {
      for (const input of form.querySelectorAll('input[type="file"]')) input.disabled = false;
    });

    for (const doc of form.querySelectorAll("[data-doc]")) docZone(doc);
    for (const chip of form.querySelectorAll("[data-chip]")) extraChip(chip);
    for (const button of form.querySelectorAll("[data-next], [data-back]")) button.hidden = false;
    form.classList.add("stepped");
    formatTin();
    update();
    show(0);
  }

  // A document on Add a TIN (doc_zone in src/pages/tins.rs): drop or pick a
  // file, check its type and size here, and show it as a tile with a
  // preview, Replace and Remove. The input keeps the files, so the form
  // posts them as before.
  const DOC_TYPES = {
    "application/pdf": "PDF",
    "image/jpeg": "JPG",
    "image/png": "PNG",
    "image/webp": "WEBP",
    "image/heic": "HEIC",
    "image/heif": "HEIC",
  };
  const DOC_EXTENSIONS = { pdf: "PDF", jpg: "JPG", jpeg: "JPG", png: "PNG", webp: "WEBP", heic: "HEIC", heif: "HEIC" };
  const MAX_SUPPORTING = 5;

  function docType(file) {
    return DOC_TYPES[file.type] || DOC_EXTENSIONS[(file.name.split(".").pop() || "").toLowerCase()] || "";
  }

  function element(tag, className, text) {
    const node = document.createElement(tag);
    if (className) node.className = className;
    if (text !== undefined) node.textContent = text;
    return node;
  }

  function iconElement(name) {
    const node = element("span", "icon", name);
    node.setAttribute("aria-hidden", "true");
    return node;
  }

  // Counts a PDF's pages from its page objects. Compressed object streams
  // hide them, so no count is fine too.
  async function pdfPages(file) {
    try {
      const text = await file.slice(0, MAX_FILE_BYTES).text();
      return (text.match(/\/Type\s*\/Page(?![a-z])/g) || []).length;
    } catch {
      return 0;
    }
  }

  function docZone(doc) {
    const zone = doc.querySelector("[data-zone]");
    const input = doc.querySelector('input[type="file"]');
    const list = doc.querySelector("[data-files]");
    const error = doc.querySelector("[data-error]");
    const title = doc.dataset.title;
    const multiple = doc.dataset.multiple === "true";
    const selfie = input.accept === "image/*";
    let files = [];
    let urls = [];
    let adding = false;

    const setFiles = (chosen) => {
      const transfer = new DataTransfer();
      for (const file of chosen) transfer.items.add(file);
      input.files = transfer.files;
    };
    const fail = (message) => {
      error.textContent = "";
      error.append(iconElement("error"), element("span", "", message));
      doc.classList.add("invalid");
    };
    const check = (chosen) => {
      if (multiple && chosen.length > MAX_SUPPORTING) return `Up to ${MAX_SUPPORTING} other papers, please.`;
      for (const file of chosen) {
        const type = docType(file);
        if (!type || (selfie && type === "PDF")) {
          return selfie
            ? `${file.name} isn't a photo. Use a JPG or PNG.`
            : `${file.name} can't be used. Send a JPG, PNG or PDF.`;
        }
        if (file.size > MAX_FILE_BYTES) return `${file.name} is ${sizeText(file.size)}. Files must be 10 MB or smaller.`;
      }
      return "";
    };
    // New files from the picker or a drop. A bad one leaves the old ones.
    const take = (chosen) => {
      if (!multiple) chosen = chosen.slice(0, 1);
      else if (adding) chosen = files.concat(chosen);
      adding = false;
      if (!chosen.length) return;
      const problem = check(chosen);
      if (problem) {
        setFiles(files);
        fail(problem);
        return;
      }
      files = chosen;
      setFiles(files);
      render();
      const first = list.querySelector("button");
      if (first) first.focus({ preventScroll: true });
    };
    const remove = (index) => {
      files = files.filter((_, at) => at !== index);
      setFiles(files);
      render();
      if (files.length) list.querySelector("button").focus({ preventScroll: true });
      else input.focus({ preventScroll: true });
    };

    const tile = (file, index) => {
      const type = docType(file);
      const row = element("div", "tile");
      const thumb = element("span", "tile-thumb");
      if (type === "PDF") {
        thumb.classList.add("pdf");
        thumb.append(iconElement("picture_as_pdf"), element("span", "tile-glyph", "PDF"));
      } else {
        const url = URL.createObjectURL(file);
        urls.push(url);
        const image = element("img");
        image.alt = "";
        image.src = url;
        // HEIC photos don't preview in most browsers.
        image.addEventListener("error", () => image.replaceWith(iconElement("image")));
        thumb.append(image);
      }
      const info = element("span", "tile-info");
      const name = element("span", "tile-name", file.name);
      name.title = file.name;
      const meta = element("span", "tile-meta");
      const detail = element("span", "", `${sizeText(file.size)} · ${type}`);
      meta.append(iconElement("check_circle"), detail);
      info.append(element("span", "tile-kind", title), name, meta);
      if (type === "PDF") {
        pdfPages(file).then((pages) => {
          if (pages) detail.textContent = `${sizeText(file.size)} · PDF, ${pages === 1 ? "1 page" : `${pages} pages`}`;
        });
      }
      const actions = element("span", "tile-actions");
      if (!multiple) {
        const replace = element("button", "tile-action", "Replace");
        replace.type = "button";
        replace.setAttribute("aria-label", `Replace ${title}: ${file.name}`);
        replace.addEventListener("click", () => input.click());
        actions.append(replace);
      }
      const drop = element("button", "tile-action quiet", "Remove");
      drop.type = "button";
      drop.setAttribute("aria-label", `Remove ${file.name}`);
      drop.addEventListener("click", () => remove(index));
      actions.append(drop);
      row.append(thumb, info, actions);
      return row;
    };

    const render = () => {
      for (const url of urls) URL.revokeObjectURL(url);
      urls = [];
      list.textContent = "";
      error.textContent = "";
      doc.classList.remove("invalid");
      files.forEach((file, index) => list.append(tile(file, index)));
      if (multiple && files.length && files.length < MAX_SUPPORTING) {
        const more = element("button", "tile-more");
        more.type = "button";
        more.append(iconElement("add"), element("span", "", "Add another file"));
        more.addEventListener("click", () => {
          adding = true;
          input.click();
        });
        list.append(more);
      }
      zone.classList.toggle("has-file", files.length > 0);
      // The hidden input stays out of the tab order behind a tile.
      input.tabIndex = files.length ? -1 : 0;
      doc.dispatchEvent(new CustomEvent("doc-files", { bubbles: true, detail: { files, urls } }));
      input.dispatchEvent(new Event("input", { bubbles: true }));
    };

    input.addEventListener("change", () => take(Array.from(input.files)));
    input.addEventListener("cancel", () => {
      adding = false;
    });

    // Drag and drop on the whole zone, empty or filled.
    let depth = 0;
    zone.addEventListener("dragenter", (event) => {
      if (!Array.from(event.dataTransfer.types).includes("Files")) return;
      event.preventDefault();
      depth++;
      zone.classList.add("drag");
    });
    zone.addEventListener("dragover", (event) => {
      if (!Array.from(event.dataTransfer.types).includes("Files")) return;
      event.preventDefault();
      event.dataTransfer.dropEffect = "copy";
    });
    zone.addEventListener("dragleave", () => {
      depth = Math.max(0, depth - 1);
      if (!depth) zone.classList.remove("drag");
    });
    zone.addEventListener("drop", (event) => {
      event.preventDefault();
      depth = 0;
      zone.classList.remove("drag");
      adding = multiple && files.length > 0;
      take(Array.from(event.dataTransfer.files));
    });

    // The selfie: the camera on phones, or a photo already taken.
    for (const way of doc.querySelectorAll("[data-capture]")) {
      way.addEventListener("click", () => {
        if (way.dataset.capture) input.setAttribute("capture", way.dataset.capture);
        else input.removeAttribute("capture");
        input.click();
      });
    }
    if (doc.querySelector("[data-capture]")) input.removeAttribute("capture");
  }

  // An optional paper's chip opens its zone below the row; pressing it
  // again, or Cancel, closes the zone while it is empty.
  function extraChip(chip) {
    const panel = document.getElementById(chip.getAttribute("aria-controls"));
    const doc = panel.querySelector("[data-doc]");
    const tile = chip.querySelector(".extra-chip-tile");
    const badge = chip.querySelector("[data-badge]");
    const state = chip.querySelector("[data-chip-state]");
    let hasFile = false;

    const sync = () => {
      const open = panel.classList.contains("open");
      chip.setAttribute("aria-expanded", String(open));
      panel.inert = !open;
      badge.textContent = hasFile ? "check" : open ? "remove" : "add";
      chip.classList.toggle("has-file", hasFile);
      state.textContent = hasFile ? ", file added" : "";
    };
    const setOpen = (open) => {
      panel.classList.toggle("open", open);
      sync();
    };
    chip.addEventListener("click", () => {
      if (!panel.classList.contains("open")) {
        setOpen(true);
        const reduce = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
        setTimeout(() => panel.scrollIntoView({ block: "nearest", behavior: reduce ? "auto" : "smooth" }), reduce ? 0 : 200);
      } else if (!hasFile) {
        setOpen(false);
      } else {
        (panel.querySelector(".tile button") || chip).focus();
      }
    });
    panel.querySelector("[data-close]").addEventListener("click", () => {
      setOpen(false);
      chip.focus();
    });
    doc.addEventListener("doc-files", (event) => {
      const { files, urls } = event.detail;
      hasFile = files.length > 0;
      const old = tile.querySelector("img");
      if (old) old.remove();
      if (urls.length) {
        const image = element("img", "extra-chip-img");
        image.alt = "";
        image.src = urls[0];
        image.addEventListener("error", () => image.remove());
        tile.prepend(image);
      }
      sync();
    });
    sync();
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
