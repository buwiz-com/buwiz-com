// The home page's self-playing AI Co-worker demos, one per stage of a filing: connect, tax
// profiles, prepare, submit, confirmation and pay. Ported from the old server's Leptos films
// (src/app/marketing/demo in buwiz-server); the class names are Tailwind utilities, compiled
// into /assets/demo.css (see src/demo.tw.css).
//
// The page renders an empty `[data-film]` box per demo; this script draws the app window into
// each and plays it. Radios named `demo` pick the one on show.
(() => {
  "use strict";

  // ---------------------------------------------------------------------------------------
  // Signals and effects: just enough reactivity for the films' views. An effect re-runs when a
  // signal it read changes, and throws away the effects it made on its last run.

  let owner = null;
  let listener = null;

  function signal(value) {
    const subs = new Set();
    return {
      get() {
        if (listener) {
          subs.add(listener);
          listener.deps.add(subs);
        }
        return value;
      },
      peek: () => value,
      set(next) {
        if (Object.is(next, value)) return;
        value = next;
        for (const e of [...subs]) e.run();
      },
    };
  }

  function unlink(node) {
    for (const subs of node.deps) subs.delete(node);
    node.deps.clear();
    for (const kid of node.kids) {
      kid.dead = true;
      unlink(kid);
    }
    node.kids = [];
  }

  function within(node, track, fn) {
    const [o, l] = [owner, listener];
    owner = node;
    listener = track ? node : null;
    try {
      return fn();
    } finally {
      owner = o;
      listener = l;
    }
  }

  function effect(fn) {
    const node = {
      deps: new Set(),
      kids: [],
      dead: false,
      run() {
        if (node.dead) return;
        unlink(node);
        within(node, true, fn);
      },
    };
    if (owner) owner.kids.push(node);
    node.run();
  }

  /// Runs `fn` untracked under a new owner; `dispose()` ends the effects it made.
  function root(fn) {
    const node = { deps: new Set(), kids: [], dead: false };
    const value = within(node, false, fn);
    return { value, dispose: () => unlink(node) };
  }

  /// A signal that follows `fn`, so views only redraw when its value changes.
  function memo(fn) {
    const s = signal(undefined);
    effect(() => s.set(fn()));
    return s;
  }

  // ---------------------------------------------------------------------------------------
  // Views. `h(tag, props, ...kids)`: a prop or kid that is a function is live.

  const SVG = new Set(["svg", "path"]);

  const ref = () => ({ el: null });

  function attr(el, key, value) {
    if (value == null || value === false) el.removeAttribute(key);
    else el.setAttribute(key, value === true ? "" : String(value));
  }

  function h(tag, props, ...kids) {
    const el = SVG.has(tag)
      ? document.createElementNS("http://www.w3.org/2000/svg", tag)
      : document.createElement(tag);
    for (const [key, value] of Object.entries(props || {})) {
      if (key === "ref") value.el = el;
      else if (key.startsWith("on")) el.addEventListener(key.slice(2), value);
      else if (typeof value === "function") effect(() => attr(el, key, value()));
      else attr(el, key, value);
    }
    add(el, kids);
    return el;
  }

  function add(parent, kid) {
    if (kid == null || kid === false || kid === true) return;
    if (Array.isArray(kid)) for (const k of kid) add(parent, k);
    else if (typeof kid === "function") live(parent, kid);
    else if (kid instanceof Node) parent.appendChild(kid);
    else parent.appendChild(document.createTextNode(String(kid)));
  }

  function live(parent, fn) {
    const start = document.createComment("");
    const end = document.createComment("");
    parent.append(start, end);
    effect(() => {
      const frag = document.createDocumentFragment();
      add(frag, fn());
      while (start.nextSibling && start.nextSibling !== end) start.nextSibling.remove();
      end.parentNode.insertBefore(frag, end);
    });
  }

  // ---------------------------------------------------------------------------------------
  // The player: a pausable timeline on its own clock, and a cursor that moves like a hand
  // (curved path, ease in and out, press ring). Every play bumps a run number; anything left
  // over from an older run does nothing.

  class Timeline {
    constructor() {
      this.run = 0;
      this.now = 0;
      this.last = 0;
      this.jobs = [];
      this.frames = [];
      this.script = null;
      /// The frame loop has stopped: the run ended.
      this.stopped = false;
      this.state = signal("idle");
      /// How fast the demo clock runs against real time. CSS transitions don't follow it.
      this.speed = signal(1);
    }

    play() {
      const run = ++this.run;
      this.now = 0;
      this.last = performance.now();
      this.jobs = [];
      this.frames = [];
      this.stopped = false;
      this.state.set("playing");
      if (this.script) this.script(new Script(this));
      this.tick(run);
    }

    /// Ends the run where it is, for a demo taken off screen.
    stop() {
      this.run++;
      this.jobs = [];
      this.frames = [];
      this.state.set("idle");
    }

    /// Restarts the frame loop for work scheduled after the run ended.
    wake() {
      if (this.stopped && (this.jobs.length || this.frames.length)) {
        this.stopped = false;
        this.last = performance.now();
        this.tick(this.run);
      }
    }

    at(ms, job) {
      let i = 0;
      while (i < this.jobs.length && this.jobs[i][0] <= ms) i++;
      this.jobs.splice(i, 0, [ms, job]);
      this.wake();
    }

    /// Runs `f` after `ms` of demo time from now.
    after(ms, f) {
      this.at(this.now + ms, f);
    }

    /// Calls `f` with the demo clock on every frame until it returns false.
    everyFrame(f) {
      this.frames.push(f);
      this.wake();
    }

    tick(run) {
      requestAnimationFrame(() => {
        if (this.run !== run) return;
        const t = performance.now();
        // A hidden tab stops frames; don't leap ahead when it comes back.
        const dt = Math.min(Math.max(t - this.last, 0), 100);
        this.last = t;
        this.now += dt * this.speed.peek();
        let n = 0;
        while (n < this.jobs.length && this.jobs[n][0] <= this.now) n++;
        const now = this.now;
        for (const [, job] of this.jobs.splice(0, n)) {
          if (this.run !== run) return;
          job();
        }
        if (this.run !== run) return;
        const frames = this.frames;
        this.frames = [];
        this.frames = frames.filter((f) => f(now)).concat(this.frames);
        const busy = this.jobs.length > 0 || this.frames.length > 0;
        this.stopped = !busy;
        if (busy) this.tick(run);
        else this.state.set("done");
      });
    }
  }

  /// How long the cursor gets to reach a target.
  const GLIDE = 750;

  /// Lays out a run's steps on the timeline. `t` is where the next step lands.
  class Script {
    constructor(tl) {
      this.tl = tl;
      this.t = 0;
    }

    wait(ms) {
      this.t += ms;
      return this;
    }

    /// Runs `f` at the current time.
    then(f) {
      this.tl.at(this.t, f);
      return this;
    }

    /// Types `text` a character at a time, `perChar` ms apart, handing `f` what's typed so far.
    typing(text, perChar, f) {
      const chars = Array.from(text);
      for (let i = 1; i <= chars.length; i++) {
        const sofar = chars.slice(0, i).join("");
        this.wait(perChar).then(() => f(sofar));
      }
      return this;
    }

    /// Glides the cursor to `aim`, measured when the move starts.
    moveTo(cur, aim) {
      this.then(() => cur.glide(aim, GLIDE));
      return this.wait(GLIDE);
    }

    /// Shows the press ring. Doesn't move the script's time.
    press(cur) {
      return this.then(() => cur.press());
    }
  }

  function rectOf(aim) {
    const el = aim.el;
    if (!el) return null;
    const r = el.getBoundingClientRect();
    return r.width > 0 && r.height > 0 ? r : null;
  }

  /// The demo's pointer. Lives inside a positioned stage; positions are measured against it.
  class Cursor {
    constructor(tl, stage, home) {
      this.tl = tl;
      this.stage = stage;
      /// Resting spot, as a fraction of the stage.
      this.home = home;
      /// null while resting at home.
      this.pos = signal(null);
      this.down = signal(false);
      this.rest = null;
      this.glideId = 0;
      // Layout changes move the targets; keep the cursor on the one it rests on.
      addEventListener("resize", () => {
        const p = this.rest && this.point(this.rest);
        if (p) this.pos.set(p);
      });
    }

    /// Back to the resting spot, at once.
    reset() {
      this.glideId++;
      this.rest = null;
      this.pos.set(null);
      this.down.set(false);
    }

    press() {
      this.down.set(true);
      this.tl.after(170, () => this.down.set(false));
    }

    point(goal) {
      const stage = this.stage.el;
      if (!stage) return null;
      const a = stage.getBoundingClientRect();
      if (goal === "home") return [a.width * this.home[0], a.height * this.home[1]];
      const r = rectOf(goal);
      return r && [r.left - a.left + r.width * 0.5, r.top - a.top + r.height * 0.55];
    }

    /// A curved, eased glide. The target is re-measured every frame, so it lands true even if
    /// the layout shifts mid-move.
    glide(goal, budget) {
      const start = this.pos.peek() || this.point("home");
      const end = this.point(goal);
      if (!start || !end) return;
      const id = ++this.glideId;
      this.rest = goal === "home" ? null : goal;
      const [sx, sy] = start;
      const [dx, dy] = [end[0] - sx, end[1] - sy];
      const dist = Math.hypot(dx, dy);
      const bend = Math.min(dist * 0.18, 80) * (dx > 0 ? 1 : -1);
      const ctrl = [sx + dx * 0.5 - dy * 0.15 + bend * 0.2, sy + dy * 0.5 + dx * 0.12 - Math.abs(bend) * 0.3];
      const dur = Math.max(Math.min(280 + dist * 1.1, budget), Math.min(380, budget));
      const t0 = this.tl.now;
      let lastEnd = end;
      this.tl.everyFrame((now) => {
        if (this.glideId !== id) return false;
        lastEnd = this.point(goal) || lastEnd;
        const u = Math.min(Math.max((now - t0) / dur, 0), 1);
        const e = u < 0.5 ? 4 * u * u * u : 1 - (-2 * u + 2) ** 3 / 2;
        const [a, b, c] = [(1 - e) * (1 - e), 2 * (1 - e) * e, e * e];
        this.pos.set([a * sx + b * ctrl[0] + c * lastEnd[0], a * sy + b * ctrl[1] + c * lastEnd[1]]);
        if (u < 1) return true;
        if (goal === "home") this.pos.set(null);
        return false;
      });
    }
  }

  /// Draws a `Cursor`. Put it last inside the stage.
  function pointer(cur) {
    const [hx, hy] = cur.home;
    return h(
      "div",
      {
        class: "pointer-events-none absolute left-0 top-0 z-40 will-change-transform",
        style: () => {
          const p = cur.pos.get();
          return p
            ? `transform: translate(${p[0].toFixed(1)}px, ${p[1].toFixed(1)}px);`
            : `left: ${(hx * 100).toFixed(1)}%; top: ${(hy * 100).toFixed(1)}%;`;
        },
        "aria-hidden": "true",
      },
      h("span", {
        class: () =>
          cur.down.get()
            ? "absolute -left-3 -top-3 w-6 h-6 rounded-full bg-gc-accent/40 scale-100 transition-transform duration-150"
            : "absolute -left-3 -top-3 w-6 h-6 rounded-full bg-gc-accent/40 scale-0 transition-transform duration-150",
      }),
      h(
        "svg",
        { viewBox: "0 0 16 24", class: "relative w-4 h-6 drop-shadow-[0_2px_3px_rgba(0,0,0,.5)]" },
        h("path", { d: "M1 1 L1 19 L6 14 L9.5 22 L12.5 20.7 L9 13 L15 13 Z", fill: "white", stroke: "black", "stroke-width": "1.2" }),
      ),
    );
  }

  /// The demo's steps: the current one filled, past ones checked off.
  function stepChips(steps, current) {
    return steps.map((label, i) =>
      h(
        "span",
        {
          "aria-current": () => (current.get() === i ? "step" : null),
          class: () => {
            const on = current.get();
            if (on === i) return "rounded-full border border-gc-text bg-gc-text px-3 py-1 text-[12px] font-semibold text-gc-bg";
            if (on !== null && i < on) return "rounded-full border border-gc-accent/50 px-3 py-1 text-[12px] text-gc-accent";
            return "rounded-full border border-gc-border px-3 py-1 text-[12px] text-gc-text-faint";
          },
        },
        label,
      ),
    );
  }

  const CONTROL =
    "inline-flex items-center gap-1.5 rounded-full border border-gc-border px-4 py-1.5 text-[13px] font-semibold text-gc-text hover:border-gc-accent/40 cursor-pointer focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-gc-accent";

  /// Runs the demo again from the top.
  function replayButton(tl) {
    return h(
      "button",
      { type: "button", class: CONTROL, onclick: () => tl.play() },
      h("span", { class: "material-symbols-outlined text-[16px]" }, "replay"),
      "Replay",
    );
  }

  /// Speed chips: the one picked sets the timeline's `speed`.
  function speedChips(tl, speeds = [1, 2, 4]) {
    const speed = tl.speed;
    return h(
      "div",
      { class: "inline-flex rounded-full border border-gc-border p-0.5", role: "group", "aria-label": "Playback speed" },
      speeds.map((k) => {
        const on = () => Math.abs(speed.get() - k) < 1e-6;
        return h(
          "button",
          {
            type: "button",
            "aria-pressed": () => String(on()),
            class: () =>
              on()
                ? "rounded-full bg-gc-text px-3 py-1 font-mono text-[12.5px] font-semibold text-gc-bg cursor-pointer"
                : "rounded-full px-3 py-1 font-mono text-[12.5px] font-semibold text-gc-text-muted hover:text-gc-text cursor-pointer",
            onclick: () => speed.set(k),
          },
          `${k}×`,
        );
      }),
    );
  }

  /// Whether `win` is far enough on screen for a demo to play.
  function inView(win) {
    const el = win.el;
    if (!el) return false;
    const r = el.getBoundingClientRect();
    return r.top < innerHeight * 0.7 && r.bottom > 0;
  }

  // ---------------------------------------------------------------------------------------
  // The kit every film is made of: the rig (timeline, cursor, window refs and the pane's
  // state), the chat thread, the messages every film uses, cards, the login card and the
  // Touch ID sheet.

  /// Cursor targets the window itself owns. Films number theirs from `LIT_FILM`.
  const LIT_NONE = 0;
  const LIT_PLUGINS = 1;
  const LIT_PLUS = 2;
  const LIT_FILM = 10;

  class Rig {
    constructor() {
      this.tl = new Timeline();
      this.tl.speed.set(2);
      this.win = ref();
      /// Called when a run ends on screen: moves the tour to the next film. Without it the
      /// film plays again.
      this.next = null;
      this.cur = new Cursor(this.tl, this.win, [0.62, 0.95]);
      this.screen = new AgentScreen();
      this.scroller = ref();
      this.composer = ref();
      this.plusBtn = ref();
      this.pluginsBtn = ref();
      this.act_ = signal(null);
      this.caption = signal("");
      this.draft = signal("");
      this.lit = signal(LIT_NONE);
      /// The id of the tool call still running; 0 when none is.
      this.busy = signal(0);
      /// "off", "waiting" or "ok".
      this.touch = signal("off");
      /// What the Touch ID sheet says it fills: [account, site].
      this.touchFor = signal(["", ""]);
      this.plugins = signal([]);
      this.routines = signal([]);
      /// The details pane's tab: "details", "library", "computer" or "connect".
      this.tab = signal("details");
    }

    /// Back to the top of a run, with the plugins and routines the film starts with.
    reset(plugins, routines) {
      this.draft.set("");
      this.lit.set(LIT_NONE);
      this.busy.set(0);
      this.touch.set("off");
      this.plugins.set(plugins);
      this.routines.set(routines);
      this.tab.set("details");
      this.screen.reset();
      this.cur.reset();
    }

    /// The plugins are now `plugins`; the pane shows them under Connect.
    connect(plugins) {
      this.plugins.set(plugins);
      this.tab.set("connect");
    }

    /// Adds a routine; the pane shows it under Details.
    addRoutine(routine) {
      this.routines.set([...this.routines.peek(), routine]);
      this.tab.set("details");
    }

    /// Grows the Computer screen out of the Computer tab's thumbnail.
    openScreen() {
      this.tab.set("computer");
      this.screen.expand();
    }

    act(n, caption) {
      this.act_.set(n);
      this.caption.set(caption);
    }

    /// `base`, ringed while the cursor rests on `id`.
    litClass(id, base) {
      return () => (this.lit.get() === id ? `${base} ring-2 ring-[#14C370]/70 ring-offset-1 ring-offset-transparent` : base);
    }

    /// Rest on `aim` with `id` lit, press, then `f`.
    tap(s, aim, id, f) {
      s.moveTo(this.cur, aim)
        .then(() => this.lit.set(id))
        .wait(220)
        .press(this.cur)
        .wait(170)
        .then(() => {
          this.lit.set(LIT_NONE);
          f();
        })
        .wait(320);
    }

    /// Type `text` into the composer, then `send` (which pushes the bubble).
    say(s, text, send) {
      s.moveTo(this.cur, this.composer)
        .wait(150)
        .typing(text, 28, (t) => this.draft.set(t))
        .wait(300)
        .then(() => {
          this.draft.set("");
          send();
        });
    }

    /// Touch ID: the sheet comes up, reads a finger, and goes.
    touchId(s, account, site) {
      s.then(() => {
        this.touchFor.set([account, site]);
        this.touch.set("waiting");
      })
        .wait(1300)
        .then(() => this.touch.set("ok"))
        .wait(600)
        .then(() => this.touch.set("off"))
        .wait(300);
    }

    /// The end of a run: hold the last frame, then go on to the next film if still on
    /// screen (or play again when this film stands alone).
    end(s, caption) {
      s.then(() => this.caption.set(caption))
        .wait(4500)
        .then(() => {
          if (!inView(this.win)) return;
          if (this.next) this.next();
          else this.tl.play();
        });
    }

    /// Plays from the top if the window is on screen and not already playing.
    start() {
      const st = this.tl.state.peek();
      if ((st === "idle" || st === "done") && inView(this.win)) this.tl.play();
    }

    /// Starts the first time the window scrolls into view, and again after a run that ended
    /// off screen.
    autoplay() {
      setTimeout(() => this.start(), 300);
      addEventListener("scroll", () => this.start(), { passive: true });
    }
  }

  /// A film's chat thread: its messages are drawn into `box`.
  class Chat {
    constructor(rig) {
      this.scroller = rig.scroller;
      this.next = 0;
      this.roots = [];
      this.box = h("div", { class: "flex flex-col gap-2.5 px-4 pb-4 pt-14 text-[13.5px] md:px-8" });
    }

    clear() {
      for (const r of this.roots) r.dispose();
      this.roots = [];
      this.box.textContent = "";
    }

    /// Draws `render(id)` at the end of the thread and scrolls down; returns its id.
    push(render) {
      const id = ++this.next;
      const r = root(() => render(id));
      this.roots.push(r);
      this.box.append(r.value);
      const scroller = this.scroller;
      // Twice: cards that slide in settle a frame later.
      const down = () => {
        if (scroller.el) scroller.el.scrollTop = scroller.el.scrollHeight;
      };
      requestAnimationFrame(down);
      setTimeout(down, 320);
      return id;
    }
  }

  const BUBBLE_ME = "gc-card-in gs-user max-w-[78%] self-end rounded-[18px] px-3.5 py-2 leading-snug";
  const BUBBLE_AI = "gc-card-in gs-bubble gs-line max-w-[80%] self-start rounded-[16px] border px-3.5 py-2.5 leading-snug";

  /// Your message.
  const me = (text) => () => h("div", { class: BUBBLE_ME }, text);
  /// The co-worker's reply.
  const ai = (text) => () => h("div", { class: BUBBLE_AI }, text);

  /// Runs a tool call: the line spins for `ms`, then checks off.
  function tool(s, rig, chat, plugin, call, ms) {
    s.then(() => {
      const id = chat.push((id) =>
        h(
          "div",
          { class: "gc-card-in gs-f flex items-center gap-1.5 self-start text-[11.5px]" },
          () =>
            rig.busy.get() === id
              ? h("span", { class: "h-3 w-3 animate-spin rounded-full border-[1.5px] border-current border-t-transparent" })
              : h("span", { class: "material-symbols-outlined text-[14px] text-[#14C370]" }, "check_circle"),
          pluginMark(plugin),
          h("span", { class: "font-mono" }, `${plugin} · ${call}`),
        ),
      );
      rig.busy.set(id);
    })
      .wait(ms)
      .then(() => rig.busy.set(0))
      .wait(200);
  }

  /// A spreadsheet's tile.
  function fileTile(name, detail, extra) {
    return h(
      "div",
      { class: `gs-card flex items-center gap-2.5 rounded-xl px-2.5 py-2 ${extra}` },
      h("span", { class: "grid h-9 w-9 place-items-center rounded-lg bg-[#1d6f42] text-[9px] font-black text-white" }, "XLSX"),
      h(
        "span",
        { class: "flex flex-col leading-tight" },
        h("span", { class: "text-[12.5px] font-semibold" }, name),
        h("span", { class: "gs-f text-[11px]" }, detail),
      ),
    );
  }

  /// A plugin's mark.
  function pluginMark(plugin) {
    const [letter, tone] =
      {
        gmail: ["M", "bg-[#ea4335]"],
        Gmail: ["M", "bg-[#ea4335]"],
        browser: ["◎", "bg-[#3b82f6]"],
        routine: ["↻", "bg-[#8b5cf6]"],
        plugins: ["+", "bg-[#71717a]"],
      }[plugin] || ["B", "bg-[#0f766e]"];
    return h(
      "span",
      { class: `grid h-4 w-4 shrink-0 place-items-center rounded-[4px] ${tone} text-[9px] font-black text-white` },
      letter,
    );
  }

  /// A generated card in the thread: a header (icon, title, tag) over `body`.
  function card(icon, title, tag, ...body) {
    return h(
      "div",
      { class: "gc-card-in gs-card w-[min(440px,100%)] self-start overflow-hidden rounded-[14px] text-[12.5px]" },
      h(
        "div",
        { class: "gs-line flex items-center gap-2 border-b px-3 py-2" },
        h("span", { class: "material-symbols-outlined text-[16px] text-[#14C370]" }, icon),
        h("span", { class: "font-semibold" }, title),
        tag && h("span", { class: "gs-hover gs-d ml-auto rounded-full px-2 py-0.5 text-[10.5px]" }, tag),
      ),
      h("div", { class: "px-3 py-2.5" }, body),
    );
  }

  const BTN = "gs-line inline-flex items-center gap-1 rounded-full border px-3 py-1 text-[12px] font-semibold";
  const BTN_PRIMARY = "gs-btn inline-flex items-center gap-1 rounded-full px-3.5 py-1 text-[12px] font-semibold";

  /// Rows of label / value.
  function rows(items) {
    return h(
      "div",
      { class: "grid grid-cols-[auto_1fr] gap-x-4 gap-y-1" },
      items.map(([k, v]) => [h("span", { class: "gs-d" }, k), h("span", { class: "text-right tabular-nums" }, v)]),
    );
  }

  /// The login card (`request_user_form`): Email and Password, filled from a saved login.
  /// `state`: 0 empty, 1 saved logins showing, 2 filled and locked, 3 logged in.
  function loginCard(rig, plugin, title, site, account, state, [field, saved, login], [litField, litSaved, litLogin]) {
    const fieldCls = (locked) =>
      locked
        ? "gs-line gs-hover flex h-8 items-center rounded-lg border px-2.5 text-[12.5px]"
        : "gs-line flex h-8 items-center rounded-lg border px-2.5 text-[12.5px]";
    const done = memo(() => state.get() === 3);
    const picking = memo(() => state.get() === 1);
    const locked = memo(() => state.get() >= 2);
    return h(
      "div",
      { class: "gc-card-in gs-card relative w-[min(380px,100%)] self-start rounded-[14px] p-3.5 text-[12.5px]" },
      h(
        "div",
        { class: "flex items-center gap-2 font-semibold" },
        pluginMark(plugin),
        title,
        h("span", { class: "gs-f ml-auto text-[11px] font-normal" }, site),
      ),
      () =>
        done.get()
          ? h(
              "div",
              { class: "mt-2 flex items-center gap-1.5 text-[#14C370]" },
              h("span", { class: "material-symbols-outlined text-[16px]" }, "check_circle"),
              `Logged in as ${account}`,
            )
          : [
              h("div", { class: "gs-d mb-1 mt-2.5 text-[11px]" }, "Email"),
              h(
                "div",
                {
                  ref: field,
                  class: () => {
                    const base = fieldCls(locked.get());
                    return rig.lit.get() === litField ? `${base} ring-2 ring-[#14C370]/70` : base;
                  },
                },
                () => (locked.get() ? h("span", null, account) : h("span", { class: "gs-f" }, "Choose a saved login")),
              ),
              h("div", { class: "gs-d mb-1 mt-2 text-[11px]" }, "Password"),
              h(
                "div",
                { class: () => fieldCls(locked.get()) },
                () =>
                  locked.get()
                    ? h("span", { class: "tracking-[2px]" }, "••••••••••••")
                    : h("span", { class: "gs-f" }, "Filled by Touch ID"),
              ),
              h(
                "div",
                { class: "mt-3 flex items-center gap-2" },
                h("span", { class: "gs-f text-[11px]" }, "The password never enters the chat."),
                h("div", { ref: login, class: rig.litClass(litLogin, "gs-btn ml-auto rounded-full px-3.5 py-1 text-[12px] font-semibold") }, "Log in"),
              ),
              () =>
                picking.get() &&
                h(
                  "div",
                  { class: "gc-card-in gs-card absolute left-3.5 right-3.5 top-[84px] z-10 rounded-xl p-1.5 shadow-xl" },
                  h(
                    "div",
                    { ref: saved, class: rig.litClass(litSaved, "gs-hover flex items-center gap-2 rounded-lg px-2 py-1.5") },
                    h("span", { class: "material-symbols-outlined text-[16px] text-[#14C370]" }, "key"),
                    h("span", { class: "flex-1" }, account),
                    h("span", { class: "gs-f text-[11px]" }, "Passwords"),
                  ),
                  h("div", { class: "gs-f px-2 pt-1 text-[10.5px]" }, "Choose an account. Touch ID fills in its password."),
                ),
            ],
    );
  }

  /// The macOS Touch ID sheet over the window.
  function touchSheet(rig) {
    const shown = memo(() => rig.touch.get() !== "off");
    const ok = () => rig.touch.get() === "ok";
    return () =>
      shown.get() &&
      h(
        "div",
        { class: "absolute inset-0 z-[35] grid place-items-center bg-black/35" },
        h(
          "div",
          { class: "gc-card-in w-[min(290px,86%)] rounded-2xl bg-[#f5f5f7] p-5 text-center text-[#16161c] shadow-2xl" },
          avatar("mx-auto mb-2 h-12 w-12"),
          h("div", { class: "text-[13px] font-bold" }, "Buwiz wants to use a saved password."),
          h("div", { class: "mt-1 text-[11px] text-[#5e6278]" }, () => {
            const [a, s] = rig.touchFor.get();
            return `Touch ID to fill ${a} for ${s}.`;
          }),
          h(
            "div",
            { class: "mx-auto my-3 h-14 w-14" },
            h(
              "svg",
              {
                viewBox: "0 0 64 64",
                fill: "none",
                "stroke-width": "2.6",
                "stroke-linecap": "round",
                class: () => (ok() ? "h-14 w-14 stroke-[#059669] transition-colors" : "h-14 w-14 stroke-[#e5484d] transition-colors"),
              },
              [
                "M20 26a12 12 0 0 1 24 0v6",
                "M14 30a18 18 0 0 1 36-4",
                "M26 28a6 6 0 0 1 12 0v8c0 6-2 10-5 14",
                "M32 30v8c0 5-2 9-5 12",
                "M20 34c0 7-1 11-3 14",
                "M44 36c0 6-1 10-3 14",
              ].map((d) => h("path", { d })),
            ),
          ),
          h("div", { class: "text-[11px] text-[#5e6278]" }, () => (ok() ? "Done" : "Touch ID")),
        ),
      );
  }

  /// An approval card: what the co-worker wants to do, then Deny and the approve button.
  /// `settled()` is what to show once it is approved, or null before.
  function approveCard(rig, icon, title, body, approve, approveRef, approveLit, settled) {
    return h(
      "div",
      { class: "gc-card-in gs-card w-[min(440px,100%)] self-start rounded-[14px] p-3.5 text-[12.5px]" },
      h(
        "div",
        { class: "flex items-center gap-2 font-semibold" },
        h("span", { class: "material-symbols-outlined text-[18px] text-[#f59e0b]" }, icon),
        title,
      ),
      h("div", { class: "mt-2" }, body),
      () => {
        const done = settled();
        return done
          ? h(
              "div",
              { class: "mt-3 flex items-center gap-1.5 font-medium text-[#14C370]" },
              h("span", { class: "material-symbols-outlined text-[16px]" }, "check_circle"),
              done,
            )
          : h(
              "div",
              { class: "mt-3 flex justify-end gap-2" },
              h("span", { class: BTN }, "Deny"),
              h("div", { ref: approveRef, class: rig.litClass(approveLit, BTN_PRIMARY) }, approve),
            );
      },
    );
  }

  /// A checklist row: on (check) or off (dash).
  function checkRow(on, label) {
    return h(
      "div",
      { class: on ? "flex items-center gap-2 py-0.5" : "gs-f flex items-center gap-2 py-0.5" },
      h(
        "span",
        { class: on ? "material-symbols-outlined text-[15px] text-[#14C370]" : "material-symbols-outlined gs-f text-[15px]" },
        on ? "check_box" : "check_box_outline_blank",
      ),
      label,
    );
  }

  // ---------------------------------------------------------------------------------------
  // The app window the demos share, drawn after Buwiz's AI Co-worker app: a sidebar (the
  // co-worker's tile, Chats, your avatar and Plugins), the chat (the co-worker's name chip, the
  // thread, the composer pill) and the details pane (avatar, model, and tabs: Routines under
  // Details, the Computer thumbnail under Computer, plugins under Connect).
  //
  // The Computer screen can grow out of the pane to fill the window and shrink back; it shows
  // whatever the co-worker has open (a browser, a spreadsheet, a PDF).
  //
  // Up to 1080px wide and 640px tall. Below `lg` the sidebar hides; below `md` the pane hides.

  /// The Computer screen is laid out at this size, then scaled into the pane's thumbnail.
  const VW = 960;
  const VH = 600;
  const GROW_MS = 380;
  const SHRINK_MS = 340;

  const twoFrames = (f) => requestAnimationFrame(() => requestAnimationFrame(f));

  /// The Computer screen: the pane's thumbnail and the full-window copy that grows out of it.
  /// With reduced motion it crossfades instead.
  class AgentScreen {
    constructor() {
      /// "closed", "open" or "closing".
      this.zoom = signal("closed");
      this.style = signal("");
      this.run = 0;
      this.reduced = matchMedia("(prefers-reduced-motion: reduce)").matches;
      this.big = ref();
      this.thumb = ref();
    }

    reset() {
      this.run++;
      this.zoom.set("closed");
      this.style.set("");
    }

    /// The transform that puts the full-window screen exactly over the thumbnail.
    overThumb() {
      if (!this.big.el) return "";
      const o = this.big.el.getBoundingClientRect();
      const t = this.thumb.el && this.thumb.el.getBoundingClientRect();
      if (!t || t.width <= 0 || o.width <= 0 || o.height <= 0) return "";
      return (
        `transform:translate(${(t.left - o.left).toFixed(1)}px,${(t.top - o.top).toFixed(1)}px) ` +
        `scale(${(t.width / o.width).toFixed(4)},${(t.height / o.height).toFixed(4)});`
      );
    }

    expand() {
      const run = ++this.run;
      const [from, to] = this.reduced
        ? ["opacity:0;transition:none;", "opacity:1;transition:opacity 220ms ease-out;"]
        : [
            `${this.overThumb()}opacity:.55;transition:none;`,
            `transform:none;opacity:1;transition:transform ${GROW_MS}ms cubic-bezier(.2,.8,.2,1),opacity 160ms ease-out;`,
          ];
      this.style.set(from);
      this.zoom.set("open");
      twoFrames(() => {
        if (this.run === run) this.style.set(to);
      });
    }

    shrink() {
      const run = ++this.run;
      const to = this.reduced
        ? "opacity:0;transition:opacity 200ms ease-in;"
        : `${this.overThumb()}opacity:.4;transition:transform ${SHRINK_MS}ms cubic-bezier(.4,0,.2,1),opacity ${SHRINK_MS}ms ease-in;`;
      this.zoom.set("closing");
      this.style.set(to);
      setTimeout(() => {
        if (this.run === run) {
          this.zoom.set("closed");
          this.style.set("");
        }
      }, SHRINK_MS + 30);
    }
  }

  /// Grok Bot's star mark at rest (grok-shell's baked rest pose): the body, then its two eyes
  /// cut out of it.
  const MASCOT =
    "M127 59L131 63L136 67L142 69L148 68L154 65L160 62L165 59L172 58L178 60L183 64L186 69L187 76L186 82L184 88L182 94L181 100L184 106L188 111L193 114L199 118L204 121L210 124L214 129L217 134L218 140L217 147L214 152L210 157L205 162L200 165L195 168L188 169L182 169L179 173L180 179L181 186L180 192L177 198L173 203L169 207L164 212L159 215L153 215L146 213L141 211L136 206L132 201L128 196L124 192L119 188L113 186L107 187L101 190L95 193L90 196L83 197L77 195L72 191L69 186L68 179L69 173L71 167L73 161L74 155L71 149L67 144L62 141L56 137L51 134L45 131L41 126L38 121L37 115L38 108L41 103L45 98L50 93L55 90L61 87L67 86L73 86L76 82L75 76L74 69L75 63L78 57L82 52L86 48L91 43L96 40L102 40L109 42L114 44L119 49L123 54ZM127 114L130 117L132 121L134 125L136 129L138 134L139 138L140 142L137 146L132 146L128 145L125 141L123 137L121 133L119 129L117 125L115 120L115 116L118 113L122 112ZM160 105L163 108L165 112L167 116L169 120L171 125L173 129L173 133L170 137L165 137L161 136L158 132L156 128L154 124L152 120L150 116L148 111L148 107L151 104L156 103Z";

  /// The co-worker's avatar: its Grok Bot mark in its green.
  function avatar(size) {
    return h(
      "svg",
      { viewBox: "30 30 196 196", "aria-hidden": "true", class: `block shrink-0 ${size}` },
      h("path", { fill: "#14C370", "fill-rule": "evenodd", stroke: "#14C370", "stroke-width": "3", "stroke-linejoin": "round", d: MASCOT }),
    );
  }

  function thumb(desktop, r, tab) {
    const k = signal(0);
    const measure = () => {
      const w = r.el ? r.el.getBoundingClientRect().width : 0;
      if (w > 0) k.set(w / VW);
    };
    effect(() => {
      tab.get();
      measure();
      requestAnimationFrame(measure);
    });
    addEventListener("resize", measure);
    return h(
      "div",
      { ref: r, class: "gs-line relative aspect-[16/10] w-full overflow-hidden rounded-xl border" },
      h(
        "div",
        {
          class: "pointer-events-none absolute left-0 top-0 origin-top-left",
          style: () => `width:${VW}px;height:${VH}px;transform:scale(${k.get().toFixed(4)})`,
        },
        desktop(false),
      ),
    );
  }

  const PANE_LABEL = "gs-f mb-1.5 mt-4 w-full text-[11px] font-semibold";

  const pluginBlurb = (plugin) => (plugin === "Gmail" ? "Reads BIR's confirmation emails" : "Profiles, forms and filing");

  const PANE_TABS = [
    ["details", "Details"],
    ["library", "Library"],
    ["computer", "Computer"],
    ["connect", "Connect"],
  ];

  /// The app window. `thread` is the chat; `desktop(big)` draws the Computer screen, `big` for
  /// the full-window copy (only that copy should attach refs the cursor aims at).
  function shellWindow(rig, desktop, chats, thread) {
    const zoom = rig.screen.zoom;
    const onTab = (t) => () => (rig.tab.get() === t ? "w-full" : "hidden");
    return h(
      "div",
      {
        ref: rig.win,
        class:
          "gs-win gs-chat gs-t relative z-[1] mx-auto flex h-[640px] w-full max-w-[1080px] select-none overflow-hidden rounded-[18px] text-left text-[13px] leading-snug",
      },
      h(
        "div",
        { class: "gs-side gs-line hidden w-[216px] shrink-0 flex-col border-r px-3 pb-3 pt-3 lg:flex" },
        h(
          "div",
          { class: "flex items-center gap-1.5" },
          h("span", { class: "h-3 w-3 rounded-full bg-[#ff5f57]" }),
          h("span", { class: "h-3 w-3 rounded-full bg-[#febc2e]" }),
          h("span", { class: "h-3 w-3 rounded-full bg-[#28c840]" }),
          h(
            "span",
            { class: "gs-hover gs-d ml-auto grid h-7 w-7 place-items-center rounded-full" },
            h("span", { class: "material-symbols-outlined text-[16px]" }, "view_sidebar"),
          ),
          h(
            "span",
            { class: "gs-hover gs-d grid h-7 w-7 place-items-center rounded-full" },
            h("span", { class: "material-symbols-outlined text-[16px]" }, "add"),
          ),
        ),
        h(
          "div",
          { class: "gs-hover mx-auto mt-4 flex w-[96px] flex-col items-center gap-1 rounded-xl py-2.5" },
          avatar("h-12 w-12"),
          h("span", { class: "text-[11px] font-medium" }, "Buwiz"),
        ),
        h("div", { class: "gs-f mb-1.5 mt-4 px-1 text-[11px] font-semibold" }, "Chats"),
        chats.map((title, i) =>
          h(
            "div",
            { class: i === 0 ? "gs-hover flex items-center gap-2 rounded-lg px-1.5 py-1.5" : "flex items-center gap-2 rounded-lg px-1.5 py-1.5" },
            h(
              "span",
              { class: "grid h-6 w-6 shrink-0 place-items-center rounded-full bg-[#14C370] text-[10px] font-bold text-white" },
              title[0],
            ),
            h("span", { class: "min-w-0 truncate text-[12px]" }, title),
          ),
        ),
        h(
          "div",
          { class: "mt-auto flex items-center gap-1.5" },
          h(
            "span",
            { class: "grid aspect-square h-7 w-7 shrink-0 place-items-center rounded-full bg-[#1884FC] text-[10.5px] font-semibold text-white" },
            "JD",
          ),
          h(
            "div",
            {
              ref: rig.pluginsBtn,
              class: () =>
                rig.lit.get() === LIT_PLUGINS
                  ? "gs-hover flex h-7 items-center gap-1 whitespace-nowrap rounded-full px-2.5 text-[11.5px] font-medium ring-2 ring-[#14C370]/70"
                  : "gs-hover flex h-7 items-center gap-1 whitespace-nowrap rounded-full px-2.5 text-[11.5px] font-medium",
            },
            h("span", { class: "material-symbols-outlined gs-d text-[14px]" }, "extension"),
            "Plugins",
          ),
        ),
      ),

      h(
        "div",
        { class: "relative flex min-w-0 flex-1 flex-col" },
        h("div", { class: "gs-fade pointer-events-none absolute inset-x-0 top-0 z-[4] h-14" }),
        h(
          "div",
          {
            class:
              "gs-card absolute left-1/2 top-2.5 z-[5] flex -translate-x-1/2 items-center gap-1.5 whitespace-nowrap rounded-full py-1 pl-1.5 pr-3 text-[12.5px] font-semibold shadow-[0_2px_10px_rgba(0,0,0,.12)]",
          },
          avatar("h-6 w-6"),
          "Buwiz",
          h("span", { class: "material-symbols-outlined gs-d -ml-0.5 text-[16px]" }, "expand_more"),
        ),
        h("div", { ref: rig.scroller, class: "min-h-0 flex-1 overflow-y-auto" }, thread),
        h(
          "div",
          { class: "shrink-0 px-4 pb-4 pt-1 md:px-8" },
          h(
            "div",
            { class: "gs-bubble gs-line flex min-h-11 items-center gap-2 rounded-[22px] border py-1.5 pl-1.5 pr-1.5" },
            h(
              "div",
              {
                ref: rig.plusBtn,
                class: () =>
                  rig.lit.get() === LIT_PLUS
                    ? "gs-hover grid h-8 w-8 shrink-0 place-items-center rounded-full ring-2 ring-[#14C370]/70"
                    : "gs-hover grid h-8 w-8 shrink-0 place-items-center rounded-full",
              },
              h("span", { class: "material-symbols-outlined gs-d text-[18px]" }, "add"),
            ),
            h("div", { ref: rig.composer, class: "min-w-0 flex-1 truncate text-[13.5px]" }, () => {
              const t = rig.draft.get();
              return t ? h("span", null, t, h("span", { class: "gc-caret" })) : h("span", { class: "gs-f" }, "Message Buwiz");
            }),
            h(
              "span",
              { class: "gs-hover gs-d hidden h-7 items-center gap-0.5 rounded-full px-2.5 text-[11.5px] sm:flex" },
              "Auto",
              h("span", { class: "material-symbols-outlined text-[14px]" }, "expand_more"),
            ),
            h("span", { class: "material-symbols-outlined gs-d text-[18px]" }, "mic"),
            h(
              "span",
              { class: "grid h-8 w-8 shrink-0 place-items-center rounded-full bg-[#14C370] text-white" },
              h("span", { class: "material-symbols-outlined text-[18px]" }, "graphic_eq"),
            ),
          ),
        ),
      ),

      h(
        "aside",
        { class: "gs-pane gs-line relative hidden w-[264px] shrink-0 flex-col items-center overflow-hidden border-l px-4 pt-6 md:flex" },
        avatar("h-16 w-16"),
        h("div", { class: "mt-2 text-[14px] font-semibold" }, "Buwiz"),
        h("div", { class: "gs-f text-[11px]" }, "Your personal tax agent"),
        h(
          "span",
          { class: "gs-hover gs-d mt-2 flex items-center gap-0.5 rounded-full px-2.5 py-1 text-[11px]" },
          "Auto",
          h("span", { class: "material-symbols-outlined text-[14px]" }, "expand_more"),
        ),
        h(
          "div",
          { class: "mt-3 flex items-center text-[11px]" },
          PANE_TABS.map(([t, label]) =>
            h(
              "span",
              {
                class: () =>
                  rig.tab.get() === t
                    ? "gs-hover whitespace-nowrap rounded-full px-2 py-1 font-medium transition-colors"
                    : "gs-d whitespace-nowrap rounded-full px-2 py-1 transition-colors",
              },
              label,
            ),
          ),
        ),
        h("div", { class: onTab("computer") }, h("div", { class: PANE_LABEL }, "Computer"), thumb(desktop, rig.screen.thumb, rig.tab)),
        h(
          "div",
          { class: onTab("library") },
          h("div", { class: PANE_LABEL }, "Library"),
          h("div", { class: "gs-hover gs-d w-full rounded-xl px-3 py-2.5 text-[11.5px] leading-snug" }, "Files Buwiz makes show up here."),
        ),
        h("div", { class: onTab("connect") }, h("div", { class: PANE_LABEL }, "Connected apps"), () => {
          const on = rig.plugins.get();
          if (!on.length) {
            return h(
              "div",
              { class: "gs-hover gs-d w-full rounded-xl px-3 py-2.5 text-[11.5px] leading-snug" },
              "No apps connected yet. Ask Buwiz to install one.",
            );
          }
          return h(
            "div",
            { class: "gs-card w-full overflow-hidden rounded-xl" },
            on.map((p, i) =>
              h(
                "div",
                { class: i === 0 ? "gc-card-in flex items-center gap-2 px-3 py-2.5" : "gs-line gc-card-in flex items-center gap-2 border-t px-3 py-2.5" },
                pluginMark(p),
                h(
                  "span",
                  { class: "flex min-w-0 flex-1 flex-col leading-tight" },
                  h("span", { class: "text-[12px] font-semibold" }, p),
                  h("span", { class: "gs-f text-[10.5px]" }, pluginBlurb(p)),
                ),
                h(
                  "span",
                  { class: "relative h-[18px] w-[30px] shrink-0 rounded-full bg-[#14C370]" },
                  h("span", { class: "absolute right-[2px] top-[2px] h-[14px] w-[14px] rounded-full bg-white shadow" }),
                ),
              ),
            ),
          );
        }),
        h("div", { class: onTab("details") }, h("div", { class: PANE_LABEL }, "Routines"), () => {
          const list = rig.routines.get();
          if (!list.length) {
            return h(
              "div",
              { class: "gs-hover gs-d w-full rounded-xl px-3 py-2.5 text-[11.5px] leading-snug" },
              "No routines yet. Ask Buwiz in chat to run something on a schedule.",
            );
          }
          return list.map((r) =>
            h(
              "div",
              { class: "gs-card gc-card-in mb-1.5 w-full rounded-xl px-3 py-2" },
              h(
                "div",
                { class: "flex items-center gap-1.5 text-[12px] font-semibold" },
                h("span", { class: "material-symbols-outlined text-[15px] text-[#14C370]" }, "schedule"),
                r.name,
              ),
              h("div", { class: "gs-d mt-0.5 text-[11px]" }, r.when),
              h("div", { class: "gs-f mt-0.5 text-[11px]" }, () => r.note.get()),
            ),
          );
        }),
      ),

      h("div", {
        class: () =>
          zoom.get() === "open"
            ? "pointer-events-none absolute inset-0 z-20 bg-black/45 opacity-100 transition-opacity duration-300"
            : "pointer-events-none absolute inset-0 z-20 bg-black/45 opacity-0 transition-opacity duration-300",
      }),
      h(
        "div",
        {
          ref: rig.screen.big,
          class: () =>
            zoom.get() === "closed"
              ? "invisible absolute inset-1.5 z-30 origin-top-left overflow-hidden rounded-2xl md:inset-3"
              : "absolute inset-1.5 z-30 origin-top-left overflow-hidden rounded-2xl shadow-[0_24px_80px_rgba(0,0,0,.55)] will-change-transform md:inset-3",
          style: () => rig.screen.style.get(),
          "aria-hidden": () => String(zoom.get() === "closed"),
        },
        desktop(true),
      ),

      touchSheet(rig),
      pointer(rig.cur),
    );
  }

  /// An idle Computer: nothing open.
  function idleScreen() {
    return h(
      "div",
      { class: "grid h-full w-full place-items-center bg-[#0b0b0c] text-white/40" },
      h(
        "div",
        { class: "flex flex-col items-center gap-2" },
        h("span", { class: "material-symbols-outlined text-[48px]" }, "computer"),
        h("span", { class: "text-[15px]" }, "Computer is idle"),
      ),
    );
  }

  const idleDesktop = () => idleScreen();

  /// A file open in the viewer: a title bar with the file's name and actions, then the file.
  function viewer(kind, name, actions, body) {
    const [badge, tone] = kind === "xlsx" ? ["XLSX", "bg-[#1d6f42]"] : ["PDF", "bg-[#d93025]"];
    return h(
      "div",
      { class: "flex h-full w-full flex-col bg-[#2b2b2e] text-[#f5f5f5]" },
      h(
        "div",
        { class: "flex h-11 shrink-0 items-center gap-2.5 border-b border-white/10 bg-[#1c1c1e] px-4" },
        h("span", { class: `grid h-6 w-8 place-items-center rounded ${tone} text-[9px] font-black text-white` }, badge),
        h("span", { class: "text-[13px] font-semibold" }, name),
        h("span", { class: "ml-auto flex items-center gap-2" }, actions),
      ),
      h("div", { class: "relative min-h-0 flex-1 overflow-hidden" }, body),
    );
  }

  /// A document Buwiz printed (`/assets/demo/`, written by bir-print's `filled_document`), shown
  /// the way the print preview lays it out: 612pt pages at 70%, or smaller when the screen is
  /// narrower than that. `anchor` names the page to show first. It is scrolled to inside the
  /// frame: a `#fragment` would scroll the home page too.
  function docFrame(src, anchor) {
    const k = signal(0.7);
    const toAnchor = (e) => {
      const frame = e.target;
      const page = frame.contentDocument && frame.contentDocument.getElementById(anchor);
      if (page) frame.contentWindow.scrollTo(0, page.getBoundingClientRect().top);
    };
    const box = h(
      "div",
      { class: "absolute inset-0 overflow-hidden bg-[#525659]" },
      h(
        "div",
        {
          class: "absolute left-1/2 top-4 w-[816px] origin-top -translate-x-1/2 bg-white shadow-xl",
          style: () => `scale:${k.get()};height:calc((100% - 16px) / ${k.get()})`,
        },
        h("iframe", {
          src,
          title: "Buwiz print preview",
          tabindex: "-1",
          class: "pointer-events-none block h-full w-full border-0 bg-white",
          onload: anchor ? toAnchor : null,
        }),
      ),
    );
    new ResizeObserver(([entry]) => {
      const w = entry.contentRect.width;
      if (w > 0) k.set(Math.round(Math.min(0.7, (w - 16) / 816) * 1000) / 1000);
    }).observe(box);
    return box;
  }

  /// A browser on the co-worker's computer.
  function browser(tab, host, path, body) {
    return h(
      "div",
      { class: "flex h-full w-full flex-col bg-white text-[#202124]" },
      h(
        "div",
        { class: "flex h-9 shrink-0 items-end gap-1 bg-[#dee1e6] pl-3" },
        h(
          "span",
          { class: "flex max-w-[260px] items-center gap-1.5 rounded-t-lg bg-white px-3 py-1.5 text-[12px]" },
          h("span", { class: "material-symbols-outlined text-[14px] text-[#5f6368]" }, "public"),
          h("span", { class: "truncate" }, tab),
        ),
      ),
      h(
        "div",
        { class: "flex h-10 shrink-0 items-center gap-2 border-b border-[#e5e7eb] px-3 text-[#5f6368]" },
        h("span", { class: "material-symbols-outlined text-[18px]" }, "arrow_back"),
        h("span", { class: "material-symbols-outlined text-[18px]" }, "refresh"),
        h(
          "span",
          { class: "flex min-w-0 flex-1 items-center gap-1.5 rounded-full bg-[#f1f3f4] px-3 py-1 text-[13px]" },
          h("span", { class: "material-symbols-outlined text-[14px]" }, "lock"),
          h("span", { class: "truncate" }, h("span", { class: "text-[#202124]" }, host), path),
        ),
      ),
      h("div", { class: "relative min-h-0 flex-1 overflow-hidden" }, body),
    );
  }

  /// A film's steps, speed and replay controls and live caption, over the window.
  function filmBar(rig, steps) {
    return h(
      "div",
      { class: "mx-auto mb-4 flex w-full max-w-[1080px] flex-wrap items-center gap-2" },
      stepChips(steps, rig.act_),
      h("div", { class: "ml-auto flex items-center gap-2" }, speedChips(rig.tl), replayButton(rig.tl)),
      h("p", { class: "min-h-5 basis-full pt-1 text-left text-[13px] text-gc-text-muted", "aria-live": "polite" }, () => rig.caption.get()),
    );
  }

  /// A film's bar and window, set to play when it scrolls into view.
  function film(rig, steps, desktop, chats, chat) {
    rig.autoplay();
    return { rig, nodes: [filmBar(rig, steps), shellWindow(rig, desktop, chats, chat.box)] };
  }

  // ---------------------------------------------------------------------------------------
  // Film 1, Connect: install the Buwiz plugin from the chat, "Log in with Buwiz", a login card
  // filled from Passwords with Touch ID, then the permission prompt (Publish to BIR is left
  // off).

  function connectFilm() {
    const STEPS = ["1 · Install the plugin", "2 · Log in with Buwiz", "3 · Touch ID", "4 · Allow access"];
    const ACCOUNT = "juan@delacruz.ph";
    const [LIT_FIELD, LIT_SAVED, LIT_LOGIN, LIT_ALLOW] = [LIT_FILM, LIT_FILM + 1, LIT_FILM + 2, LIT_FILM + 3];
    const r = new Rig();
    const c = new Chat(r);
    const f = { login: signal(0), allowed: signal(false), field: ref(), saved: ref(), loginBtn: ref(), allowBtn: ref() };

    const installed = () =>
      card(
        "extension",
        "Buwiz installed",
        "Plugin",
        h("p", { class: "gs-d" }, "Tax profiles, deadlines, BIR forms and filings, as tools your co-worker can use."),
      );
    const login = () =>
      loginCard(r, "buwiz", "Log in to Buwiz", "account.buwiz.com", ACCOUNT, f.login, [f.field, f.saved, f.loginBtn], [LIT_FIELD, LIT_SAVED, LIT_LOGIN]);
    const allow = () =>
      approveCard(
        r,
        "shield_person",
        "Allow Buwiz to work in the Buwiz app on this computer?",
        [
          h("div", { class: "gs-d mb-1 text-[11px]" }, "Profiles"),
          h("div", { class: "mb-2" }, "JUAN DELA CRUZ · ***-***-788 · all 4 branches"),
          h("div", { class: "gs-d mb-1 text-[11px]" }, "What it may do"),
          h(
            "div",
            { class: "grid grid-cols-1 sm:grid-cols-2 sm:gap-x-3" },
            checkRow(true, "See taxpayer details"),
            checkRow(true, "See tax years and their forms"),
            checkRow(true, "Add tax years, choose forms"),
            checkRow(true, "See drafts"),
            checkRow(true, "Save a new draft"),
            checkRow(true, "Update an existing draft"),
            checkRow(true, "Mark a draft reviewed"),
            checkRow(true, "See filing history and BIR confirmations"),
            checkRow(false, "Edit taxpayer details"),
            checkRow(false, "Publish to BIR"),
          ),
        ],
        "Allow",
        f.allowBtn,
        LIT_ALLOW,
        () => f.allowed.get() && "Allowed. Publishing to BIR still asks you first.",
      );

    r.tl.script = (s) => {
      r.reset([], []);
      c.clear();
      f.login.set(0);
      f.allowed.set(false);
      s.wait(300);
      s.then(() => r.act(0, "Ask for the plugin in plain words. The co-worker installs it with a tool call.")).wait(400);
      r.say(s, "Install the Buwiz plugin", () => c.push(me("Install the Buwiz plugin")));
      s.wait(300);
      tool(s, r, c, "plugins", "install_plugin buwiz", 1400);
      s.then(() => {
        r.connect(["Buwiz"]);
        c.push(installed);
      })
        .wait(1600)
        .then(() => r.act(1, "Say “Log in with Buwiz”. The co-worker asks for your login on a form card."))
        .wait(300);
      r.say(s, "Log in with Buwiz", () => c.push(me("Log in with Buwiz")));
      s.wait(400);
      tool(s, r, c, "buwiz", "request_user_form", 700);
      s.then(() => c.push(login)).wait(900);
      r.tap(s, f.field, LIT_FIELD, () => f.login.set(1));
      s.wait(400).then(() => r.act(2, "Pick the saved login. Touch ID fills the password, so it never lands in the chat."));
      r.tap(s, f.saved, LIT_SAVED, () => {});
      r.touchId(s, ACCOUNT, "account.buwiz.com");
      s.then(() => f.login.set(2)).wait(700);
      r.tap(s, f.loginBtn, LIT_LOGIN, () => f.login.set(3));
      s.wait(600)
        .then(() => r.act(3, "The Buwiz app asks what the co-worker may do on this computer. You choose; publishing to BIR stays off."))
        .wait(300)
        .then(() => c.push(allow))
        .wait(1800);
      r.tap(s, f.allowBtn, LIT_ALLOW, () => f.allowed.set(true));
      s.wait(500)
        .then(() => c.push(ai("Connected to Buwiz. I can see 3 tax profiles on your account. What should we work on?")))
        .wait(400);
      r.end(s, "Installed, logged in with Touch ID, and allowed. Publishing to BIR still asks you first.");
    };
    return film(r, STEPS, idleDesktop, ["Buwiz setup", "Weekly bookkeeping"], c);
  }

  // ---------------------------------------------------------------------------------------
  // Film 2, Tax profiles: generative UI over the Buwiz plugin. One mixed-income earner's
  // profile with every branch under the TIN, the branch you pick, the tax year and forms, and
  // what falls due next.

  function profilesFilm() {
    const STEPS = ["1 · Your profile", "2 · Pick a branch", "3 · Tax year", "4 · Forms", "5 · Due dates"];
    const LIT_ROW = LIT_FILM;
    /// The branch the film picks: its row in `BRANCHES`.
    const PICK = 1;
    /// Code, trade name, where (RDO), what it earns from.
    const BRANCHES = [
      ["00000", "Head office", "Makati · 047", "Salary + freelance"],
      ["00001", "Dela Cruz Bakeshop", "Makati · 047", "Bakery, 6 staff"],
      ["00002", "Dela Cruz Bakeshop", "Quezon City · 038", "Bakery, 3 staff"],
      ["00003", "JDC Apartments", "Pasig · 043", "Rentals, 1 staff"],
    ];
    /// Form, how often, what it is, which branches file it.
    const FORMS = [
      ["1701Q", "Quarterly", "Income tax, all income", "00000"],
      ["2551Q", "Quarterly", "Percentage tax", "All"],
      ["1601-C", "Monthly", "Tax withheld on salaries", "00001–03"],
      ["1604-C", "Yearly", "Annual withholding report", "00001–03"],
    ];
    /// Form, period, branches, due, left.
    const DUES = [
      ["1601-C", "Oct 2026", "×3", "Nov 10", "In 17 days"],
      ["1701Q", "Q3 2026", "", "Nov 16", "In 23 days"],
      ["2551Q", "Q4 2026", "×4", "Jan 25, 2027", "In 93 days"],
      ["1604-C", "2026", "×3", "Feb 1, 2027", "In 100 days"],
    ];
    const r = new Rig();
    const c = new Chat(r);
    const f = { picked: signal(false), formsOn: signal(0), row: ref() };

    const table = () =>
      card(
        "account_tree",
        "JUAN DELA CRUZ",
        "Mini app",
        h(
          "div",
          { class: "gs-d -mt-0.5 mb-2 flex flex-wrap items-center gap-x-2 gap-y-0.5 text-[11.5px]" },
          h("span", { class: "font-mono" }, "TIN ***-***-788"),
          h("span", null, "·"),
          h("span", null, "Mixed income earner"),
          h("span", null, "·"),
          h("span", null, "4 branches"),
        ),
        h(
          "div",
          { class: "grid grid-cols-[auto_1fr_auto] gap-x-3 sm:grid-cols-[auto_1fr_auto_auto]" },
          h("span", { class: "gs-f pb-1 text-[11px]" }, "Branch"),
          h("span", { class: "gs-f pb-1 text-[11px]" }, "Name"),
          h("span", { class: "gs-f pb-1 text-[11px]" }, "RDO"),
          h("span", { class: "gs-f hidden pb-1 text-[11px] sm:block" }, "Income"),
        ),
        BRANCHES.map(([code, name, place, kind], i) => {
          const base =
            i === PICK
              ? "gs-line -mx-1.5 grid grid-cols-[auto_1fr_auto] gap-x-3 rounded-lg border-t px-1.5 py-1.5 sm:grid-cols-[auto_1fr_auto_auto]"
              : "gs-line -mx-1.5 grid grid-cols-[auto_1fr_auto] gap-x-3 border-t px-1.5 py-1.5 sm:grid-cols-[auto_1fr_auto_auto]";
          const lit = r.litClass(LIT_ROW, base);
          return h(
            "div",
            i === PICK ? { ref: f.row, class: () => lit() + (f.picked.get() ? " gs-hover" : "") } : { class: base },
            h("span", { class: "font-mono text-[11.5px]" }, code),
            h("span", { class: "truncate font-medium" }, name),
            h("span", { class: "gs-d text-[11.5px]" }, place),
            h("span", { class: "gs-d hidden text-[11.5px] sm:block" }, kind),
          );
        }),
      );
    const profile = () =>
      card(
        "storefront",
        "Dela Cruz Bakeshop · 00001",
        "Branch",
        rows([
          ["TIN", "***-***-788-00001"],
          ["Registered to", "JUAN DELA CRUZ"],
          ["RDO", "047 · East Makati"],
          ["Address", "12 Rizal St., Makati City"],
          ["Business", "Bakery and pastry shop"],
          ["Employees", "6"],
        ]),
        h("div", { class: "mt-2.5 flex gap-2" }, h("span", { class: BTN }, "View profile"), h("span", { class: BTN }, "Edit in Buwiz")),
      );
    const year = () =>
      card(
        "calendar_month",
        "Tax year 2026",
        "",
        h("p", { class: "gs-d" }, "Added for JUAN DELA CRUZ and all 4 branches. Calendar year, January to December 2026."),
      );
    const forms = () =>
      card(
        "description",
        "Forms for 2026",
        "Mini app",
        FORMS.map(([code, every, what, branches], i) =>
          h(
            "div",
            { class: "flex items-center gap-2 py-1" },
            h(
              "span",
              {
                class: () =>
                  f.formsOn.get() > i ? "material-symbols-outlined text-[16px] text-[#14C370]" : "material-symbols-outlined gs-f text-[16px]",
              },
              () => (f.formsOn.get() > i ? "check_box" : "check_box_outline_blank"),
            ),
            h("span", { class: "w-14 font-mono font-semibold" }, code),
            h("span", { class: "gs-d min-w-0 flex-1 truncate" }, what),
            h("span", { class: "gs-f hidden font-mono text-[11px] sm:inline" }, branches),
            h("span", { class: "gs-hover gs-d rounded-full px-2 py-0.5 text-[10.5px]" }, every),
          ),
        ),
      );
    const dues = () =>
      card(
        "event_upcoming",
        "Upcoming due dates",
        "Mini app",
        DUES.map(([code, period, many, due, left], i) =>
          h(
            "div",
            { class: i === 0 ? "flex items-center gap-2 py-1" : "gs-line flex items-center gap-2 border-t py-1" },
            h("span", { class: "w-14 font-mono font-semibold" }, code),
            h("span", { class: "gs-d min-w-0 flex-1 truncate" }, period, h("span", { class: "gs-f ml-1.5 text-[11px]" }, many)),
            h("span", { class: "tabular-nums" }, due),
            h(
              "span",
              {
                class:
                  i < 2
                    ? "w-[78px] rounded-full bg-[#f59e0b]/15 px-2 py-0.5 text-center text-[10.5px] font-semibold text-[#d97706]"
                    : "gs-hover gs-d w-[78px] rounded-full px-2 py-0.5 text-center text-[10.5px]",
              },
              left,
            ),
          ),
        ),
      );

    r.tl.script = (s) => {
      r.reset(["Buwiz"], []);
      c.clear();
      f.picked.set(false);
      f.formsOn.set(0);
      s.wait(300);
      s.then(() => r.act(0, "Ask in plain words. Your TIN and every branch under it come back as a live table.")).wait(300);
      r.say(s, "Show my tax profile", () => c.push(me("Show my tax profile")));
      s.wait(300);
      tool(s, r, c, "buwiz", "list_tax_profiles", 1000);
      s.then(() => c.push(table))
        .wait(1700)
        .then(() => r.act(1, "One TIN, four branches: a job, two bakeshops and an apartment. Point at one to work on it."));
      r.tap(s, f.row, LIT_ROW, () => f.picked.set(true));
      s.then(() => c.push(me("Open the Makati bakeshop"))).wait(300);
      tool(s, r, c, "buwiz", "get_branch 00001", 900);
      s.then(() => c.push(profile))
        .wait(1800)
        .then(() => r.act(2, "Set the tax year once. It covers every branch."));
      r.say(s, "Set the tax year to 2026", () => c.push(me("Set the tax year to 2026")));
      s.wait(300);
      tool(s, r, c, "buwiz", "add_tax_year 2026", 900);
      s.then(() => c.push(year))
        .wait(1300)
        .then(() => r.act(3, "Say what you file. Buwiz knows which branch files each form, and when."));
      r.say(s, "We file 1701Q, 2551Q, 1601-C and 1604-C", () => c.push(me("We file 1701Q, 2551Q, 1601-C and 1604-C")));
      s.wait(300);
      tool(s, r, c, "buwiz", "choose_forms", 800);
      s.then(() => c.push(forms)).wait(400);
      for (let n = 1; n <= FORMS.length; n++) s.then(() => f.formsOn.set(n)).wait(350);
      s.wait(900).then(() => r.act(4, "Ask what is due. Deadlines come from each form's schedule."));
      r.say(s, "What's due next?", () => c.push(me("What's due next?")));
      s.wait(300);
      tool(s, r, c, "buwiz", "list_tax_years", 800);
      s.then(() => c.push(dues))
        .wait(900)
        .then(() =>
          c.push(ai("Three 1601-Cs are due Nov 10, one for each branch with staff. Send me October's payroll and I'll prepare them.")),
        )
        .wait(300);
      r.end(s, "One TIN, every branch: year, forms and deadlines, each as a card you can keep as a Mini App.");
    };
    return film(r, STEPS, idleDesktop, ["My branches", "Buwiz setup"], c);
  }

  // ---------------------------------------------------------------------------------------
  // Film 3, Prepare returns: attach the accountant's payroll workbook for three branches under
  // one TIN, the co-worker opens it, fills one October 1601-C per branch, checks them against
  // BIR's rules, shows branch 00001's form as a PDF, and marks them reviewed when you say so.

  /// The filled 1601-C exactly as Buwiz's print preview renders it.
  const formPage = () => docFrame("/assets/demo/1601c-october-2026.html");

  function prepareFilm() {
    const STEPS = ["1 · Accountant's sheet", "2 · Process it", "3 · Fill the form", "4 · View the form", "5 · Reviewed"];
    const LIT_VIEW = LIT_FILM;
    const SHEET = "payroll-oct-2026.xlsx";
    const PDF = "1601-C · October 2026 · 00001.pdf";
    /// Per branch: [branch, its employees, its subtotal]. A row is employee, gross, de minimis,
    /// mandatory contributions, taxable, withheld (TRAIN's 2023 monthly table).
    const PAYROLL = [
      [
        "00001 · Dela Cruz Bakeshop, Makati",
        [
          ["Santos, Ana", "42,000.00", "2,000.00", "3,400.00", "36,600.00", "2,528.40"],
          ["Reyes, Ben", "36,000.00", "2,000.00", "2,900.00", "31,100.00", "1,540.05"],
          ["Cruz, Carla", "32,000.00", "2,000.00", "2,600.00", "27,400.00", "985.05"],
          ["Bautista, Dan", "28,000.00", "2,000.00", "2,400.00", "23,600.00", "415.05"],
          ["Garcia, Ella", "26,000.00", "2,000.00", "2,300.00", "21,700.00", "130.05"],
          ["Mendoza, Fe", "22,000.00", "2,000.00", "2,000.00", "18,000.00", "0.00"],
        ],
        ["Subtotal", "186,000.00", "12,000.00", "15,600.00", "158,400.00", "5,598.60"],
      ],
      [
        "00002 · Dela Cruz Bakeshop, Quezon City",
        [
          ["Lim, Gina", "30,000.00", "2,000.00", "2,500.00", "25,500.00", "700.05"],
          ["Torres, Hans", "26,000.00", "2,000.00", "2,300.00", "21,700.00", "130.05"],
          ["Villanueva, Ivy", "24,000.00", "2,000.00", "2,100.00", "19,900.00", "0.00"],
        ],
        ["Subtotal", "80,000.00", "6,000.00", "6,900.00", "67,100.00", "830.10"],
      ],
      [
        "00003 · JDC Apartments, Pasig",
        [["Ramos, Jose", "15,000.00", "0.00", "1,200.00", "13,800.00", "0.00"]],
        ["Subtotal", "15,000.00", "0.00", "1,200.00", "13,800.00", "0.00"],
      ],
    ];
    const TOTAL = ["All branches", "281,000.00", "18,000.00", "23,700.00", "239,300.00", "6,428.70"];
    /// One 1601-C per branch: branch, place, compensation, tax withheld.
    const RETURNS = [
      ["00001", "Makati bakeshop", "₱186,000.00", "₱5,598.60"],
      ["00002", "QC bakeshop", "₱80,000.00", "₱830.10"],
      ["00003", "Pasig apartments", "₱15,000.00", "₱0.00"],
    ];
    const r = new Rig();
    const c = new Chat(r);
    /// What the Computer shows: "nothing", "sheet" or "form".
    const f = { open: signal("nothing"), reviewed: signal(false), viewBtn: ref() };

    const draft = () =>
      card(
        "description",
        "1601-C · October 2026 · 3 branches",
        "Mini app",
        h(
          "div",
          { class: "grid grid-cols-[auto_1fr_auto] gap-x-3 sm:grid-cols-[auto_1fr_auto_auto]" },
          h("span", { class: "gs-f pb-1 text-[11px]" }, "Branch"),
          h("span", { class: "gs-f pb-1 text-[11px]" }),
          h("span", { class: "gs-f hidden pb-1 text-right text-[11px] sm:block" }, "Compensation"),
          h("span", { class: "gs-f pb-1 text-right text-[11px]" }, "Tax withheld"),
          RETURNS.map(([code, place, comp, tax]) => [
            h("span", { class: "gs-line border-t py-1 font-mono text-[11.5px]" }, code),
            h("span", { class: "gs-line truncate border-t py-1" }, place),
            h("span", { class: "gs-line gs-d hidden border-t py-1 text-right tabular-nums sm:block" }, comp),
            h("span", { class: "gs-line border-t py-1 text-right font-medium tabular-nums" }, tax),
          ]),
          h("span", { class: "gs-line border-t pt-1 font-semibold" }, "Total"),
          h("span", { class: "gs-line border-t pt-1" }),
          h("span", { class: "gs-line gs-d hidden border-t pt-1 text-right tabular-nums sm:block" }, "₱281,000.00"),
          h("span", { class: "gs-line border-t pt-1 text-right font-semibold tabular-nums" }, "₱6,428.70"),
        ),
        h(
          "div",
          { class: "mt-2 flex items-center gap-1.5 text-[11.5px] text-[#14C370]" },
          h("span", { class: "material-symbols-outlined text-[15px]" }, "verified"),
          "All three pass every 1601-C rule",
        ),
        h(
          "div",
          { class: "mt-2.5 flex items-center gap-2" },
          h(
            "div",
            { ref: f.viewBtn, class: r.litClass(LIT_VIEW, BTN_PRIMARY) },
            h("span", { class: "material-symbols-outlined text-[14px]" }, "picture_as_pdf"),
            "View form",
          ),
          h("span", { class: BTN }, "Open in Buwiz"),
          h(
            "span",
            {
              class: () =>
                f.reviewed.get()
                  ? "ml-auto rounded-full bg-[#14C370]/15 px-2 py-0.5 text-[10.5px] font-semibold text-[#14C370]"
                  : "gs-hover gs-d ml-auto rounded-full px-2 py-0.5 text-[10.5px]",
            },
            () => (f.reviewed.get() ? "Reviewed" : "Draft"),
          ),
        ),
      );

    const sheet = () => {
      const TH = "border border-[#d4d4d8] bg-[#f4f4f5] px-2 py-1 text-left font-semibold";
      const TD = "border border-[#e4e4e7] px-2 py-[3px]";
      const tr = (cls, [n, g, d, m, t, w]) =>
        h(
          "tr",
          { class: cls },
          h("td", { class: TD }, n),
          [g, d, m, t, w].map((v) => h("td", { class: `${TD} text-right` }, v)),
        );
      return h(
        "div",
        { class: "absolute inset-0 overflow-hidden bg-white p-4 text-[12px] text-[#18181b]" },
        h("div", { class: "mb-2 font-semibold" }, "JUAN DELA CRUZ · TIN 123-456-788 · Payroll, October 2026"),
        h(
          "table",
          { class: "w-full border-collapse tabular-nums" },
          h(
            "tbody",
            null,
            h(
              "tr",
              null,
              ["Employee", "Gross pay", "De minimis", "SSS / PhilHealth / Pag-IBIG", "Taxable", "Tax withheld"].map((t) =>
                h("th", { class: TH }, t),
              ),
            ),
            PAYROLL.map(([branch, people, sub]) => [
              h(
                "tr",
                null,
                h(
                  "td",
                  { colspan: "6", class: "border border-[#d4d4d8] bg-[#eef2ff] px-2 py-1 text-[11.5px] font-semibold text-[#3730a3]" },
                  branch,
                ),
              ),
              people.map((p) => tr("", p)),
              tr("bg-[#f4f4f5] font-semibold", sub),
            ]),
            tr("bg-[#dcfce7] font-semibold", TOTAL),
          ),
        ),
        h(
          "div",
          { class: "mt-3 flex gap-1 text-[11px] text-[#71717a]" },
          h(
            "span",
            { class: "rounded-t border border-b-0 border-[#d4d4d8] bg-white px-2 py-0.5 font-semibold text-[#1d6f42]" },
            "October",
          ),
          h("span", { class: "px-2 py-0.5" }, "Employees"),
          h("span", { class: "px-2 py-0.5" }, "Notes"),
        ),
      );
    };

    const desktop = () => () => {
      const open = f.open.get();
      if (open === "sheet") return viewer("xlsx", SHEET, h("span", { class: "text-[11px] text-white/50" }, "Read only"), sheet());
      if (open === "form") {
        return viewer(
          "pdf",
          PDF,
          [
            h(
              "span",
              { class: "mr-2 flex items-center gap-1 font-mono text-[11px]" },
              h("span", { class: "rounded-full bg-white px-2 py-0.5 font-semibold text-[#1c1c1e]" }, "00001"),
              h("span", { class: "rounded-full px-2 py-0.5 text-white/55" }, "00002"),
              h("span", { class: "rounded-full px-2 py-0.5 text-white/55" }, "00003"),
            ),
            h("span", { class: "rounded-full border border-white/20 px-2.5 py-0.5 text-[11px]" }, "Print"),
            h("span", { class: "rounded-full border border-white/20 px-2.5 py-0.5 text-[11px]" }, "Download"),
          ],
          formPage(),
        );
      }
      return idleScreen();
    };

    r.tl.script = (s) => {
      r.reset(["Buwiz"], []);
      c.clear();
      f.open.set("nothing");
      f.reviewed.set(false);
      s.wait(300);
      s.then(() => r.act(0, "Attach the workbook your accountant sent, and say what it is for.")).wait(300);
      r.tap(s, r.plusBtn, LIT_PLUS, () =>
        c.push(() => fileTile(SHEET, "3 branches · 10 employees · 31 KB", "gc-card-in self-end")),
      );
      r.say(s, "From our accountant. Make October's 1601-C for each branch.", () =>
        c.push(me("From our accountant. Make October's 1601-C for each branch.")),
      );
      s.wait(300).then(() => r.act(1, "The co-worker opens the workbook and reads each branch's payroll."));
      tool(s, r, c, "buwiz", "read_workbook", 500);
      s.then(() => {
        f.open.set("sheet");
        r.openScreen();
      })
        .wait(3000)
        .then(() => r.screen.shrink())
        .wait(700)
        .then(() => r.act(2, "It fills one 1601-C per branch from that branch's totals, then runs every BIR rule."));
      tool(s, r, c, "buwiz", "save_form_draft 1601-C × 3 · October 2026", 1100);
      tool(s, r, c, "buwiz", "validate_form × 3 · 0 errors", 900);
      s.then(() => c.push(draft))
        .wait(1500)
        .then(() => r.act(3, "Open the form as BIR prints it. Spreadsheets, PDFs and pages all open right here."));
      r.tap(s, f.viewBtn, LIT_VIEW, () => {
        f.open.set("form");
        r.openScreen();
      });
      s.wait(3400)
        .then(() => r.screen.shrink())
        .wait(700)
        .then(() => r.act(4, "Happy with it? Say so, and the draft is marked reviewed."));
      r.say(s, "Looks right. Mark them reviewed.", () => c.push(me("Looks right. Mark them reviewed.")));
      s.wait(300);
      tool(s, r, c, "buwiz", "mark_draft_reviewed × 3", 700);
      s.then(() => {
        f.reviewed.set(true);
        c.push(ai("All three are reviewed. Say “submit” when you want them filed."));
      }).wait(300);
      r.end(s, "One workbook, three branches: three reviewed 1601-Cs, checked against BIR's rules.");
    };
    return film(r, STEPS, desktop, ["October returns", "Tax profiles"], c);
  }

  // ---------------------------------------------------------------------------------------
  // Film 4, Submit: say "submit", the co-worker validates the draft again, finds it may not
  // publish to BIR and asks for that permission, then asks you to approve the submission
  // itself. Approved, the return joins the filing queue.

  function submitFilm() {
    const STEPS = ["1 · Ask to submit", "2 · Validate again", "3 · Permission", "4 · Approve", "5 · Queued"];
    const [LIT_ALLOW, LIT_SUBMIT] = [LIT_FILM, LIT_FILM + 1];
    const r = new Rig();
    const c = new Chat(r);
    /// `stage`: 0 queued, 1 sending, 2 sent.
    const f = { allowed: signal(false), submitted: signal(false), stage: signal(0), allowBtn: ref(), submitBtn: ref() };

    const permission = () =>
      approveCard(
        r,
        "shield_person",
        "Allow Buwiz to publish to BIR?",
        [
          checkRow(true, "Publish to BIR"),
          h("div", { class: "gs-d mt-1 text-[11.5px]" }, "Only for 1601-C · October 2026, branch 00001. Each submission still asks you."),
        ],
        "Allow once",
        f.allowBtn,
        LIT_ALLOW,
        () => f.allowed.get() && "Allowed for this return",
      );
    const confirm = () =>
      approveCard(
        r,
        "send",
        "Submit this return to BIR?",
        rows([
          ["Form", "1601-C"],
          ["Period", "October 2026"],
          ["Taxpayer", "JUAN DELA CRUZ"],
          ["Branch", "00001 · Dela Cruz Bakeshop, Makati"],
          ["TIN", "***-***-788-00001"],
          ["Tax due", "₱5,598.60"],
        ]),
        "Submit",
        f.submitBtn,
        LIT_SUBMIT,
        () => f.submitted.get() && "Approved by you",
      );
    const queued = () =>
      card(
        "schedule_send",
        "Filing queue",
        "Mini app",
        [
          ["Queued", 0],
          ["Sending to BIR", 1],
          ["Sent, awaiting confirmation", 2],
        ].map(([label, at]) =>
          h(
            "div",
            { class: "flex items-center gap-2 py-0.5" },
            () => {
              const st = f.stage.get();
              if (st > at || (st === 2 && at === 2)) {
                return h("span", { class: "material-symbols-outlined text-[16px] text-[#14C370]" }, "check_circle");
              }
              if (st === at) {
                return h("span", { class: "mx-[2px] h-3 w-3 animate-spin rounded-full border-[1.5px] border-[#14C370] border-t-transparent" });
              }
              return h("span", { class: "material-symbols-outlined gs-f text-[16px]" }, "radio_button_unchecked");
            },
            h("span", { class: () => (f.stage.get() >= at ? "" : "gs-f") }, label),
          ),
        ),
        h("div", { class: "gs-d mt-1.5 text-[11.5px]" }, "1601-C · October 2026 · ₱5,598.60"),
      );

    r.tl.script = (s) => {
      r.reset(["Buwiz"], []);
      c.clear();
      f.allowed.set(false);
      f.submitted.set(false);
      f.stage.set(0);
      s.wait(300);
      s.then(() => r.act(0, "When the reviewed form looks right, just say “submit”.")).wait(300);
      r.say(s, "Submit the Makati branch's.", () => c.push(me("Submit the Makati branch's.")));
      s.wait(300).then(() => r.act(1, "It checks the form against every BIR rule again before anything leaves."));
      tool(s, r, c, "buwiz", "validate_form 1601-C · 0 errors", 1100);
      tool(s, r, c, "buwiz", "check_permission publish", 800);
      s.then(() => r.act(2, "Publishing was left off, so it asks. You can allow it for this return only."))
        .then(() => c.push(ai("Publishing to BIR is off for me. Allow it for this return?")))
        .wait(500)
        .then(() => c.push(permission))
        .wait(1600);
      r.tap(s, f.allowBtn, LIT_ALLOW, () => f.allowed.set(true));
      s.wait(400)
        .then(() => r.act(3, "One last look: what is filed, for which period, and the tax due. Nothing goes until you approve."))
        .then(() => c.push(confirm))
        .wait(2000);
      r.tap(s, f.submitBtn, LIT_SUBMIT, () => f.submitted.set(true));
      s.wait(300).then(() => r.act(4, "Approved. The return joins the Buwiz app's filing queue and goes to BIR from your computer."));
      tool(s, r, c, "buwiz", "submit_filing", 900);
      s.then(() => c.push(queued))
        .wait(1300)
        .then(() => f.stage.set(1))
        .wait(1800)
        .then(() => f.stage.set(2))
        .wait(600)
        .then(() => c.push(ai("Sent. BIR emails a confirmation when it is received; want me to watch for it?")))
        .wait(300);
      r.end(s, "Validated twice, permission asked, your approval given, then queued.");
    };
    return film(r, STEPS, idleDesktop, ["October returns", "Tax profiles"], c);
  }

  // ---------------------------------------------------------------------------------------
  // Film 5, Confirmation email: install Gmail, set a routine that checks the inbox every
  // minute, and when BIR's confirmation arrives the co-worker brings it back as a PDF to print
  // or keep.

  function confirmFilm() {
    const STEPS = ["1 · Install Gmail", "2 · Set a routine", "3 · Every minute", "4 · Email arrives", "5 · Confirmation PDF"];
    const [LIT_OPEN, LIT_DOWNLOAD] = [LIT_FILM, LIT_FILM + 1];
    const PDF = "BIR confirmation · 1601-C October 2026.pdf";
    const r = new Rig();
    const c = new Chat(r);
    const f = { note: signal(""), open: signal(false), saved: signal(false), openBtn: ref(), downloadBtn: ref() };

    const installed = () =>
      card(
        "extension",
        "Gmail installed",
        "Plugin",
        h("p", { class: "gs-d" }, "Reads and searches the inbox you connected. It cannot send mail unless you allow it."),
      );
    const routine = () =>
      card(
        "schedule",
        "Routine: BIR confirmation",
        "Routine",
        h("p", { class: "gs-d" }, "Every minute, search Gmail for BIR's confirmation of 1601-C · October 2026. Stop once found."),
        h("div", { class: "gs-f mt-1.5 text-[11.5px]" }, () => f.note.get()),
      );
    const email = () =>
      card(
        "mail",
        "Tax Return Receipt Confirmation",
        "Gmail",
        h("div", { class: "gs-d text-[11.5px]" }, "ebirforms-noreply@bir.gov.ph · 10:44"),
        h("p", { class: "mt-1" }, "This confirms receipt of your submission with the following details subject to validation by BIR:"),
        h(
          "div",
          { class: "gs-d mt-1 font-mono text-[11px] leading-relaxed" },
          "File name: 123456788001-1601Cv2018-102026.xml",
          h("br"),
          "Date received by BIR: 3 November 2026",
          h("br"),
          "Time received by BIR: 10:43 AM",
        ),
        h(
          "div",
          { class: "mt-2.5 flex gap-2" },
          h(
            "div",
            { ref: f.openBtn, class: r.litClass(LIT_OPEN, BTN_PRIMARY) },
            h("span", { class: "material-symbols-outlined text-[14px]" }, "picture_as_pdf"),
            "Open PDF",
          ),
          h("span", { class: BTN }, "Open email"),
        ),
      );

    /// The return as Buwiz prints it with BIR's confirmation email as its last page.
    const letter = () => docFrame("/assets/demo/1601c-october-2026-confirmation.html", "page-receipt");
    const desktop = (big) => () => {
      if (!f.open.get()) return idleScreen();
      if (!big) return letter();
      return viewer(
        "pdf",
        PDF,
        [
          h("span", { class: "rounded-full border border-white/20 px-2.5 py-0.5 text-[11px]" }, "Print"),
          h(
            "div",
            { ref: f.downloadBtn, class: r.litClass(LIT_DOWNLOAD, "rounded-full border border-white/20 px-2.5 py-0.5 text-[11px]") },
            () => (f.saved.get() ? "Saved" : "Download"),
          ),
        ],
        letter(),
      );
    };

    r.tl.script = (s) => {
      r.reset(["Buwiz"], []);
      c.clear();
      f.note.set("");
      f.open.set(false);
      f.saved.set(false);
      s.wait(300);
      s.then(() => r.act(0, "The co-worker needs your inbox for this. Ask it to install Gmail.")).wait(300);
      r.say(s, "Install Gmail", () => c.push(me("Install Gmail")));
      s.wait(300);
      tool(s, r, c, "plugins", "install_plugin gmail", 1300);
      s.then(() => {
        r.connect(["Buwiz", "Gmail"]);
        c.push(installed);
      })
        .wait(1300)
        .then(() => r.act(1, "Now ask for a routine: it runs on a schedule, even when you are not chatting."));
      r.say(s, "Check my email every minute for BIR's confirmation", () =>
        c.push(me("Check my email every minute for BIR's confirmation")),
      );
      s.wait(300);
      tool(s, r, c, "routine", "create_routine every 1m", 900);
      s.then(() => {
        f.note.set("Starting…");
        r.addRoutine({ name: "BIR confirmation", when: "Every minute · Gmail", note: f.note });
        c.push(routine);
      })
        .wait(1200)
        .then(() => r.act(2, "Each run searches the inbox. Nothing yet, so it stays quiet."));
      for (const note of ["10:41 · nothing yet", "10:42 · nothing yet", "10:43 · nothing yet"]) {
        s.then(() => f.note.set("Checking Gmail…"))
          .wait(700)
          .then(() => f.note.set(note))
          .wait(900);
      }
      s.then(() => f.note.set("Checking Gmail…"))
        .wait(700)
        .then(() => {
          f.note.set("10:44 · found BIR's confirmation");
          r.act(3, "BIR's email lands. The routine picks it up and tells you in the chat.");
        });
      tool(s, r, c, "gmail", "search “Tax Return Receipt Confirmation”", 800);
      s.then(() => c.push(email))
        .wait(1600)
        .then(() => r.act(4, "The confirmation opens as a PDF, ready to print or save."));
      r.tap(s, f.openBtn, LIT_OPEN, () => {
        f.open.set(true);
        r.openScreen();
      });
      s.wait(1800);
      r.tap(s, f.downloadBtn, LIT_DOWNLOAD, () => f.saved.set(true));
      s.wait(1400)
        .then(() => r.screen.shrink())
        .wait(600)
        .then(() => c.push(ai("Saved to Downloads and to the return in Buwiz. Want me to pay the ₱5,598.60 next?")))
        .wait(300);
      r.end(s, "A routine watched the inbox and brought back BIR's confirmation as a PDF.");
    };
    return film(r, STEPS, desktop, ["October returns", "Tax profiles"], c);
  }

  // ---------------------------------------------------------------------------------------
  // Film 6, Pay: a routine that pays once BIR's confirmation is in. The co-worker opens the
  // bank's online payment site on its computer, picks BIR, fills the form, period, ATC and
  // amount, asks you to log in (saved login, Touch ID) and to approve the amount, then pays and
  // keeps the receipt. The bank site here is a mock.

  function payFilm() {
    const STEPS = ["1 · Pay routine", "2 · Open the bank", "3 · Fill the payment", "4 · Log in", "5 · Approve", "6 · Receipt"];
    const [LIT_FIELD, LIT_SAVED, LIT_LOGIN, LIT_PAY] = [LIT_FILM, LIT_FILM + 1, LIT_FILM + 2, LIT_FILM + 3];
    const ACCOUNT = "delacruz.bakeshop";
    const HOST = "linkbiz.landbank.demo";
    /// Label, value, as the co-worker fills them in.
    const FIELDS = [
      ["TIN", "123-456-788-00001"],
      ["RDO", "047 · East Makati"],
      ["Form", "1601-C"],
      ["ATC", "WW010"],
      ["Tax type", "WC · Withholding on compensation"],
      ["Return period", "10/31/2026"],
      ["Amount", "5,598.60"],
    ];
    const r = new Rig();
    const c = new Chat(r);
    /// `page`: "blank", "billers", "form", "login", "paying" or "receipt".
    const f = {
      note: signal(""),
      page: signal("blank"),
      search: signal(""),
      filled: signal(0),
      signed: signal(false),
      login: signal(0),
      approved: signal(false),
      field: ref(),
      saved: ref(),
      loginBtn: ref(),
      payBtn: ref(),
    };
    const watch = signal("10:44 · found BIR's confirmation");

    const routine = () =>
      card(
        "schedule",
        "Routine: Pay BIR",
        "Routine",
        h("p", { class: "gs-d" }, "When BIR confirms 1601-C · October 2026, pay ₱5,598.60 through LandBank. Ask me before paying."),
        h("div", { class: "gs-f mt-1.5 text-[11.5px]" }, () => f.note.get()),
      );
    const login = () =>
      loginCard(r, "browser", "Log in to LandBank", HOST, ACCOUNT, f.login, [f.field, f.saved, f.loginBtn], [LIT_FIELD, LIT_SAVED, LIT_LOGIN]);
    const approve = () =>
      approveCard(
        r,
        "payments",
        "Pay ₱5,598.60 to BIR?",
        rows([
          ["Biller", "Bureau of Internal Revenue"],
          ["Form", "1601-C · October 2026"],
          ["ATC", "WW010"],
          ["From", "LandBank · Savings ••• 4821"],
        ]),
        "Pay ₱5,598.60",
        f.payBtn,
        LIT_PAY,
        () => f.approved.get() && "Approved by you",
      );
    const receipt = () =>
      card(
        "receipt_long",
        "Payment receipt",
        "PDF",
        rows([
          ["Paid", "₱5,598.60"],
          ["For", "1601-C · October 2026"],
          ["Reference", "LBP-2611-0387-4412"],
          ["Date", "Nov 3, 2026 · 10:47"],
        ]),
        h("div", { class: "mt-2.5 flex gap-2" }, h("span", { class: BTN }, "Download PDF"), h("span", { class: BTN }, "Print")),
      );

    const FIELD = "flex h-9 items-center rounded border border-[#cbd5e1] px-3 text-[13px]";
    const page = () => {
      switch (f.page.get()) {
        case "billers":
          return h(
            "div",
            { class: "mx-auto max-w-[560px]" },
            h("div", { class: "mb-2 text-[15px] font-bold" }, "Choose a biller"),
            h(
              "div",
              { class: `${FIELD} bg-white` },
              h("span", { class: "material-symbols-outlined mr-2 text-[18px] text-[#64748b]" }, "search"),
              () => f.search.get(),
              h("span", { class: "gc-caret" }),
            ),
            h("div", { class: "mt-3 flex flex-col gap-2" }, () =>
              f.search.get().length === 3
                ? h("div", { class: "rounded border-2 border-[#0b6b3a] bg-white px-3 py-2.5 text-[13px] font-semibold" }, "Bureau of Internal Revenue (eTax)")
                : ["Bureau of Internal Revenue (eTax)", "Social Security System", "Pag-IBIG Fund", "PhilHealth"].map((b) =>
                    h("div", { class: "rounded border border-[#e2e8f0] bg-white px-3 py-2.5 text-[13px]" }, b),
                  ),
            ),
          );
        case "form":
          return h(
            "div",
            { class: "mx-auto max-w-[620px] rounded-lg bg-white p-5 shadow-sm" },
            h("div", { class: "mb-3 text-[15px] font-bold" }, "Bureau of Internal Revenue · Tax payment"),
            h(
              "div",
              { class: "grid grid-cols-2 gap-x-4 gap-y-2.5" },
              FIELDS.map(([label, value], i) =>
                h(
                  "div",
                  null,
                  h("div", { class: "mb-0.5 text-[11px] font-semibold text-[#475569]" }, label),
                  h(
                    "div",
                    { class: () => (f.filled.get() === i + 1 ? `${FIELD} ring-2 ring-[#0b6b3a]/40` : FIELD) },
                    () => f.filled.get() > i && value,
                  ),
                ),
              ),
            ),
            h(
              "div",
              { class: "mt-4 flex justify-end" },
              h("span", { class: "rounded bg-[#0b6b3a] px-4 py-2 text-[13px] font-semibold text-white" }, "Proceed to payment"),
            ),
          );
        case "login":
        case "paying":
          return h(
            "div",
            { class: "mx-auto max-w-[380px] rounded-lg bg-white p-5 shadow-sm" },
            h("div", { class: "text-[15px] font-bold" }, "Pay with your LandBank account"),
            h("div", { class: "mb-3 text-[12px] text-[#64748b]" }, "BIR · 1601-C · PHP 5,598.60"),
            h("div", { class: "mb-0.5 text-[11px] font-semibold text-[#475569]" }, "User ID"),
            h("div", { class: FIELD }, () => f.signed.get() && ACCOUNT),
            h("div", { class: "mb-0.5 mt-2.5 text-[11px] font-semibold text-[#475569]" }, "Password"),
            h("div", { class: FIELD }, () => f.signed.get() && "••••••••••"),
            h(
              "div",
              { class: "mt-4 rounded bg-[#0b6b3a] py-2 text-center text-[13px] font-semibold text-white" },
              () => (f.page.get() === "paying" ? "Paying…" : "Log in and pay"),
            ),
          );
        case "receipt":
          return h(
            "div",
            { class: "mx-auto max-w-[420px] rounded-lg bg-white p-6 text-center shadow-sm" },
            h("span", { class: "material-symbols-outlined text-[44px] text-[#0b6b3a]" }, "check_circle"),
            h("div", { class: "text-[16px] font-bold" }, "Payment successful"),
            h(
              "div",
              { class: "mt-3 grid grid-cols-2 gap-y-1 text-left text-[12.5px]" },
              h("span", { class: "text-[#64748b]" }, "Amount"),
              h("span", { class: "text-right font-semibold" }, "PHP 5,598.60"),
              h("span", { class: "text-[#64748b]" }, "Biller"),
              h("span", { class: "text-right" }, "BIR (eTax)"),
              h("span", { class: "text-[#64748b]" }, "Reference"),
              h("span", { class: "text-right font-mono" }, "LBP-2611-0387-4412"),
            ),
          );
        default:
          return null;
      }
    };
    const site = () =>
      h(
        "div",
        { class: "flex h-full flex-col bg-[#f1f5f2]" },
        h(
          "div",
          { class: "flex h-12 shrink-0 items-center gap-3 bg-[#0b6b3a] px-5 text-white" },
          h("span", { class: "grid h-7 w-7 place-items-center rounded-full bg-[#f5c518] text-[12px] font-black text-[#0b6b3a]" }, "LB"),
          h("span", { class: "text-[15px] font-bold" }, "Link.BizPortal"),
          h("span", { class: "ml-auto rounded bg-white/15 px-2 py-0.5 text-[11px]" }, "Demo site"),
        ),
        h("div", { class: "min-h-0 flex-1 overflow-hidden p-5" }, page),
      );
    const desktop = () => () => (f.page.get() === "blank" ? idleScreen() : browser("Link.BizPortal", HOST, "/bir/pay", site()));

    r.tl.script = (s) => {
      r.reset(["Buwiz", "Gmail"], [{ name: "BIR confirmation", when: "Every minute · Gmail", note: watch }]);
      c.clear();
      f.note.set("");
      f.page.set("blank");
      f.search.set("");
      f.filled.set(0);
      f.signed.set(false);
      f.login.set(0);
      f.approved.set(false);
      s.wait(300);
      s.then(() => r.act(0, "One more routine: when BIR's confirmation is in, pay the tax due online.")).wait(300);
      r.say(s, "When the confirmation arrives, pay it through LandBank", () =>
        c.push(me("When the confirmation arrives, pay it through LandBank")),
      );
      s.wait(300);
      tool(s, r, c, "routine", "create_routine on BIR confirmation", 900);
      s.then(() => {
        f.note.set("Waiting for the confirmation");
        r.addRoutine({ name: "Pay BIR", when: "When the confirmation arrives", note: f.note });
        c.push(routine);
      })
        .wait(1400)
        .then(() => {
          f.note.set("Confirmation found · paying");
          c.push(ai("BIR's confirmation is in. Opening LandBank's online payment site."));
          r.act(1, "The routine fires. The co-worker opens the bank's payment site on its computer.");
        })
        .wait(500);
      tool(s, r, c, "browser", "open linkbiz.landbank.demo", 600);
      s.then(() => {
        f.page.set("billers");
        r.openScreen();
      })
        .wait(900)
        .typing("BIR", 160, (t) => f.search.set(t))
        .wait(700)
        .then(() => {
          f.page.set("form");
          r.act(2, "It picks BIR, then fills the form, period, ATC and amount from the filed return.");
        })
        .wait(600);
      for (let n = 1; n <= FIELDS.length; n++) s.then(() => f.filled.set(n)).wait(480);
      s.wait(700)
        .then(() => f.page.set("login"))
        .wait(1100)
        .then(() => {
          r.screen.shrink();
          r.act(3, "The bank wants a login. It asks you in the chat; Touch ID fills it.");
        })
        .wait(600);
      tool(s, r, c, "browser", "request_user_form", 600);
      s.then(() => c.push(login)).wait(900);
      r.tap(s, f.field, LIT_FIELD, () => f.login.set(1));
      r.tap(s, f.saved, LIT_SAVED, () => {});
      r.touchId(s, ACCOUNT, HOST);
      s.then(() => f.login.set(2)).wait(600);
      r.tap(s, f.loginBtn, LIT_LOGIN, () => {
        f.login.set(3);
        f.signed.set(true);
      });
      s.wait(400)
        .then(() => r.act(4, "Before money moves, it shows you the amount and asks."))
        .then(() => c.push(approve))
        .wait(1800);
      r.tap(s, f.payBtn, LIT_PAY, () => f.approved.set(true));
      s.wait(300)
        .then(() => {
          f.page.set("paying");
          r.openScreen();
        })
        .wait(1600)
        .then(() => {
          f.page.set("receipt");
          f.note.set("Paid ₱5,598.60 · done");
          r.act(5, "Paid on the bank's page. The receipt goes back to the chat and onto the return in Buwiz.");
        })
        .wait(2200)
        .then(() => r.screen.shrink())
        .wait(500)
        .then(() => c.push(receipt))
        .wait(300);
      r.end(s, "Filed, confirmed and paid, with your approval at every step that counts.");
    };
    return film(r, STEPS, desktop, ["October returns", "Tax profiles"], c);
  }

  // ---------------------------------------------------------------------------------------
  // Mount every film, and play only the one whose radio is picked.

  const FILMS = {
    connect: connectFilm,
    profiles: profilesFilm,
    prepare: prepareFilm,
    submit: submitFilm,
    confirm: confirmFilm,
    pay: payFilm,
  };

  const rigs = new Map();
  for (const box of document.querySelectorAll("[data-film]")) {
    const make = FILMS[box.dataset.film];
    if (!make) continue;
    const { rig, nodes } = root(make).value;
    box.append(...nodes);
    rigs.set(box.dataset.film, rig);
  }
  const radios = [...document.querySelectorAll('input[name="demo"]')];
  for (const radio of radios) {
    radio.addEventListener("change", () => {
      for (const [name, rig] of rigs) if (name !== radio.value) rig.tl.stop();
      const rig = rigs.get(radio.value);
      if (rig) requestAnimationFrame(() => rig.start());
    });
  }

  // The films play as one tour: when one ends, the next tab opens, and after the last the
  // tour starts again from the first. A visitor's own pick just continues the tour from there.
  const reduced = matchMedia("(prefers-reduced-motion: reduce)").matches;
  radios.forEach((radio, i) => {
    const rig = rigs.get(radio.value);
    if (!rig || radios.length < 2) return;
    rig.next = () => {
      const following = radios[(i + 1) % radios.length];
      following.checked = true;
      following.dispatchEvent(new Event("change"));
      // Keep the open tab visible in a scrolling tab strip, without moving the page.
      const tab = document.querySelector(`label[for="${following.id}"]`);
      const strip = tab && tab.parentElement;
      if (strip && strip.scrollWidth > strip.clientWidth) {
        strip.scrollTo({ left: tab.offsetLeft - strip.offsetLeft - 16, behavior: reduced ? "auto" : "smooth" });
      }
    };
  });
})();
