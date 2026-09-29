// `<adi-chat>` — a chat window, whole: pick an agent and a conversation, read it as it streams,
// answer it. Plain JavaScript over the panel's own agent API, so it works anywhere the panel's
// `/api` answers — the new UI's chat window, the old chat, a page an agent writes.
//
//   <adi-chat picker></adi-chat>                          agent + conversation pickers above it
//   <adi-chat agent="adi-agent" run="1790…-0000"></adi-chat>   one conversation, no chrome
//
// Attributes:
//   agent     which agent. Without it the window opens on `adi-agent`, or the first there is.
//   run       which conversation; `new` for a fresh one. Without it, the one this browser last had
//             open with that agent, else its newest.
//   picker    draw the agent and conversation pickers.
//   api       where the panel answers, for a paired machine (`http://<node>.node.adi`). Default: this
//             page's own origin.
//
// Events: `open` `{ agent, run }` whenever the conversation shown changes — a host that keeps the
// choice in its URL listens for this.
//
// # How it reads
//
// Exactly what the old chat asks for (`state::chat_view`): the newest twenty turns, tool runs
// folded to their receipt lines, more on "earlier messages", and a folded run's calls fetched from
// `/api/agents/run/steps` when a reader opens it. It polls — every second while an answer is
// streaming or a message is queued, every five otherwise — and the transcript repaints only what
// changed (see `transcript.js`).
//
// How a wire turn becomes transcript entries is `feed_turn` in the panel
// (crates/adi-webapp/src/pages/agents/actions.rs), ported rule for rule: text is the divider, so
// every run of tool calls between two things the agent said is one receipt; a message the platform
// stamped is a note, not a bubble; keys and DOM ids are `adi-turn-<seq>[-<part>]`, and a call's
// anchor is `adi-step-<turn>-<step>`, so a link built by the panel lands here too.

import { AdiElement, define, sheet } from "./base.js";
import "./transcript.js";
import "./composer.js";
import "./ask.js";
import "./icon.js";

/** The agent a window opens on when it is told nothing — the environment's root agent. */
const ROOT_AGENT = "adi-agent";
/** Turns per page, and what "earlier messages" adds. The panel's `CHAT_PAGE`. */
const PAGE = 20;
const FAST_MS = 1000;
const SLOW_MS = 5000;

const TOOL_STATE = { running: "running", ok: "ok", error: "failed", unanswered: "unanswered" };

/** A tool call's arguments the way the model wrote them — `AgentStep::params_of`. */
function paramsOf(input) {
  try {
    const value = JSON.parse(input);
    if (value && typeof value === "object" && !Array.isArray(value)) {
      return Object.entries(value).map(([k, v]) => [k, typeof v === "string" ? v : JSON.stringify(v, null, 2)]);
    }
  } catch {
    // Not JSON — shown as the one string it is.
  }
  return [["input", input]];
}

/** The first argument, flattened: what a receipt line shows of a call. */
function previewOf(call) {
  return (call.params[0]?.[1] ?? "").split(/\s+/).filter(Boolean).join(" ");
}

function toolCall(turn, step, s) {
  return {
    name: s.name,
    params: paramsOf(s.input ?? ""),
    state: TOOL_STATE[s.status] ?? "ok",
    result: s.output || null,
    anchor: `adi-step-${turn}-${step}`,
  };
}

function thinkingCall(text) {
  return { name: "thinking", params: [["text", text]], state: "ok", result: null, anchor: null };
}

/** A run whose calls are here: its receipt derived from them, exactly as a folded one arrives. */
function loadedRun(id, calls) {
  const tools = [...new Set(calls.map((c) => c.name))];
  const head = calls.find((c) => c.state === "running") ?? calls[calls.length - 1];
  return { id, count: calls.length, tools, preview: head ? previewOf(head) : "", state: head?.state ?? "ok", calls };
}

/** The key a folded run's fetched calls are filed under: its address plus its shape. */
function runKey(turn, from, to, count, status) {
  return `adi-run-${turn}-${from}:${to}:${count}:${status}`;
}

