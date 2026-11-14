// The staff console's script (src/account/pages/staff.rs). Every page works
// without it: selections are links (`?open=`), actions are plain forms with
// their reason inline. With it, the inspector changes in place, the phone
// sheet and the tablet's names open and close, reasons are asked in a
// dialog, and the user page's tabs switch without a reload. No framework,
// no inline script; loaded in <head>, it waits for the page.
document.documentElement.classList.add("js");
(() => {
  "use strict";
  const narrow = () => window.matchMedia("(max-width: 1199.98px)").matches;
  const FOCUSABLE = 'a[href], button:not([disabled]), input:not([disabled]):not([type="hidden"]), select:not([disabled]), textarea:not([disabled]), summary, [tabindex]:not([tabindex="-1"])';

  // Keeps Tab inside `box` while it is open; returns the undo.
  function trap(box) {
    const onKey = (event) => {
      if (event.key !== "Tab") return;
      const items = Array.from(box.querySelectorAll(FOCUSABLE)).filter((item) => item.offsetParent !== null);
      if (!items.length) return;
      const first = items[0];
      const last = items[items.length - 1];
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    };
    box.addEventListener("keydown", onKey);
    return () => box.removeEventListener("keydown", onKey);
  }

  // Everything outside `keep` stops taking focus and clicks while a sheet or
  // modal is open.
  function setInert(keep, on) {
    for (const region of document.querySelectorAll(".sx-side, .sx-top, .sx-main, .sx-tabbar, .sx-fab")) {
      if (!keep.contains(region)) region.inert = on;
    }
  }

  // ---------------------------------------------------------------- confirm
  // Forms that end something without a reason ask first, in the console's
  // own dialog: `data-confirm` the question, `data-confirm-detail` the line
  // under it, `data-confirm-label` the button, `data-confirm-tone="danger"`
  // paints it red.
  let confirmBox = null;
  function askFirst(form, submitter) {
    if (!confirmBox) {
      confirmBox = document.createElement("dialog");
      confirmBox.className = "sx-dialog";
      confirmBox.setAttribute("aria-labelledby", "sx-confirm-title");
      confirmBox.setAttribute("aria-describedby", "sx-confirm-detail");
      confirmBox.innerHTML =
        '<form method="dialog" class="sx-confirm">' +
        '<h2 id="sx-confirm-title"></h2><p id="sx-confirm-detail"></p>' +
        '<div class="sx-form-a"><button class="sx-btn" value="cancel">Cancel</button>' +
        '<button class="sx-btn pri" value="ok" data-ok></button></div></form>';
      document.body.append(confirmBox);
      confirmBox.addEventListener("click", (event) => {
        if (event.target === confirmBox) confirmBox.close("cancel");
      });
    }
    const data = form.dataset;
    confirmBox.querySelector("h2").textContent = data.confirm;
    const detail = confirmBox.querySelector("p");
    detail.textContent = data.confirmDetail || "";
    detail.hidden = !data.confirmDetail;
    const ok = confirmBox.querySelector("[data-ok]");
    ok.textContent = data.confirmLabel || "Confirm";
    ok.className = data.confirmTone === "danger" ? "sx-btn dan solid" : "sx-btn pri";
    confirmBox.returnValue = "cancel";
    confirmBox.onclose = () => {
      if (confirmBox.returnValue !== "ok") return;
      form.dataset.confirmed = "yes";
      if (form.requestSubmit) form.requestSubmit(submitter || undefined);
      else form.submit();
    };
    confirmBox.showModal();
    // The safe choice has focus, so Enter never ends anything by accident.
    confirmBox.querySelector('[value="cancel"]').focus();
  }
  document.addEventListener("submit", (event) => {
    const form = event.target;
    if (!form.dataset || !form.dataset.confirm) return;
    if (form.dataset.confirmed === "yes") {
      delete form.dataset.confirmed;
      return;
    }
    if (typeof HTMLDialogElement !== "function") return;
    event.preventDefault();
    askFirst(form, event.submitter);
  });

  // ---------------------------------------------------------------- reason dialogs
  // A `details.sx-act` holds an action and its required reason. Without
  // script it opens in place; here its panel moves into a modal dialog,
  // labelled by the panel's heading.
  function actDialog(details) {
    if (typeof HTMLDialogElement !== "function") return;
    const summary = details.querySelector(":scope > summary");
    const panel = details.querySelector(":scope > .sx-act-panel");
    if (!summary || !panel) return;
    const dialog = document.createElement("dialog");
    dialog.className = "sx-dialog";
    const heading = panel.querySelector("h3");
    if (heading && heading.id) dialog.setAttribute("aria-labelledby", heading.id);
    dialog.append(panel);
    details.after(dialog);
    summary.setAttribute("aria-haspopup", "dialog");
    summary.addEventListener("click", (event) => {
      event.preventDefault();
      dialog.showModal();
      const first = panel.querySelector("textarea, input:not([type=hidden]), select");
      if (first) first.focus();
    });
    for (const close of panel.querySelectorAll("[data-close]")) {
      close.addEventListener("click", () => dialog.close());
    }
    dialog.addEventListener("click", (event) => {
      if (event.target === dialog) dialog.close();
    });
    dialog.addEventListener("close", () => summary.focus());
  }

  // ---------------------------------------------------------------- the New user modal
  // Rendered by the server on /staff/users for Admins: hidden, or open with
  // ?new=1. Buttons marked data-new-user open it here without a reload.
  function newUserModal() {
    const modal = document.getElementById("new-user");
    if (!modal) return;
    let untrap = null;
    let opener = null;
    const close = () => {
      modal.hidden = true;
      setInert(modal, false);
      if (untrap) untrap();
      const url = new URL(window.location.href);
      if (url.searchParams.has("new")) {
        url.searchParams.delete("new");
        history.replaceState(null, "", url);
      }
      if (opener) opener.focus();
    };
    const open = (from) => {
      opener = from || document.activeElement;
      modal.hidden = false;
      setInert(modal, true);
      untrap = trap(modal);
      const field = modal.querySelector("input[type=email]");
      if (field) field.focus();
    };
    for (const link of modal.querySelectorAll("[data-modal-close]")) {
      link.addEventListener("click", (event) => {
        event.preventDefault();
        close();
      });
    }
    modal.addEventListener("keydown", (event) => {
      if (event.key === "Escape") close();
    });
    document.addEventListener("click", (event) => {
      const trigger = event.target.closest("[data-new-user]");
      if (!trigger) return;
      event.preventDefault();
      open(trigger);
    });
    if (!modal.hidden) open(null);
  }

  // ---------------------------------------------------------------- the inspector
  // Rows link to the same list with ?open=<id>; here the inspector is swapped
  // from that page, the row marked, and the URL kept, so back and reload
  // work. On tablet and phone it opens over the list as a dialog.
  let untrapSheet = null;
  let sheetOpener = null;
  function sheetMode(inspector, open) {
    const scrim = document.querySelector(".sx-scrim");
    inspector.classList.toggle("is-open", open);
    if (scrim) scrim.classList.toggle("is-open", open && narrow());
    const modal = open && narrow();
    if (modal) {
      inspector.setAttribute("role", "dialog");
      inspector.setAttribute("aria-modal", "true");
    } else {
      inspector.removeAttribute("role");
      inspector.removeAttribute("aria-modal");
    }
    setInert(inspector, modal);
    if (untrapSheet) {
      untrapSheet();
      untrapSheet = null;
    }
    if (modal) untrapSheet = trap(inspector);
  }
  // The rows' marks as the server drew them for the inspector's URL: `sel`
  // on the row it shows (with `auto` when nobody chose it) and
  // aria-current on that row's link, every other row unmarked.
  function markRows(doc) {
    const drawn = new Map(Array.from(doc.querySelectorAll("tr[data-id]"), (row) => [row.dataset.id, row]));
    for (const row of document.querySelectorAll("tr[data-id]")) {
      const from = drawn.get(row.dataset.id);
      row.className = from ? from.className : "";
      const link = row.querySelector("a[data-open]");
      if (!link) continue;
      const fromLink = from && from.querySelector("a[data-open]");
      if (fromLink && fromLink.hasAttribute("aria-current")) link.setAttribute("aria-current", fromLink.getAttribute("aria-current"));
      else link.removeAttribute("aria-current");
    }
  }
  // Swaps in the inspector of the page at `href` (a row chosen with ?open=,
  // or the list without it) and marks the rows as that page does.
  async function showInInspector(href, push) {
    const inspector = document.getElementById("inspector");
    if (!inspector) {
      window.location.href = href;
      return;
    }
    let html;
    try {
      const response = await fetch(href, { credentials: "same-origin", headers: { Accept: "text/html" } });
      if (!response.ok || response.redirected) throw new Error(String(response.status));
      html = await response.text();
    } catch (error) {
      window.location.href = href;
      return;
    }
    const page = new DOMParser().parseFromString(html, "text/html");
    const next = page.getElementById("inspector");
    if (!next) {
      window.location.href = href;
      return;
    }
    inspector.innerHTML = next.innerHTML;
    inspector.setAttribute("aria-label", next.getAttribute("aria-label") || "");
    enhance(inspector);
    if (push) history.pushState({ inspector: true }, "", href);
    markRows(page);
    const open = next.classList.contains("is-open");
    sheetMode(inspector, open);
    const body = inspector.querySelector(".sx-insp-b");
    if (body) body.scrollTop = 0;
    if (!open) return;
    const heading = inspector.querySelector(".sx-insp-h h2");
    if (heading) {
      heading.tabIndex = -1;
      heading.focus({ preventScroll: true });
    }
  }
  function closeInspector(link) {
    const inspector = document.getElementById("inspector");
    sheetMode(inspector, false);
    history.pushState({ inspector: true }, "", link.href);
    if (sheetOpener && document.contains(sheetOpener)) sheetOpener.focus();
    // The list without a choice: its own panel and marks.
    showInInspector(link.href, false);
  }
  document.addEventListener("click", (event) => {
    if (event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
    const close = event.target.closest("[data-close-insp]");
    if (close && narrow()) {
      event.preventDefault();
      closeInspector(close);
      return;
    }
    const open = event.target.closest("a[data-open]");
    if (open) {
      event.preventDefault();
      sheetOpener = open;
      showInInspector(open.href, true);
      return;
    }
    // A click anywhere on a row follows its link.
    const row = event.target.closest("tr[data-id]");
    if (row && !event.target.closest("a, button, input, label, select, textarea")) {
      const link = row.querySelector("a[data-open], a.sx-rowlink");
      if (link) link.click();
    }
  });
  // Back and forward: the inspector and the marks follow the URL, with or
  // without ?open= (without it, the list's own first row, unchosen).
  window.addEventListener("popstate", () => {
    const inspector = document.getElementById("inspector");
    if (!inspector) return;
    const url = new URL(window.location.href);
    if (!url.searchParams.get("open")) sheetMode(inspector, false);
    showInInspector(url.href, false);
  });
  // The queue's "More" menu closes on a click elsewhere or Escape.
  const closeMenus = (except) => {
    for (const menu of document.querySelectorAll("details.sx-moretabs[open]")) {
      if (menu !== except) menu.open = false;
    }
  };
  document.addEventListener("click", (event) => closeMenus(event.target.closest("details.sx-moretabs")));
  document.addEventListener("keydown", (event) => {
    if (event.key !== "Escape") return;
    closeMenus(null);
    const inspector = document.getElementById("inspector");
    if (inspector && inspector.classList.contains("is-open") && narrow() && !document.querySelector("dialog[open]")) {
      const close = inspector.querySelector("[data-close-insp]");
      if (close) closeInspector(close);
    }
    if (document.documentElement.classList.contains("sx-rail-open")) toggleRail(false);
  });

  // ---------------------------------------------------------------- the tablet rail
  function toggleRail(open) {
    const root = document.documentElement;
    const want = open === undefined ? !root.classList.contains("sx-rail-open") : open;
    root.classList.toggle("sx-rail-open", want);
    const button = document.querySelector(".sx-rail-toggle");
    if (button) {
      button.setAttribute("aria-expanded", String(want));
      button.setAttribute("aria-label", want ? "Hide names" : "Show names");
      const use = button.querySelector("use");
      // The icons are the page's own (the inline sprite), named by id.
      if (use) use.setAttribute("href", want ? "#chevrons-left" : "#chevrons-right");
    }
    let veil = document.querySelector(".sx-rail-veil");
    if (want && !veil) {
      veil = document.createElement("div");
      veil.className = "sx-rail-veil";
      veil.addEventListener("click", () => toggleRail(false));
      document.body.append(veil);
    } else if (!want && veil) {
      veil.remove();
    }
  }

  // ---------------------------------------------------------------- tabs
  // The user page's tabs are links (?tab=); here they switch in place.
  function tabs(list) {
    const items = Array.from(list.querySelectorAll('[role="tab"]'));
    const select = (tab, focus) => {
      for (const item of items) {
        const on = item === tab;
        item.setAttribute("aria-selected", String(on));
        item.tabIndex = on ? 0 : -1;
        const panel = document.getElementById(item.getAttribute("aria-controls"));
        if (panel) panel.hidden = !on;
      }
      if (focus) tab.focus();
      // A form outside every tab (the page head's) goes back to the tab shown.
      for (const field of document.querySelectorAll('input[type="hidden"][name="tab"]')) {
        if (!field.closest('[role="tabpanel"]')) field.value = tab.dataset.tab;
      }
      const url = new URL(window.location.href);
      url.searchParams.set("tab", tab.dataset.tab);
      history.replaceState(history.state, "", url);
    };
    for (const item of items) {
      item.addEventListener("click", (event) => {
        event.preventDefault();
        select(item, false);
      });
      item.addEventListener("keydown", (event) => {
        const at = items.indexOf(item);
        let to = null;
        if (event.key === "ArrowRight") to = items[(at + 1) % items.length];
        else if (event.key === "ArrowLeft") to = items[(at - 1 + items.length) % items.length];
        else if (event.key === "Home") to = items[0];
        else if (event.key === "End") to = items[items.length - 1];
        if (to) {
          event.preventDefault();
          select(to, true);
        }
      });
    }
  }

  // ---------------------------------------------------------------- document viewer
  // Zoom and rotate a photo on screen; each photo's pane has its own tools.
  // The tabs are radio buttons.
  function documentViewer(viewer) {
    for (const tools of viewer.querySelectorAll("[data-viewer-tools]")) {
      const pane = tools.closest(".sx-viewer-pane");
      const image = pane && pane.querySelector("img[data-zoomable]");
      if (!image) continue;
      tools.hidden = false;
      const look = { zoom: 1, turn: 0 };
      tools.addEventListener("click", (event) => {
        const button = event.target.closest("button");
        if (!button) return;
        if (button.dataset.zoom) look.zoom = Math.min(4, Math.max(0.5, look.zoom + Number(button.dataset.zoom) * 0.25));
        if (button.hasAttribute("data-rotate")) look.turn = (look.turn + 90) % 360;
        image.style.transform = `rotate(${look.turn}deg) scale(${look.zoom})`;
      });
    }
  }

  function enhance(root) {
    for (const details of root.querySelectorAll("details.sx-act")) actDialog(details);
    for (const viewer of root.querySelectorAll(".sx-viewer")) documentViewer(viewer);
    for (const list of root.querySelectorAll('[role="tablist"][data-tabs]')) tabs(list);
  }

  function ready() {
    enhance(document);
    newUserModal();
    const rail = document.querySelector(".sx-rail-toggle");
    if (rail) {
      rail.hidden = false;
      rail.addEventListener("click", () => toggleRail());
    }
    // A sheet the server opened (?open=) behaves like one opened here.
    const inspector = document.getElementById("inspector");
    if (inspector && inspector.classList.contains("is-open")) sheetMode(inspector, true);
    window.matchMedia("(max-width: 1199.98px)").addEventListener("change", () => {
      const open = document.getElementById("inspector");
      if (open) sheetMode(open, open.classList.contains("is-open"));
      if (!narrow()) toggleRail(false);
    });
  }

  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", ready);
  else ready();
})();
