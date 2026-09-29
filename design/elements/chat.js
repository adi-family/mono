// `<adi-chat>` — a chat window, whole: pick an agent, pick one of its conversations, read it as it
// streams, answer it. Plain JavaScript over the panel's own agent API, so it works anywhere the
// panel's `/api` answers — the new UI's chat windows, the old chat, a widget, a page an agent writes.
//
//   <adi-chat picker windows></adi-chat>                    agent → its chats → one chat
//   <adi-chat agent="adi-agent" run="1790…-0000"></adi-chat>   one conversation, no chrome
//
// With `picker` the window moves in two steps. First the agent — the picker lists what the old
// chat's does: starred agents only (and whichever is open), the root agent first, a ● before one
// that is running — and under it every conversation it has, newest first. Then, a conversation
// opened, the window is fixed on it: the bar says which, and its one way out is back to the list.
//
// Attributes:
//   agent     which agent. Without it the one this browser last picked, else `adi-agent`.
//   node      the paired machine that agent is on (`hetzner-adi`); absent for this one. Everything
//             for it goes through this panel's `/api/node/<node>` forwarder.
//   run       which conversation; `new` for a fresh one. Without it, the list (`picker`), or the
//             agent's newest (no `picker`).
//   picker    the agent picker and the conversation list.
//   windows   the host can open a conversation in a window of its own: the list's right-click
//             menu offers "Open in new window", and the choice arrives as `open-window`.
//   api       where the panel answers. Default: this page's own origin.
//
// A picker lists this machine's agents and every unlocked paired machine's (`/api/fleet/nodes`),
// grouped by machine as the old chat's picker groups them.
//
// Events: `open` `{ node, agent, run }` whenever the conversation shown changes; `open-window`
// `{ node, agent, run, title }` when a conversation is asked for in a window of its own.
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
import "./mic.js";

/** A message carries at most this many files; the rest are refused with a sentence. */
const MAX_ATTACHMENTS = 6;
/** What a model can be *shown*; anything else reaches it as a path it opens. */
const PICTURES = ["image/png", "image/jpeg", "image/webp", "image/gif"];
const MAX_PICTURE = 5 * 1024 * 1024;
const MAX_FILE = 25 * 1024 * 1024;
/** Said instead of a paperclip when this conversation can be shown nothing — the panel's words. */
const IMAGES_REFUSED =
  "this one can't be sent a file — a terminal session takes typing, and a simulated run has no model to give one to";

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

/** A conversation's name in a list: its title, else the first line it was opened with. */
function titleOf(run) {
  if (!run) return "Conversation";
  const line = (run.title || run.message?.split("\n").find((l) => l.trim()) || "Conversation").trim();
  return line.length > 80 ? `${line.slice(0, 80)}…` : line;
}

/** How long ago, in the coarsest unit that still says something: `40s ago`, `12m ago`, `3h ago`. */
function ago(ms) {
  if (!ms) return "";
  const s = Math.max(0, Math.floor((Date.now() - ms) / 1000));
  if (s < 60) return `${s}s ago`;
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  return `${Math.floor(s / 86400)}d ago`;
}