/** Who sent a message, off its `from` marker. */
function senderOf(markers = []) {
  const from = markers.find((m) => m.kind === "from");
  if (!from) return null;
  return from.user ? `${from.node}/${from.user}` : from.node;
}

/** The platform's own note on a turn, or null for a message somebody typed — `platform_note`. */
function platformNote(turn) {
  const sender = senderOf(turn.markers);
  for (const m of turn.markers ?? []) {
    if (m.kind === "await-woken") {
      let head;
      if (m.cause === "event" && m.event) head = [{ text: "Woken by" }, { code: m.event }];
      else if (m.cause === "event") head = [{ text: "Woken by an event" }];
      else if (m.cause === "expired") head = [{ text: "Expired without firing" }];
      else head = [{ text: "Woken by the clock" }];
      if (m.check) head.push({ text: "· check passed" });
      return { icon: "bell", head, id: m.id, body: turn.text };
    }
    if (m.kind === "ask-answered") {
      const head = [{ text: m.by === "default" ? "Answered by default" : "Answered" }];
      if (sender) head.push({ text: `· ${sender}` });
      return { icon: "check", head, id: m.id, body: turn.text };
    }
    if (m.kind === "goal-check") {
      const head = [{ text: "Goal check" }];
      if (m.open > 0) head.push({ text: `· ${m.open} open` });
      return { icon: "flag", head, id: null, body: turn.text };
    }
  }
  return null;
}

class AdiChat extends AdiElement {
  static observedAttributes = ["agent", "run", "picker", "api"];

  static sheet = sheet(`
    :host { display: flex; flex-direction: column; min-height: 0; height: 100%; color: var(--ink); }
    .bar { flex: none; display: flex; align-items: center; gap: var(--s2); padding: var(--s2) var(--s4);
      border-bottom: 1px solid var(--line); }
    .bar[hidden] { display: none; }
    select { min-width: 0; padding: 5px 8px; border: 1px solid var(--line-strong); border-radius: var(--r);
      background: var(--bg-raise); color: var(--ink); font: inherit; font-size: var(--fs-small); }
    .agent { flex: 0 1 200px; }
    .conv { flex: 1; }
    .top { flex: none; display: flex; flex-direction: column; gap: var(--s2); padding: var(--s3) var(--s4) 0; }
    .top[hidden] { display: none; }
    .await { display: flex; align-items: center; gap: var(--s2); font-size: var(--fs-label); color: var(--ink-3); }
    .await .what { min-width: 0; flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--ink-2); }
    .await button { padding: 2px 8px; border-radius: var(--r); color: var(--ink-2);
      transition: background var(--transition); }
    .await button:hover { background: var(--bg-hover); }
    .error { flex: none; margin: 0; padding: var(--s2) var(--s4); font-size: var(--fs-small); color: var(--err); }
    .error:empty { display: none; }
    adi-transcript { flex: 1; min-height: 0; padding: var(--s4); }
    .lead { display: flex; flex-direction: column; gap: var(--s3); }
    .lead:empty { display: none; }
    .earlier { align-self: flex-start; padding: 4px 8px; border-radius: var(--r); font-size: var(--fs-small);
      color: var(--ink-3); transition: background var(--transition), color var(--transition); }
    .earlier:hover { background: var(--bg-hover); color: var(--ink-2); }
    .earlier[hidden] { display: none; }
    .empty { font-size: var(--fs-small); color: var(--ink-3); }
    .empty:empty { display: none; }
    pre.log { margin: 0; padding: 12px 14px; border: 1px solid var(--line); border-radius: var(--r-lg);
      background: var(--bg-raise); font-family: var(--mono); font-size: var(--fs-mono); line-height: 1.6;
      color: var(--code); white-space: pre-wrap; word-break: break-word; }
    pre.log:empty { display: none; }
  `);