class AdiChat extends AdiElement {
  static observedAttributes = ["agent", "run", "picker", "api", "node"];

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
    .goal { display: flex; align-items: center; gap: var(--s2); min-height: 28px;
      font-size: var(--fs-small); color: var(--ink-3); }
    .goal .text { min-width: 0; flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
      text-align: left; color: var(--ink-2); }
    .goal .text:hover { color: var(--ink); }
    .goal input { flex: 1; min-width: 0; padding: 5px 10px; border: 1px solid var(--line-strong);
      border-radius: var(--r); background: var(--bg-raise); color: var(--ink); font: inherit; font-size: var(--fs-ui); }
    .goal input::placeholder { color: var(--ink-3); }
    .link { color: var(--ink-3); transition: color var(--transition); }
    .link:hover { color: var(--ink-2); }
    .small { flex: none; padding: 3px 10px; border-radius: var(--r); font-size: var(--fs-small); color: var(--ink-2);
      transition: background var(--transition), color var(--transition); }
    .small:hover { background: var(--bg-hover); color: var(--ink); }
    .small.default { background: var(--btn); color: var(--ink); }
    .small.default:hover { background: var(--btn-hover); }
    .small.danger:hover { color: var(--err); }
    .small:disabled { opacity: .4; cursor: not-allowed; }
    .explain { font-size: var(--fs-label); color: var(--ink-3); }
    .note { min-height: 0; font-size: var(--fs-label); color: var(--ink-3); }
    .note:empty { display: none; }
    .crumb { display: flex; align-items: center; gap: var(--s2); min-width: 0; flex: 1; }
    .crumb[hidden], .pick[hidden] { display: none; }
    .back { flex: none; display: flex; align-items: center; gap: 4px; padding: 4px 8px 4px 4px;
      border-radius: var(--r); font-size: var(--fs-small); color: var(--ink-2);
      transition: background var(--transition), color var(--transition); }
    .back:hover { background: var(--bg-hover); color: var(--ink); }
    .crumb .title { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
      font-size: var(--fs-ui-sm); font-weight: 500; }
    .pick { display: flex; align-items: center; gap: var(--s2); flex: 1; min-width: 0; }
    .pickhint { font-size: var(--fs-small); color: var(--ink-3); }
    .pickhint:empty { display: none; }
    .chat { display: flex; flex-direction: column; flex: 1; min-height: 0; }
    .chat[hidden], .list[hidden] { display: none; }
    .list { flex: 1; min-height: 0; overflow-y: auto; padding: var(--s3) var(--s2); }
    .fresh { display: flex; align-items: center; gap: 8px; width: 100%; padding: 7px 8px; border-radius: var(--r);
      font-size: var(--fs-ui-sm); color: var(--ink-2); transition: background var(--transition), color var(--transition); }
    .fresh:hover { background: var(--bg-hover); color: var(--ink); }
    .rows { display: flex; flex-direction: column; margin-top: var(--s2); }
    .row { display: flex; align-items: center; gap: 8px; width: 100%; padding: 7px 8px; border-radius: var(--r);
      text-align: left; transition: background var(--transition); }
    .row:hover { background: var(--bg-hover); }
    .row .text { display: flex; flex-direction: column; gap: 2px; min-width: 0; flex: 1; }
    .row .name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: var(--fs-ui-sm); color: var(--ink); }
    .row .meta { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: var(--fs-label); color: var(--ink-3); }
    .row .star { flex: none; color: var(--ink-3); }
    .live { flex: none; width: 6px; height: 6px; border-radius: 50%; background: var(--accent); }
    .none { padding: 7px 8px; font-size: var(--fs-small); color: var(--ink-3); }
    :host { position: relative; }
    .menu { position: absolute; z-index: 20; min-width: 168px; padding: 4px; border: 1px solid var(--line-strong);
      border-radius: var(--r-lg); background: var(--bg-raise); }
    .menu[hidden] { display: none; }
    .menu button { display: block; width: 100%; padding: 6px 8px; border-radius: var(--r); text-align: left;
      font-size: var(--fs-ui-sm); color: var(--ink); }
    .menu button:hover { background: var(--bg-hover); }
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

  /** Every machine's agents: `[{ node, agents }]`, this machine (`node: null`) first. */
  #sources = [];
  /** The paired machine the open agent is on; `null` for this one. */
  #node = null;
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
  /** This conversation's goals, open and closed, and the editor's state over them. */
  #goals = [];
  #goalEditor = null;
  #goalBusy = false;
  #goalsDrawn = "";
  /** What the next message carries: `{ key, name, preview, image, state, id, error }`. */
  #files = [];
  /** Whether the last read had a turn running — its end is when goals are worth re-reading. */
  #wasRunning = false;
  #runsDrawn = "";
  /** `list` — an agent's conversations — or `chat`, one of them. */
  #mode = "list";
  /** Bumped on every switch, so an answer for a conversation the window has left is dropped. */
  #epoch = 0;

  template() {
    return `
      <div class="bar" part="bar" hidden>
        <div class="pick">
          <select class="agent" aria-label="Agent"
            title="which agent this chat runs on — starred agents only"></select>
          <span class="pickhint"></span>
        </div>
        <div class="crumb" hidden>
          <button class="back" type="button" title="Every conversation with this agent">
            <adi-icon name="chevron-left" size="16"></adi-icon><span class="to"></span>
          </button>
          <span class="title"></span>
        </div>
      </div>
      <div class="list" hidden>
        <button class="fresh" type="button"><adi-icon name="message-square-plus" size="16"></adi-icon>New conversation</button>
        <div class="rows" role="list"></div>
      </div>
      <div class="menu" role="menu" hidden></div>
      <div class="chat">
      <div class="top" hidden>
        <div class="goals"></div>
        <div class="awaits"></div>
        <adi-composer attach><adi-mic slot="tools"></adi-mic></adi-composer>
        <div class="note"></div>
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
      </div>
    `;
  }

  setup() {
    this.$(".agent").addEventListener("change", (e) => {
      const [node, name] = JSON.parse(e.target.value);
      localStorage.setItem("adi-chat:agent", JSON.stringify({ node, name }));
      this.#openAgent(name, undefined, node);
    });
    this.$(".back").addEventListener("click", () => this.#showList());
    this.$(".fresh").addEventListener("click", () => this.#openRun(null));
    // Any click anywhere closes the menu, the one that chose from it included.
    this.shadowRoot.addEventListener("click", () => (this.$(".menu").hidden = true));
    this.addEventListener("contextmenu", (ev) => {
      if (!ev.composedPath().some((n) => n.classList?.contains("row"))) this.$(".menu").hidden = true;
    });
    const composer = this.$("adi-composer");
    composer.addEventListener("send", (e) => this.#say(e.detail.text, "regular"));
    composer.addEventListener("asap", (e) => this.#say(e.detail.text, "asap"));
    composer.addEventListener("stop", () => this.#stop());
    composer.addEventListener("files", (e) => this.#attach(e.detail.files));
    composer.addEventListener("unattach", (e) => {
      this.#files = this.#files.filter((f) => f.key !== e.detail.key);
      this.#drawFiles();
    });
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
    if ((name === "agent" || name === "node") && this.getAttribute("agent")) {
      const node = this.getAttribute("node") || null;
      const agent = this.getAttribute("agent");
      if (agent !== this.#agent || node !== this.#node) this.#openAgent(agent, this.getAttribute("run"), node);
    }
    else if (name === "run" && this.#agent && (value || null) !== this.#run) this.#openRun(value === "new" ? null : value);
    else if (name === "api") this.#start();
  }

  update() {
    const picker = this.hasAttribute("picker");
    this.$(".bar").hidden = !picker;
    // Without a picker there is no list to be on: the element is the conversation it was given.
    if (!picker) this.#mode = "chat";
    this.#drawMode();
    this.$("adi-mic").setAttribute("api", this.attr("api"));
  }

  // ---- the wire ----------------------------------------------------------------------------

  async #call(path, payload) {
    const res = await fetch(`${this.#base()}/api${path}`, {
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

  /** Where the open agent's machine answers: this panel, or its forwarder to a paired one. */
  #base() {
    return `${this.attr("api")}${this.#node ? `/api/node/${encodeURIComponent(this.#node)}` : ""}`;
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
      const saved = this.#savedAgent();
      const wanted = this.getAttribute("agent")
        ? { node: this.getAttribute("node") || null, name: this.getAttribute("agent") }
        : (saved ?? { node: null, name: ROOT_AGENT });
      this.#node = wanted.node;
      const local = await this.#readAgents(null);
      this.#sources = [{ node: null, agents: local }];
      let agents = local;
      if (wanted.node) {
        agents = await this.#readAgents(wanted.node).catch(() => []);
        this.#sources.push({ node: wanted.node, agents });
      }
      let name = agents.some((a) => a.name === wanted.name) ? wanted.name : null;
      if (!name) {
        // What was asked for is not there — a machine gone, an agent deleted: this machine's root.
        this.#node = null;
        name = (local.find((a) => a.name === ROOT_AGENT) ?? local[0])?.name;
      }
      if (!name) {
        this.$(".empty").textContent = "No agents on this machine yet.";
        return;
      }
      if (this.hasAttribute("picker")) this.#readFleet();
      await this.#openAgent(name, this.getAttribute("run"), this.#node);
    } catch (err) {
      this.#fail(err);
    }
  }

  /** One machine's agents — this one for `null`. */
  async #readAgents(node) {
    const base = `${this.attr("api")}${node ? `/api/node/${encodeURIComponent(node)}` : ""}`;
    const res = await fetch(`${base}/api/agents`, { signal: AbortSignal.timeout(15_000) });
    if (!res.ok) throw new Error(`${res.status}`);
    return (await res.json()).agents ?? [];
  }

  /**
   * Every unlocked paired machine's agents, added to the picker as each answers. A locked machine
   * asks for a login this window cannot give, and one that does not answer is left out rather than
   * holding the others back.
   */
  async #readFleet() {
    let nodes = [];
    try {
      const res = await fetch(`${this.attr("api")}/api/fleet/nodes`);
      nodes = res.ok ? ((await res.json()).nodes ?? []) : [];
    } catch {
      return;
    }
    await Promise.allSettled(
      nodes
        .filter((n) => !n.locked)
        .map(async ({ node }) => {
          const agents = await this.#readAgents(node);
          const at = this.#sources.findIndex((s) => s.node === node);
          if (at >= 0) this.#sources[at] = { node, agents };
          else this.#sources.push({ node, agents });
          this.#sources.sort((a, b) => (a.node === null ? -1 : b.node === null ? 1 : a.node.localeCompare(b.node)));
          this.#drawAgents();
        }),
    );
  }

  /** The agent this browser last picked: `{ node, name }` (a bare name, from before machines). */
  #savedAgent() {
    const raw = localStorage.getItem("adi-chat:agent");
    if (!raw) return null;
    try {
      const v = JSON.parse(raw);
      return typeof v === "string" ? { node: null, name: v } : { node: v.node ?? null, name: v.name };
    } catch {
      return { node: null, name: raw };
    }
  }

  /**
   * The agent picker, as the old chat's (`chat_agent_picker`): each machine's starred agents, and
   * whichever is open whether starred or not; the root agent first; a ● before one that is running,
   * since an option carries no markup for a dot. With more than one machine the list is grouped by
   * machine — an `<optgroup>` each — and which machine the chosen one is on is said beside it, since
   * a closed select shows the option and never its group.
   */
  #drawAgents() {
    const select = this.$(".agent");
    const multi = this.#sources.length > 1;
    const groups = this.#sources
      .map(({ node, agents }) => {
        const options = agents.filter(
          (a) => a.starred || (a.name === this.#agent && node === this.#node),
        );
        options.sort((a, b) => Number(!(node === null && a.name === ROOT_AGENT)) - Number(!(node === null && b.name === ROOT_AGENT)));
        return { node, options };
      })
      .filter((g) => g.options.length);
    const option = (node, a) => {
      const o = document.createElement("option");
      o.value = JSON.stringify([node, a.name]);
      o.textContent = a.running ? `\u25CF ${a.name}` : a.name;
      return o;
    };
    select.replaceChildren(
      ...groups.map(({ node, options }) => {
        if (!multi) return options.map((a) => option(node, a));
        const g = document.createElement("optgroup");
        g.label = node ?? "This machine";
        g.append(...options.map((a) => option(node, a)));
        return [g];
      }).flat(),
    );
    select.value = JSON.stringify([this.#node, this.#agent]);
    const any = groups.length > 0;
    select.hidden = !any;
    this.$(".pickhint").textContent = !any ? "No starred agents" : multi ? `on ${this.#node ?? "this machine"}` : "";
  }

  async #openAgent(name, run, node = this.#node) {
    this.#agent = name;
    this.#node = node ?? null;
    this.#drawAgents();
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
    if (run === "new") return this.#openRun(null);
    if (run) return this.#openRun(run);
    // No conversation named: with a picker, whatever this window was on when it was last left —
    // the conversation, if it is still there, else the list; without a picker there is nothing to
    // choose with, so the newest.
    if (this.hasAttribute("picker")) {
      const was = this.#remembered();
      if (was?.agent === name && (was.node ?? null) === this.#node && this.#runs.some((r) => r.run_id === was.run)) {
        return this.#openRun(was.run);
      }
      return this.#showList();
    }
    this.#openRun(this.#runs[0]?.run_id ?? null);
  }

  /**
   * What a picker window was on, kept per browser so a reload lands back there: `{ agent, run }`,
   * `run` null for the list. Only a picker window keeps it — one given its conversation by its host
   * is that conversation whatever was open last.
   */
  #remembered() {
    try {
      return JSON.parse(localStorage.getItem("adi-chat:open") ?? "null");
    } catch {
      return null;
    }
  }

  #remember() {
    if (!this.hasAttribute("picker") || !this.#agent) return;
    localStorage.setItem("adi-chat:open", JSON.stringify({ node: this.#node, agent: this.#agent, run: this.#run }));
  }

  /** Back to the agent's conversations — the window lets go of the one it was fixed on. */
  #showList() {
    this.#epoch += 1;
    this.#mode = "list";
    this.#run = null;
    this.#remember();
    this.#peek = null;
    this.$("adi-transcript").entries = [];
    this.emit("open", { node: this.#node, agent: this.#agent, run: null });
    this.#drawMode();
    this.#drawRuns();
    this.#tick();
  }

  #drawMode() {
    const list = this.#mode === "list";
    this.$(".list").hidden = !list;
    this.$(".chat").hidden = list;
    this.$(".pick").hidden = !list;
    this.$(".crumb").hidden = list;
    if (!list) {
      const run = this.#runs.find((r) => r.run_id === this.#run);
      this.$(".crumb .title").textContent = this.#run ? titleOf(run) : "New conversation";
      this.$(".crumb .to").textContent = this.#node ? `${this.#agent} on ${this.#node}` : (this.#agent ?? "");
    }
  }

  #openRun(run) {
    this.#epoch += 1;
    this.#mode = "chat";
    this.$(".menu").hidden = true;
    this.#run = run;
    this.#remember();
    this.#peek = null;
    this.#limit = PAGE;
    this.#steps.clear();
    this.#goals = [];
    this.#goalEditor = null;
    this.#loadGoals();
    this.$(".error").textContent = "";
    this.$("adi-transcript").entries = [];
    this.#drawMode();
    this.emit("open", { node: this.#node, agent: this.#agent, run });
    this.#draw();
    this.#tick();
  }

  /** The agent's conversations, newest first — the list a window is on before it opens one. */
  #drawRuns() {
    const box = this.$(".rows");
    if (!this.#runs.length) {
      box.innerHTML = `<div class="none">No conversations with this agent yet.</div>`;
      return;
    }
    const sig = JSON.stringify(this.#runs.map((r) => [r.run_id, r.title, r.running, r.starred, r.last_activity]));
    if (sig === this.#runsDrawn) return;
    this.#runsDrawn = sig;
    box.replaceChildren(
      ...this.#runs.map((r) => {
        const row = document.createElement("button");
        row.type = "button";
        row.className = "row";
        row.setAttribute("role", "listitem");
        row.innerHTML = `${r.running ? `<span class="live" aria-label="running"></span>` : ""}
          <span class="text"><span class="name"></span><span class="meta"></span></span>
          ${r.starred ? `<adi-icon class="star" name="star" size="14" label="Starred"></adi-icon>` : ""}`;
        row.querySelector(".name").textContent = titleOf(r);
        row.querySelector(".meta").textContent = r.running ? "working" : ago(r.last_activity || r.started_at);
        row.addEventListener("click", () => this.#openRun(r.run_id));
        row.addEventListener("contextmenu", (ev) => {
          ev.preventDefault();
          this.#menu(ev, r);
        });
        return row;
      }),
    );
  }

  /** A conversation's right-click menu: open it here, or — where the host has windows — in one of its own. */
  #menu(ev, run) {
    const menu = this.$(".menu");
    menu.replaceChildren();
    const item = (label, act) => {
      const b = document.createElement("button");
      b.type = "button";
      b.setAttribute("role", "menuitem");
      b.textContent = label;
      b.addEventListener("click", act);
      menu.append(b);
    };
    item("Open", () => this.#openRun(run.run_id));
    if (this.hasAttribute("windows")) {
      item("Open in new window", () =>
        this.emit("open-window", { node: this.#node, agent: this.#agent, run: run.run_id, title: titleOf(run) }),
      );
    }
    // Placed against this element rather than the screen: a host that is itself a blurred window
    // is the containing block of anything fixed inside it, and the menu would land beside the click.
    const box = this.getBoundingClientRect();
    menu.hidden = false;
    const x = Math.min(ev.clientX - box.left, box.width - menu.offsetWidth - 4);
    const y = Math.min(ev.clientY - box.top, box.height - menu.offsetHeight - 4);
    menu.style.left = `${Math.max(4, x)}px`;
    menu.style.top = `${Math.max(4, y)}px`;
  }

  // ---- polling ----------------------------------------------------------------------------

  async #tick() {
    window.clearTimeout(this.#timer);
    if (!this.#live) return;
    const epoch = this.#epoch;
    // On the list, the list is what moves: a conversation starts, another finishes.
    if (this.#mode === "list" && this.#agent) {
      try {
        const res = await this.#call("/agents/runs", { name: this.#agent });
        if (epoch !== this.#epoch) return;
        this.#runs = (res.runs ?? []).filter((r) => !r.hidden);
        this.#runs.sort((a, b) => (b.last_activity || b.started_at) - (a.last_activity || a.started_at));
        this.#drawRuns();
      } catch {
        // The next tick asks again.
      }
    }
    if (this.#mode === "chat" && this.#run && this.#agent) {
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
        // A turn that just ended is when a goal is likeliest to have moved — the agent closes its
        // own, or sets one — so that is when they are read again, rather than on every poll.
        if (this.#wasRunning && !peek.running) this.#loadGoals();
        this.#wasRunning = Boolean(peek.running);
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
    // Whether the open conversation's own engine can be handed a file — its capability profile,
    // not the agent's current settings, because a conversation is answered by whatever started it.
    const attach = fresh || Boolean(p?.caps?.images);
    composer.toggleAttribute("attach", attach);
    composer.setAttribute("refusal", IMAGES_REFUSED);
    // A message sent mid-answer is queued, not refused — and the line under the box says so.
    this.$(".note").textContent = p?.running ? "queued — the agent is answering" : "";

    this.#drawGoals();

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
      url: `${this.#base()}/api/agents/attachment/${encodeURIComponent(img.id)}`,
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
    const attachments = this.#files.filter((f) => f.state === "ready").map((f) => f.id);
    this.#busy = true;
    this.#draw();
    const epoch = this.#epoch;
    const composer = this.$("adi-composer");
    try {
      if (this.#run) {
        const peek = await this.#call("/agents/run/reply", {
          name: this.#agent, run_id: this.#run, message: text, mode, attachments, ...this.#view(),
        });
        composer.value = "";
        this.#clearFiles();
        if (epoch === this.#epoch) this.#peek = peek;
      } else {
        const res = await this.#call("/agents/run", {
          name: this.#agent, message: text, attachments, launched_by: "human",
        });
        composer.value = "";
        this.#clearFiles();
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

  // ---- goals ------------------------------------------------------------------------------
  // What this conversation is *for*: its open goals, each with the two ways out, and a link to set
  // one — `goal_bar` in the panel. Above the composer rather than in the transcript, because a goal
  // is a standing condition on the whole conversation, not a thing said in it. Closed goals are not
  // drawn: the transcript carries the turn that met them.

  async #loadGoals() {
    if (!this.#run || !this.#agent) return this.#drawGoals();
    const [epoch, run] = [this.#epoch, this.#run];
    try {
      const res = await this.#call("/agents/goals", { name: this.#agent, run_id: run });
      if (epoch !== this.#epoch) return;
      this.#goals = res.goals ?? [];
      this.#drawGoals();
    } catch {
      // A panel too old to keep goals has none to show; the chat is no worse for it.
    }
  }

  #drawGoals() {
    const box = this.$(".goals");
    // A goal belongs to a conversation, so a new one has nowhere to put it yet.
    if (!this.#run) {
      this.#goalsDrawn = "";
      return box.replaceChildren();
    }
    const editor = this.#goalEditor;
    if (editor) {
      if (box.querySelector("input")) return this.#goalButtons();
      box.innerHTML = `
        <div class="goal">
          <input type="text" placeholder="what would make this chat done">
          <button class="small default save" type="button">Save</button>
          <button class="small cancel" type="button">Cancel</button>
        </div>
        <div class="explain">Put back to the agent every time this chat falls quiet, until it is met or given up on.</div>`;
      const input = box.querySelector("input");
      input.value = editor.text;
      input.addEventListener("input", () => {
        editor.text = input.value;
        this.#goalButtons();
      });
      // Enter saves and Escape closes: this opened under the cursor, and asking for the mouse back
      // to dismiss a one-line box is the annoying half of a popover.
      input.addEventListener("keydown", (ev) => {
        if (ev.key === "Enter" && !ev.isComposing) this.#saveGoal();
        else if (ev.key === "Escape") this.#closeGoalEditor();
      });
      box.querySelector(".save").addEventListener("click", () => this.#saveGoal());
      box.querySelector(".cancel").addEventListener("click", () => this.#closeGoalEditor());
      this.#goalButtons();
      input.focus();
      return;
    }
    const open = this.#goals.filter((g) => g.state === "open");
    const sig = JSON.stringify([open, this.#goalBusy]);
    if (sig === this.#goalsDrawn && box.firstChild) return;
    this.#goalsDrawn = sig;
    if (!open.length) {
      // Nothing set is the normal case, so it costs one quiet line.
      box.innerHTML = `<div class="goal"><button class="link" type="button"
        title="Set what would make this chat done. It is put back to the agent every time the chat falls quiet, until it is met or given up on.">+ Set a goal</button></div>`;
      box.querySelector(".link").addEventListener("click", () => this.#openGoalEditor(null, ""));
      return;
    }
    box.replaceChildren(
      ...open.map((goal) => {
        const row = document.createElement("div");
        row.className = "goal";
        const self = goal.set_by === "agent";
        row.innerHTML = `
          <span title="This chat has a goal">Goal</span>
          <button class="text" type="button"></button>
          ${self ? `<span title="The agent set this goal for itself">self-set</span>` : ""}
          ${goal.nudges > 1 ? `<span>asked ${goal.nudges}×</span>` : ""}
          <button class="small met" type="button" title="Close this goal as met">Met</button>
          <button class="small danger gave" type="button" title="Stop working toward this goal, and stop being asked about it">Give up</button>`;
        const text = row.querySelector(".text");
        text.textContent = goal.text;
        // The sentence is the edit control: the obvious thing to do with one you disagree with is click it.
        text.title = `${goal.text} — click to reword${self ? " (the agent set this itself)" : ""}`;
        text.addEventListener("click", () => this.#openGoalEditor(goal.id, goal.text));
        row.querySelector(".met").addEventListener("click", () => this.#closeGoal(goal.id, "met"));
        row.querySelector(".gave").addEventListener("click", () => this.#closeGoal(goal.id, "given_up"));
        for (const b of row.querySelectorAll(".small")) b.disabled = this.#goalBusy;
        return row;
      }),
    );
  }

  #goalButtons() {
    const box = this.$(".goals");
    const save = box.querySelector(".save");
    if (save) save.disabled = this.#goalBusy || !this.#goalEditor?.text.trim();
    const input = box.querySelector("input");
    if (input) input.disabled = this.#goalBusy;
  }

  #openGoalEditor(id, text) {
    this.#goalEditor = { id, text };
    this.#goalsDrawn = "";
    this.$(".goals").replaceChildren();
    this.#drawGoals();
  }

  #closeGoalEditor() {
    this.#goalEditor = null;
    this.#goalsDrawn = "";
    this.#drawGoals();
  }

  async #saveGoal() {
    const editor = this.#goalEditor;
    if (!editor || !editor.text.trim() || this.#goalBusy) return;
    this.#goalBusy = true;
    this.#goalButtons();
    try {
      const res = await this.#call("/agents/goal/set", {
        name: this.#agent, run_id: this.#run, text: editor.text, goal: editor.id,
      });
      this.#goals = res.goals ?? [];
      // Closed only on success: a goal the server refused is still in the box, where it can be fixed.
      this.#goalEditor = null;
      this.#goalsDrawn = "";
    } catch (err) {
      this.#fail(err);
    } finally {
      this.#goalBusy = false;
      this.#drawGoals();
    }
  }

  async #closeGoal(goal, as) {
    this.#goalBusy = true;
    this.#drawGoals();
    try {
      const res = await this.#call("/agents/goal/close", { goal, as_: as, note: "" });
      this.#goals = res.goals ?? [];
    } catch (err) {
      this.#fail(err);
    } finally {
      this.#goalBusy = false;
      this.#drawGoals();
    }
  }

  // ---- attachments ------------------------------------------------------------------------
  // `crate::attach` in the panel: each file is stored the moment it is attached
  // (`POST /api/agents/attachment`, raw bytes, its name in a header), and the message carries the
  // ids. A picture is shown to the model; anything else reaches it as a path it opens.

  #attach(files) {
    for (const file of files) {
      if (this.#files.length >= MAX_ATTACHMENTS) {
        this.#fail(`A message can carry ${MAX_ATTACHMENTS} attachments; the rest were left out.`);
        break;
      }
      const type = file.type.trim() || "application/octet-stream";
      const image = PICTURES.includes(type);
      if (file.size > (image ? MAX_PICTURE : MAX_FILE)) {
        this.#fail(`“${file.name}” is larger than ${image ? "5 MB" : "25 MB"} — it was left out.`);
        continue;
      }
      const name = file.name.trim() || (image ? "pasted image" : "pasted file");
      const entry = {
        key: `attach-${Date.now()}-${this.#files.length}`,
        name,
        image,
        preview: image ? URL.createObjectURL(file) : "",
        state: "uploading",
      };
      this.#files = [...this.#files, entry];
      this.#upload(entry, file, type);
    }
    this.#drawFiles();
  }

  async #upload(entry, file, type) {
    try {
      const res = await fetch(`${this.#base()}/api/agents/attachment`, {
        method: "POST",
        // The name travels in a header because the body is the file. Headers are Latin-1, and a
        // screenshot's name routinely is not — so anything else becomes `_`.
        headers: { "content-type": type, "x-adi-filename": entry.name.replace(/[^\x20-\x7e]/g, "_") },
        body: file,
      });
      const data = await res.json().catch(() => ({}));
      if (!res.ok) throw new Error(data.error || `${res.status}`);
      if (entry.preview) URL.revokeObjectURL(entry.preview);
      Object.assign(entry, {
        state: "ready",
        id: data.id,
        preview: entry.image ? `${this.#base()}/api/agents/attachment/${encodeURIComponent(data.id)}` : "",
      });
    } catch (err) {
      // The row stays, marked failed, so it can be removed deliberately — and the error says why.
      Object.assign(entry, { state: "failed", error: err.message });
      this.#fail(`${entry.name}: ${err.message}`);
    }
    this.#files = [...this.#files];
    this.#drawFiles();
  }

  #drawFiles() {
    this.$("adi-composer").attachments = this.#files;
  }

  #clearFiles() {
    for (const f of this.#files) if (f.preview.startsWith("blob:")) URL.revokeObjectURL(f.preview);
    this.#files = [];
    this.#drawFiles();
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