  #agents = [];
  #runs = [];
  #agent = null;
  /** The open conversation; `null` is a new one, started by the next message. */
  #run = null;
  #peek = null;
  #limit = PAGE;
  /** Calls fetched for folded runs, by `runKey`. */
  #steps = new Map();
  #timer = 0;
  #busy = false;
  #answering = false;
  #live = false;
  /** Bumped on every switch, so an answer for a conversation the window has left is dropped. */
  #epoch = 0;

  template() {
    return `
      <div class="bar" part="bar" hidden>
        <select class="agent" aria-label="Agent"></select>
        <select class="conv" aria-label="Conversation"></select>
      </div>
      <div class="top" hidden>
        <div class="awaits"></div>
        <adi-composer></adi-composer>
      </div>
      <p class="error" role="alert"></p>
      <adi-transcript>
        <div class="lead" slot="lead"><adi-ask hidden></adi-ask><div class="queued"></div></div>
        <div slot="foot" class="foot">
          <div class="empty"></div>
          <pre class="log"></pre>
          <button class="earlier" type="button" hidden></button>
        </div>
      </adi-transcript>
    `;
  }

  setup() {
    this.$(".agent").addEventListener("change", (e) => this.#openAgent(e.target.value, undefined));
    this.$(".conv").addEventListener("change", (e) => this.#openRun(e.target.value || null));
    const composer = this.$("adi-composer");
    composer.addEventListener("send", (e) => this.#say(e.detail.text, "regular"));
    composer.addEventListener("asap", (e) => this.#say(e.detail.text, "asap"));
    composer.addEventListener("stop", () => this.#stop());
    this.$("adi-transcript").addEventListener("toggle", (e) => this.#toggleRun(e.detail));
    this.$("adi-transcript").addEventListener("unqueue", (e) => {
      const index = Number(e.target.dataset.place);
      if (Number.isInteger(index)) this.#unqueue(index);
    });
    this.$("adi-ask").addEventListener("answer", (e) => this.#answer(e.detail));
    this.$(".earlier").addEventListener("click", () => {
      this.#limit += PAGE;
      this.#tick();
    });
  }

  connectedCallback() {
    super.connectedCallback();
    this.#live = true;
    this.#start();
  }

  disconnectedCallback() {
    this.#live = false;
    window.clearTimeout(this.#timer);
  }

  attributeChangedCallback(name, old, value) {
    super.attributeChangedCallback();
    if (!this.#live || old === value) return;
    if (name === "agent" && value && value !== this.#agent) this.#openAgent(value, this.getAttribute("run"));
    else if (name === "run" && this.#agent && (value || null) !== this.#run) this.#openRun(value === "new" ? null : value);
    else if (name === "api") this.#start();
  }

  update() {
    this.$(".bar").hidden = !this.hasAttribute("picker");
  }

  // ---- the wire ----------------------------------------------------------------------------

  async #call(path, payload) {
    const res = await fetch(`${this.attr("api")}/api${path}`, {
      method: payload === undefined ? "GET" : "POST",
      headers: payload === undefined ? {} : { "content-type": "application/json" },
      body: payload === undefined ? undefined : JSON.stringify(payload),
    });
    const text = await res.text();
    let data = {};
    try {
      data = text ? JSON.parse(text) : {};
    } catch {
      data = { error: text };
    }
    if (!res.ok) throw new Error(data.error || `${res.status} ${res.statusText}`);
    return data;
  }

  #view() {
    return { limit: this.#limit, fold: true };
  }

  #fail(err) {
    this.$(".error").textContent = err instanceof Error ? err.message : String(err);
  }

  // ---- opening ----------------------------------------------------------------------------

  async #start() {
    try {
      const { agents } = await this.#call("/agents");
      this.#agents = (agents ?? []).slice().sort(
        (a, b) => Number(a.name !== ROOT_AGENT) - Number(b.name !== ROOT_AGENT) || a.name.localeCompare(b.name),
      );
      const select = this.$(".agent");
      select.replaceChildren(
        ...this.#agents.map((a) => {
          const o = document.createElement("option");
          o.value = a.name;
          o.textContent = a.name;
          return o;
        }),
      );
      const wanted = this.getAttribute("agent");
      const name = this.#agents.some((a) => a.name === wanted) ? wanted : this.#agents[0]?.name;
      if (!name) {
        this.$(".empty").textContent = "No agents on this machine yet.";
        return;
      }
      await this.#openAgent(name, this.getAttribute("run"));
    } catch (err) {
      this.#fail(err);
    }
  }

  async #openAgent(name, run) {
    this.#agent = name;
    this.$(".agent").value = name;
    this.#epoch += 1;
    const epoch = this.#epoch;
    try {
      const res = await this.#call("/agents/runs", { name });
      if (epoch !== this.#epoch) return;
      this.#runs = (res.runs ?? []).filter((r) => !r.hidden);
      this.#runs.sort((a, b) => (b.last_activity || b.started_at) - (a.last_activity || a.started_at));
      this.#drawRuns();
    } catch (err) {
      this.#fail(err);
    }
    let target = run === "new" ? null : run;
    if (run === undefined || run === null) {
      const saved = localStorage.getItem(`adi-chat:${name}`);
      target = this.#runs.some((r) => r.run_id === saved) ? saved : (this.#runs[0]?.run_id ?? null);
    }
    this.#openRun(target);
  }

  #openRun(run) {
    this.#epoch += 1;
    this.#run = run;
    this.#peek = null;
    this.#limit = PAGE;
    this.#steps.clear();
    this.$(".conv").value = run ?? "";
    this.$(".error").textContent = "";
    if (this.#agent) localStorage.setItem(`adi-chat:${this.#agent}`, run ?? "");
    this.$("adi-transcript").entries = [];
    this.emit("open", { agent: this.#agent, run });
    this.#draw();
    this.#tick();
  }

  #drawRuns() {
    const select = this.$(".conv");
    const fresh = document.createElement("option");
    fresh.value = "";
    fresh.textContent = "New conversation";
    select.replaceChildren(
      fresh,
      ...this.#runs.map((r) => {
        const o = document.createElement("option");
        o.value = r.run_id;
        const line = (r.title || r.message.split("\n").find((l) => l.trim()) || "Conversation").trim();
        o.textContent = line.length > 70 ? `${line.slice(0, 70)}…` : line;
        return o;
      }),
    );
    select.value = this.#run ?? "";
  }

  // ---- polling ----------------------------------------------------------------------------

  async #tick() {
    window.clearTimeout(this.#timer);
    if (!this.#live) return;
    const epoch = this.#epoch;
    if (this.#run && this.#agent) {
      try {
        const peek = await this.#call("/agents/run/peek", { name: this.#agent, run_id: this.#run, ...this.#view() });
        if (epoch !== this.#epoch) return;
        // A run that is not a conversation has no turns to fold — its log is what there is to show.
        if (!peek.answerable && !peek.turns?.length) {
          const whole = await this.#call("/agents/run/peek", { name: this.#agent, run_id: this.#run });
          if (epoch !== this.#epoch) return;
          peek.output = whole.output;
        }
        this.#peek = peek;
        this.$(".error").textContent = "";
        this.#draw();
      } catch (err) {
        if (epoch === this.#epoch) this.#fail(err);
      }
    }
    if (epoch !== this.#epoch || !this.#live) return;
    const p = this.#peek;
    const streaming = p?.running || p?.turns?.some((t) => t.pending || t.queued);
    this.#timer = window.setTimeout(() => this.#tick(), streaming ? FAST_MS : SLOW_MS);
  }

  // ---- drawing ----------------------------------------------------------------------------

  #draw() {
    const p = this.#peek;
    const fresh = !this.#run;
    const answerable = fresh || Boolean(p?.answerable);
    const composer = this.$("adi-composer");
    this.$(".top").hidden = !answerable;
    composer.setAttribute("placeholder", `Write to ${this.#agent ?? "the agent"}…`);
    composer.toggleAttribute("busy", this.#busy);
    composer.toggleAttribute("stoppable", Boolean(p?.running));
    composer.setAttribute("asap", "");

    this.#drawAwaits(p?.awaits ?? []);

    const ask = this.$("adi-ask");
    ask.ask = p?.pending_question ?? null;
    ask.hidden = !p?.pending_question;
    ask.toggleAttribute("busy", this.#answering);

    const turns = p?.turns ?? [];
    this.$(".queued").replaceChildren(
      ...turns
        .filter((t) => t.queued)
        .map((t, place) => {
          const m = document.createElement("adi-message");
          m.setAttribute("role", "user");
          m.setAttribute("queued", "");
          m.setAttribute("removable", "");
          m.dataset.place = String(place);
          if (t.mode === "asap") m.setAttribute("asap", "");
          const by = senderOf(t.markers);
          if (by) m.setAttribute("by", by);
          m.images = this.#pictures(t);
          m.body = t.text;
          return m;
        })
        .reverse(),
    );

    this.$("adi-transcript").entries = this.#entries(turns);

    const oldest = turns.length ? (turns[0].seq || 0) : 0;
    const earlier = this.$(".earlier");
    earlier.hidden = !(oldest > 0);
    earlier.textContent = oldest === 1 ? "1 earlier message" : `${oldest} earlier messages`;

    let empty = "";
    if (fresh) empty = `A new conversation with ${this.#agent ?? "the agent"} starts with what you write above.`;
    else if (!p) empty = "Loading…";
    else if (!turns.length && !p.output) empty = p.running ? "Working…" : "No output.";
    this.$(".empty").textContent = empty;
    this.$("pre.log").textContent = !turns.length ? (p?.output ?? "") : "";
  }

  #drawAwaits(awaits) {
    this.$(".awaits").replaceChildren(
      ...awaits.map((a) => {
        const row = document.createElement("div");
        row.className = "await";
        row.innerHTML = `<adi-icon name="bell" size="14"></adi-icon><span>Awaiting</span>
          <span class="what"></span><button type="button">Stop waiting</button>`;
        const what = row.querySelector(".what");
        what.textContent = a.note?.trim() || a.summary;
        what.title = a.summary + (a.check ? `\n\nchecks: ${a.check}` : "");
        row.querySelector("button").title =
          "Drop this wake. The chat stops waiting for it and stays where it is — nothing is cancelled at the other end.";
        row.querySelector("button").addEventListener("click", () => this.#ignoreAwait(a.id));
        return row;
      }),
    );
  }

  #pictures(turn) {
    return (turn.images ?? []).map((img) => ({
      url: `${this.attr("api")}/api/agents/attachment/${encodeURIComponent(img.id)}`,
      name: img.name || img.id,
      picture: String(img.media_type).startsWith("image/"),
    }));
  }

  /** The transcript's entries, oldest first. Queued turns are the lead's, but keep their index. */
  #entries(turns) {
    const out = [];
    turns.forEach((turn, position) => {
      if (turn.queued) return;
      const at = turn.seq || position;
      out.push(...this.#feedTurn(at, turn));
    });
    return out;
  }

  #feedTurn(at, turn) {
    const key = (part) => (part === 0 ? `adi-turn-${at}` : `adi-turn-${at}-${part}`);
    if (turn.role === "user") {
      const note = platformNote(turn);
      if (note) return [{ key: key(0), kind: "note", note }];
      return [{ key: key(0), kind: "said", role: "user", body: turn.text, images: this.#pictures(turn), by: senderOf(turn.markers) }];
    }
    const parts = [];
    let run = [];
    let runFrom = 0;
    const close = () => {
      if (run.length) parts.push({ kind: "did", run: loadedRun(`adi-run-${at}-${runFrom}`, run) });
      run = [];
    };
    (turn.steps ?? []).forEach((step, i) => {
      if (step.kind === "message") {
        close();
        if (step.text?.trim()) parts.push({ kind: "said", role: "agent", body: step.text });
      } else if (step.kind === "thinking" || step.kind === "tool") {
        if (!run.length) runFrom = i;
        run.push(step.kind === "tool" ? toolCall(at, i, step) : thinkingCall(step.text));
      } else if (step.kind === "calls") {
        close();
        const id = `adi-run-${at}-${step.from}`;
        const fetched = this.#steps.get(runKey(at, step.from, step.to, step.count, step.status));
        parts.push({
          kind: "did",
          run: {
            id,
            count: step.count,
            tools: step.tools ?? [],
            preview: step.preview ?? "",
            state: TOOL_STATE[step.status] ?? "ok",
            calls: fetched
              ? fetched
                  .map((s, offset) =>
                    s.kind === "tool" ? toolCall(at, step.from + offset, s) : s.kind === "thinking" ? thinkingCall(s.text) : null,
                  )
                  .filter(Boolean)
              : null,
          },
        });
      }
    });
    close();
    if (turn.text?.trim()) parts.push({ kind: "said", role: "agent", body: turn.text });
    return parts.map((part, i) => ({ key: key(i), ...part }));
  }

  // ---- acting -----------------------------------------------------------------------------

  /** A folded run was opened: fetch its calls, once per shape. */
  async #toggleRun({ id, open }) {
    if (!open || !this.#peek) return;
    for (const [position, turn] of (this.#peek.turns ?? []).entries()) {
      const at = turn.seq || position;
      for (const step of turn.steps ?? []) {
        if (step.kind !== "calls" || `adi-run-${at}-${step.from}` !== id) continue;
        const k = runKey(at, step.from, step.to, step.count, step.status);
        if (this.#steps.has(k)) return;
        const epoch = this.#epoch;
        try {
          const res = await this.#call("/agents/run/steps", {
            name: this.#agent, run_id: this.#run, turn: at, from: step.from, to: step.to,
          });
          if (epoch !== this.#epoch) return;
          this.#steps.set(k, res.steps ?? []);
          this.#draw();
        } catch {
          // Left on its "fetching" line; the next open tries again.
        }
        return;
      }
    }
  }

  async #say(text, mode) {
    if (this.#busy || !this.#agent) return;
    this.#busy = true;
    this.#draw();
    const epoch = this.#epoch;
    const composer = this.$("adi-composer");
    try {
      if (this.#run) {
        const peek = await this.#call("/agents/run/reply", {
          name: this.#agent, run_id: this.#run, message: text, mode, ...this.#view(),
        });
        composer.value = "";
        if (epoch === this.#epoch) this.#peek = peek;
      } else {
        const res = await this.#call("/agents/run", { name: this.#agent, message: text, launched_by: "human" });
        composer.value = "";
        if (res.run_id) {
          this.#busy = false;
          await this.#openAgent(this.#agent, res.run_id);
          return;
        }
      }
      this.$(".error").textContent = "";
    } catch (err) {
      this.#fail(err);
    } finally {
      this.#busy = false;
      this.#draw();
      composer.focus();
    }
    this.#tick();
  }

  async #stop() {
    if (!this.#run) return;
    try {
      await this.#call("/agents/run/stop", { name: this.#agent, run_id: this.#run });
    } catch (err) {
      this.#fail(err);
    }
    this.#tick();
  }

  async #unqueue(index) {
    try {
      const peek = await this.#call("/agents/run/unqueue", {
        name: this.#agent, run_id: this.#run, index, ...this.#view(),
      });
      this.#peek = peek;
      this.#draw();
    } catch (err) {
      this.#fail(err);
    }
  }

  async #answer({ id, replies }) {
    this.#answering = true;
    this.#draw();
    try {
      const peek = await this.#call("/agents/run/answer", {
        name: this.#agent, run_id: this.#run, ask: id, replies, ...this.#view(),
      });
      this.#peek = peek;
    } catch (err) {
      // A 404 is a question settled while the card sat open — the next read clears it.
      if (!/404|not found/i.test(String(err))) this.#fail(err);
    } finally {
      this.#answering = false;
      this.#draw();
    }
    this.#tick();
  }

  async #ignoreAwait(id) {
    try {
      await this.#call("/agents/await/ignore", { name: this.#agent, run_id: this.#run, id });
    } catch (err) {
      this.#fail(err);
    }
    this.#tick();
  }
}

define("adi-chat", AdiChat);

export { AdiChat };
