// Attributes: agent, node (paired machine), run (`new` for a fresh conversation), picker, windows, api.
// Events: `place` and `open-window` carry `{ node, agent, run, title }`.
// The host persists the location through attributes; each chat instance is independent.

import { AdiElement, define, detail } from "./base.ts";
import type { AdiTranscript, Call, CallRun, CallState, Note, NotePart, Picture, TranscriptEntry as Entry, TranscriptPart as Part } from "./transcript.ts";
import "./transcript.ts";
import type { AdiComposer, ComposerAttachment } from "./composer.ts";
import "./composer.ts";
import type { AdiAsk, AskQuestion } from "./ask.ts";
import "./ask.ts";
import "./icon.ts";
import "./mic.ts";

const MAX_ATTACHMENTS = 6;
const PICTURES = ["image/png", "image/jpeg", "image/webp", "image/gif"];
const MAX_PICTURE = 5 * 1024 * 1024;
const MAX_FILE = 25 * 1024 * 1024;
const IMAGES_REFUSED =
  "this one can't be sent a file — a terminal session takes typing, and a simulated run has no model to give one to";

const ROOT_AGENT = "adi-agent";
const PAGE = 20;
const FAST_MS = 1000;
const SLOW_MS = 5000;

// Wire types mirror crates/adi-webapp-api/src/types.rs; responses are not revalidated here.
/** One agent on a machine, from `GET /api/agents` — `AgentDto`. */
interface AgentDto {
  name: string;
  starred: boolean;
  running: boolean;
}

/** A paired machine, from `GET /api/fleet/nodes` — `FleetNodeAccess`. */
interface FleetNodeAccess {
  node: string;
  locked: boolean;
}

/** One conversation in an agent's list, from `/agents/runs` — `AgentRunInfo`. */
interface AgentRunInfo {
  run_id: string;
  started_at: number;
  last_activity: number;
  message: string;
  title?: string | null;
  running: boolean;
  hidden: boolean;
  starred: boolean;
}

type ToolStatus = "running" | "ok" | "error" | "unanswered";

/** One step of an agent's turn — `AgentStep`, tagged by `kind`. */
type AgentStep =
  | { kind: "message"; text: string }
  | { kind: "thinking"; text: string }
  | { kind: "tool"; name: string; input: string; status: ToolStatus; output: string }
  | { kind: "calls"; from: number; to: number; count: number; tools: string[]; preview: string; status: ToolStatus }
  | { kind: "unknown" };

type ToolStep = Extract<AgentStep, { kind: "tool" }>;
type FoldedCallsStep = Extract<AgentStep, { kind: "calls" }>;

/** What the platform stamped on a turn — `TurnMarker`, tagged by `kind`. */
type TurnMarker =
  | { kind: "from"; node: string; user: string }
  | { kind: "await-woken"; id: string; cause: string; event: string; check: boolean }
  | { kind: "ask-answered"; id: string; by: string }
  | { kind: "goal-check"; open: number }
  | { kind: "pre-run"; ran: number; dropped: number }
  | { kind: "unknown" };

interface AgentAttachment {
  id: string;
  name: string;
  media_type: string;
  size: number;
}

interface AgentTurn {
  role: string;
  text: string;
  seq: number;
  pending: boolean;
  queued: boolean;
  mode?: "regular" | "asap";
  images: AgentAttachment[];
  steps: AgentStep[];
  markers?: TurnMarker[];
}

interface AgentAwait {
  id: string;
  note: string;
  summary: string;
  check?: string;
}

interface AgentAsk {
  id: string;
  headline: string;
  questions: AskQuestion[];
}

/** A snapshot of one conversation, from `/agents/run/peek` and every reply — `AgentPeek`. */
interface AgentPeek {
  running: boolean;
  output: string;
  answerable: boolean;
  caps: { images: boolean };
  pending_question?: AgentAsk;
  awaits?: AgentAwait[];
  turns: AgentTurn[];
}

interface AgentGoal {
  id: string;
  text: string;
  state: string;
  set_by: string;
  nudges: number;
}

interface GoalEditor {
  id: string | null;
  text: string;
}

interface Attachment extends ComposerAttachment {
  image: boolean;
  preview: string;
  id?: string;
}

/** `send` and `asap` from the composer. */
type Said = { text: string };
/** `toggle` from the transcript — a run of calls (with its `id`), or a lone tool call (without). */
type Toggled = { id?: string; open: boolean };
/** `answer` from the question card. */
type Answered = { id?: string; replies: string[] };

/** One machine's agents, as the picker groups them; `node` is `null` for this one. */
interface Source {
  node: string | null;
  agents: AgentDto[];
}

const TOOL_STATE: Record<ToolStatus, CallState> = { running: "running", ok: "ok", error: "failed", unanswered: "unanswered" };

function paramsOf(input: string): [string, string][] {
  try {
    const value = JSON.parse(input);
    if (value && typeof value === "object" && !Array.isArray(value)) {
      return Object.entries(value).map(([k, v]) => [k, typeof v === "string" ? v : JSON.stringify(v, null, 2)]);
    }
  } catch {
  }
  return [["input", input]];
}

function previewOf(call: Call): string {
  return (call.params?.[0]?.[1] ?? "").split(/\s+/).filter(Boolean).join(" ");
}

function toolCall(turn: number, step: number, s: ToolStep): Call {
  return {
    name: s.name,
    params: paramsOf(s.input ?? ""),
    state: TOOL_STATE[s.status] ?? "ok",
    result: s.output || null,
    anchor: `adi-step-${turn}-${step}`,
  };
}

function thinkingCall(text: string): Call {
  return { name: "thinking", params: [["text", text]], state: "ok", result: null, anchor: null };
}

function loadedRun(id: string, calls: Call[]): CallRun {
  const tools = [...new Set(calls.map((c) => c.name))];
  const head = calls.find((c) => c.state === "running") ?? calls[calls.length - 1];
  return { id, count: calls.length, tools, preview: head ? previewOf(head) : "", state: head?.state ?? "ok", calls };
}

function runKey(turn: number, from: number, to: number, count: number, status: ToolStatus): string {
  return `adi-run-${turn}-${from}:${to}:${count}:${status}`;
}

function senderOf(markers: TurnMarker[] = []): string | null {
  const from = markers.find((m) => m.kind === "from");
  if (!from) return null;
  return from.user ? `${from.node}/${from.user}` : from.node;
}

function platformNote(turn: AgentTurn): Note | null {
  const sender = senderOf(turn.markers);
  for (const m of turn.markers ?? []) {
    if (m.kind === "await-woken") {
      let head: NotePart[];
      if (m.cause === "event" && m.event) head = [{ text: "Woken by" }, { code: m.event }];
      else if (m.cause === "event") head = [{ text: "Woken by an event" }];
      else if (m.cause === "expired") head = [{ text: "Expired without firing" }];
      else head = [{ text: "Woken by the clock" }];
      if (m.check) head.push({ text: "· check passed" });
      return { icon: "bell", head, id: m.id, body: turn.text };
    }
    if (m.kind === "ask-answered") {
      const head: NotePart[] = [{ text: m.by === "default" ? "Answered by default" : "Answered" }];
      if (sender) head.push({ text: `· ${sender}` });
      return { icon: "check", head, id: m.id, body: turn.text };
    }
    if (m.kind === "goal-check") {
      const head: NotePart[] = [{ text: "Goal check" }];
      if (m.open > 0) head.push({ text: `· ${m.open} open` });
      return { icon: "flag", head, id: null, body: turn.text };
    }
  }
  return null;
}

function titleOf(run: AgentRunInfo | undefined): string {
  if (!run) return "Conversation";
  const line = (run.title || run.message?.split("\n").find((l) => l.trim()) || "Conversation").trim();
  return line.length > 80 ? `${line.slice(0, 80)}…` : line;
}

function ago(ms: number): string {
  if (!ms) return "";
  const s = Math.max(0, Math.floor((Date.now() - ms) / 1000));
  if (s < 60) return `${s}s ago`;
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  return `${Math.floor(s / 86400)}d ago`;
}

class AdiChat extends AdiElement {
  static observedAttributes = ["agent", "run", "picker", "api", "node"];

  #sources: Source[] = [];
  #node: string | null = null;
  #runs: AgentRunInfo[] = [];
  #agent: string | null = null;
  #run: string | null = null;
  #peek: AgentPeek | null = null;
  #limit = PAGE;
  #steps = new Map<string, AgentStep[]>();
  #timer = 0;
  #busy = false;
  #answering = false;
  #live = false;
  #goals: AgentGoal[] = [];
  #goalEditor: GoalEditor | null = null;
  #goalBusy = false;
  #goalsDrawn = "";
  #files: Attachment[] = [];
  #wasRunning = false;
  #runsDrawn = "";
  #mode: "list" | "chat" = "list";
  /** Bumped on every switch, so an answer for a conversation the window has left is dropped. */
  #epoch = 0;

  override template(): string {
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

  override setup(): void {
    const select = this.must<HTMLSelectElement>(".agent");
    select.addEventListener("change", () => {
      const [node, name] = JSON.parse(select.value) as [string | null, string];
      this.#openAgent(name, undefined, node);
    });
    this.must(".back").addEventListener("click", () => this.#showList());
    this.must(".fresh").addEventListener("click", () => this.#openRun(null));
    this.shadowRoot.addEventListener("click", () => (this.must(".menu").hidden = true));
    this.addEventListener("contextmenu", (ev) => {
      if (!ev.composedPath().some((n) => n instanceof Element && n.classList.contains("row"))) this.must(".menu").hidden = true;
    });
    const composer = this.must<AdiComposer>("adi-composer");
    composer.addEventListener("send", (e) => this.#say(detail<Said>(e).text, "regular"));
    composer.addEventListener("asap", (e) => this.#say(detail<Said>(e).text, "asap"));
    composer.addEventListener("stop", () => this.#stop());
    composer.addEventListener("files", (e) => this.#attach(detail<{ files: File[] }>(e).files));
    composer.addEventListener("unattach", (e) => {
      const { key } = detail<{ key: string }>(e);
      this.#files = this.#files.filter((f) => f.key !== key);
      this.#drawFiles();
    });
    const transcript = this.must<AdiTranscript>("adi-transcript");
    transcript.addEventListener("toggle", (e) => this.#toggleRun(detail<Toggled>(e)));
    transcript.addEventListener("unqueue", (e) => {
      const index = Number((e.target as HTMLElement).dataset.place);
      if (Number.isInteger(index)) this.#unqueue(index);
    });
    this.must<AdiAsk>("adi-ask").addEventListener("answer", (e) => this.#answer(detail<Answered>(e)));
    this.must(".earlier").addEventListener("click", () => {
      this.#limit += PAGE;
      this.#tick();
    });
  }

  override connectedCallback(): void {
    super.connectedCallback();
    this.#live = true;
    this.#start();
  }

  disconnectedCallback(): void {
    this.#live = false;
    window.clearTimeout(this.#timer);
  }

  override attributeChangedCallback(name: string, old: string | null, value: string | null): void {
    super.attributeChangedCallback();
    if (!this.#live || old === value) return;
    if (name === "api") {
      this.#start();
      return;
    }
    if (name === "node" || name === "agent" || name === "run") this.#followSoon();
  }

  /** Batch attribute changes to avoid requesting a new agent on the previous machine. */
  #followSoon(): void {
    if (this.#following) return;
    this.#following = true;
    queueMicrotask(() => {
      this.#following = false;
      const agent = this.getAttribute("agent");
      if (!agent || !this.#agent) return;
      const node = this.getAttribute("node") || null;
      const run = this.getAttribute("run") || null;
      if (agent !== this.#agent || node !== this.#node) return this.#openAgent(agent, run, node);
      if (run === "new") return this.#openRun(null);
      if (run !== this.#run) return run ? this.#openRun(run) : this.#showList();
    });
  }

  #following = false;

  override update(): void {
    const picker = this.hasAttribute("picker");
    this.must(".bar").hidden = !picker;
    if (!picker) this.#mode = "chat";
    this.#drawMode();
    this.must("adi-mic").setAttribute("api", this.attr("api"));
  }

  async #call<T = unknown>(path: string, payload?: object): Promise<T> {
    const res = await fetch(`${this.#base()}/api${path}`, {
      method: payload === undefined ? "GET" : "POST",
      headers: payload === undefined ? {} : { "content-type": "application/json" },
      body: payload === undefined ? undefined : JSON.stringify(payload),
    });
    const text = await res.text();
    let data: unknown = {};
    try {
      data = text ? JSON.parse(text) : {};
    } catch {
      data = { error: text };
    }
    if (!res.ok) throw new Error((data as { error?: string }).error || `${res.status} ${res.statusText}`);
    return data as T;
  }

  #base(): string {
    return `${this.attr("api")}${this.#node ? `/api/node/${encodeURIComponent(this.#node)}` : ""}`;
  }

  #view(): { limit: number; fold: boolean } {
    return { limit: this.#limit, fold: true };
  }

  #fail(err: unknown): void {
    this.must(".error").textContent = err instanceof Error ? err.message : String(err);
  }

  async #start(): Promise<void> {
    try {
      const wanted = this.getAttribute("agent")
        ? { node: this.getAttribute("node") || null, name: this.getAttribute("agent") }
        : { node: null, name: ROOT_AGENT };
      this.#node = wanted.node;
      const local = await this.#readAgents(null);
      this.#sources = [{ node: null, agents: local }];
      let agents = local;
      if (wanted.node) {
        agents = await this.#readAgents(wanted.node).catch((): AgentDto[] => {
          this.#fail(`${wanted.node} is not answering`);
          return [];
        });
        this.#sources.push({ node: wanted.node, agents });
      }
      let name = this.getAttribute("agent") || null;
      if (!name) {
        name = agents.some((a) => a.name === wanted.name)
          ? wanted.name
          : (local.find((a) => a.name === ROOT_AGENT) ?? local[0])?.name;
      }
      if (!name) {
        this.must(".empty").textContent = "No agents on this machine yet.";
        return;
      }
      if (this.hasAttribute("picker")) this.#readFleet();
      await this.#openAgent(name, this.getAttribute("run"), this.#node);
    } catch (err) {
      this.#fail(err);
    }
  }

  /** One machine's agents — this one for `null`. */
  async #readAgents(node: string | null): Promise<AgentDto[]> {
    const base = `${this.attr("api")}${node ? `/api/node/${encodeURIComponent(node)}` : ""}`;
    const res = await fetch(`${base}/api/agents`, { signal: AbortSignal.timeout(15_000) });
    if (!res.ok) throw new Error(`${res.status}`);
    return ((await res.json()) as { agents?: AgentDto[] }).agents ?? [];
  }

  /** Load unlocked paired machines independently; unavailable machines do not block the picker. */
  async #readFleet(): Promise<void> {
    let nodes: FleetNodeAccess[] = [];
    try {
      const res = await fetch(`${this.attr("api")}/api/fleet/nodes`);
      nodes = res.ok ? (((await res.json()) as { nodes?: FleetNodeAccess[] }).nodes ?? []) : [];
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

  /** Group starred agents and the current agent by machine, with the root agent first. */
  #drawAgents(): void {
    const select = this.must<HTMLSelectElement>(".agent");
    const multipleMachines = this.#sources.length > 1;
    const choices: (HTMLOptionElement | HTMLOptGroupElement)[] = [];
    for (const { node, agents } of this.#sources) {
      const visibleAgents = agents.filter(
        (agent) => agent.starred || (agent.name === this.#agent && node === this.#node),
      );
      if (!visibleAgents.length) continue;
      visibleAgents.sort((left, right) => {
        const leftIsRoot = node === null && left.name === ROOT_AGENT;
        const rightIsRoot = node === null && right.name === ROOT_AGENT;
        return Number(rightIsRoot) - Number(leftIsRoot);
      });

      const options = visibleAgents.map((agent) => {
        const option = document.createElement("option");
        option.value = JSON.stringify([node, agent.name]);
        option.textContent = agent.running ? `\u25CF ${agent.name}` : agent.name;
        return option;
      });
      if (multipleMachines) {
        const group = document.createElement("optgroup");
        group.label = node ?? "This machine";
        group.append(...options);
        choices.push(group);
      } else {
        choices.push(...options);
      }
    }

    select.replaceChildren(...choices);
    select.value = JSON.stringify([this.#node, this.#agent]);
    select.hidden = choices.length === 0;
    const hint = this.must(".pickhint");
    if (!choices.length) hint.textContent = "No starred agents";
    else if (multipleMachines) hint.textContent = `on ${this.#node ?? "this machine"}`;
    else hint.textContent = "";
  }

  async #openAgent(name: string, run?: string | null, node: string | null = this.#node): Promise<void> {
    this.#agent = name;
    this.#node = node ?? null;
    this.#drawAgents();
    this.#epoch += 1;
    const epoch = this.#epoch;
    try {
      const res = await this.#call<{ runs?: AgentRunInfo[] }>("/agents/runs", { name });
      if (epoch !== this.#epoch) return;
      this.#runs = (res.runs ?? []).filter((r) => !r.hidden);
      this.#runs.sort((a, b) => (b.last_activity || b.started_at) - (a.last_activity || a.started_at));
      this.#drawRuns();
    } catch (err) {
      this.#fail(err);
    }
    if (run === "new") return this.#openRun(null);
    if (run) return this.#openRun(run);
    if (this.hasAttribute("picker")) return this.#showList();
    this.#openRun(this.#runs[0]?.run_id ?? null);
  }

  #showList(): void {
    this.#epoch += 1;
    this.#mode = "list";
    this.#run = null;
    this.#peek = null;
    this.must<AdiTranscript>("adi-transcript").entries = [];
    this.emit("place", { node: this.#node, agent: this.#agent, run: null, title: "" });
    this.#drawMode();
    this.#drawRuns();
    this.#tick();
  }

  #drawMode(): void {
    const list = this.#mode === "list";
    this.must(".list").hidden = !list;
    this.must(".chat").hidden = list;
    this.must(".pick").hidden = !list;
    this.must(".crumb").hidden = list;
    if (!list) {
      const run = this.#runs.find((r) => r.run_id === this.#run);
      this.must(".crumb .title").textContent = this.#run ? titleOf(run) : "New conversation";
      this.must(".crumb .to").textContent = this.#node ? `${this.#agent} on ${this.#node}` : (this.#agent ?? "");
    }
  }

  #openRun(run: string | null): void {
    this.#epoch += 1;
    this.#mode = "chat";
    this.must(".menu").hidden = true;
    this.#run = run;
    this.#peek = null;
    this.#limit = PAGE;
    this.#steps.clear();
    this.#goals = [];
    this.#goalEditor = null;
    this.#loadGoals();
    this.must(".error").textContent = "";
    this.must<AdiTranscript>("adi-transcript").entries = [];
    this.#drawMode();
    const shown = this.#runs.find((r) => r.run_id === run);
    this.emit("place", { node: this.#node, agent: this.#agent, run, title: run ? titleOf(shown) : "" });
    this.#draw();
    this.#tick();
  }

  #drawRuns(): void {
    const box = this.must(".rows");
    if (!this.#runs.length) {
      box.innerHTML = `<div class="none">No conversations with this agent yet.</div>`;
      return;
    }
    const signature = JSON.stringify(
      this.#runs.map((run) => [run.run_id, run.title, run.running, run.starred, run.last_activity]),
    );
    if (signature === this.#runsDrawn) return;
    this.#runsDrawn = signature;
    box.replaceChildren(...this.#runs.map((run) => this.#runRow(run)));
  }

  #runRow(run: AgentRunInfo): HTMLButtonElement {
    const row = document.createElement("button");
    row.type = "button";
    row.className = "row";
    row.setAttribute("role", "listitem");
    row.innerHTML = `${run.running ? `<span class="live" aria-label="running"></span>` : ""}
      <span class="text"><span class="name"></span><span class="meta"></span></span>
      ${run.starred ? `<adi-icon class="star" name="star" size="14" label="Starred"></adi-icon>` : ""}`;
    row.querySelector(".name")!.textContent = titleOf(run);
    row.querySelector(".meta")!.textContent = run.running ? "working" : ago(run.last_activity || run.started_at);
    row.addEventListener("click", () => this.#openRun(run.run_id));
    row.addEventListener("contextmenu", (event) => {
      event.preventDefault();
      this.#menu(event, run);
    });
    return row;
  }

  #menu(ev: MouseEvent, run: AgentRunInfo): void {
    const menu = this.must(".menu");
    menu.replaceChildren();
    const item = (label: string, act: () => void) => {
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
    // Position relative to this host because a blurred ancestor contains fixed descendants.
    const box = this.getBoundingClientRect();
    menu.hidden = false;
    const x = Math.min(ev.clientX - box.left, box.width - menu.offsetWidth - 4);
    const y = Math.min(ev.clientY - box.top, box.height - menu.offsetHeight - 4);
    menu.style.left = `${Math.max(4, x)}px`;
    menu.style.top = `${Math.max(4, y)}px`;
  }

  async #tick(): Promise<void> {
    window.clearTimeout(this.#timer);
    if (!this.#live) return;
    const epoch = this.#epoch;
    if (this.#mode === "list" && this.#agent) {
      try {
        const res = await this.#call<{ runs?: AgentRunInfo[] }>("/agents/runs", { name: this.#agent });
        if (epoch !== this.#epoch) return;
        this.#runs = (res.runs ?? []).filter((r) => !r.hidden);
        this.#runs.sort((a, b) => (b.last_activity || b.started_at) - (a.last_activity || a.started_at));
        this.#drawRuns();
      } catch {
      }
    }
    if (this.#mode === "chat" && this.#run && this.#agent) {
      try {
        const peek = await this.#call<AgentPeek>("/agents/run/peek", { name: this.#agent, run_id: this.#run, ...this.#view() });
        if (epoch !== this.#epoch) return;
        // Non-conversation runs expose logs rather than turns.
        if (!peek.answerable && !peek.turns?.length) {
          const whole = await this.#call<AgentPeek>("/agents/run/peek", { name: this.#agent, run_id: this.#run });
          if (epoch !== this.#epoch) return;
          peek.output = whole.output;
        }
        this.#peek = peek;
        this.must(".error").textContent = "";
        // Refresh goals when a turn ends, not on every poll.
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

  #draw(): void {
    const peek = this.#peek;
    const fresh = !this.#run;
    const answerable = fresh || Boolean(peek?.answerable);
    const composer = this.must<AdiComposer>("adi-composer");
    this.must(".top").hidden = !answerable;
    composer.setAttribute("placeholder", `Write to ${this.#agent ?? "the agent"}…`);
    composer.toggleAttribute("busy", this.#busy);
    composer.toggleAttribute("stoppable", Boolean(peek?.running));
    composer.setAttribute("asap", "");
    // Use the conversation engine’s capabilities, which may differ from current agent settings.
    const attach = fresh || Boolean(peek?.caps?.images);
    composer.toggleAttribute("attach", attach);
    composer.setAttribute("refusal", IMAGES_REFUSED);
    this.must(".note").textContent = peek?.running ? "queued — the agent is answering" : "";

    this.#drawGoals();
    this.#drawAwaits(peek?.awaits ?? []);

    const ask = this.must<AdiAsk>("adi-ask");
    ask.ask = peek?.pending_question ?? null;
    ask.hidden = !peek?.pending_question;
    ask.toggleAttribute("busy", this.#answering);

    const turns = peek?.turns ?? [];
    this.#drawQueued(turns);
    this.must<AdiTranscript>("adi-transcript").entries = this.#entries(turns);

    const oldest = turns.length ? (turns[0].seq || 0) : 0;
    const earlier = this.must(".earlier");
    earlier.hidden = !(oldest > 0);
    earlier.textContent = oldest === 1 ? "1 earlier message" : `${oldest} earlier messages`;

    let empty = "";
    if (fresh) empty = `A new conversation with ${this.#agent ?? "the agent"} starts with what you write above.`;
    else if (!peek) empty = "Loading…";
    else if (!turns.length && !peek.output) empty = peek.running ? "Working…" : "No output.";
    this.must(".empty").textContent = empty;
    this.must("pre.log").textContent = !turns.length ? (peek?.output ?? "") : "";
  }

  #drawQueued(turns: AgentTurn[]): void {
    const queuedTurns = turns.filter((turn) => turn.queued);
    const messages = queuedTurns.map((turn, queueIndex) => {
      const message = document.createElement("adi-message");
      message.setAttribute("role", "user");
      message.setAttribute("queued", "");
      message.setAttribute("removable", "");
      message.dataset.place = String(queueIndex);
      if (turn.mode === "asap") message.setAttribute("asap", "");
      const sender = senderOf(turn.markers);
      if (sender) message.setAttribute("by", sender);
      message.images = this.#pictures(turn);
      message.body = turn.text;
      return message;
    });
    // Show the newest first while preserving the queue index used to remove it.
    this.must(".queued").replaceChildren(...messages.reverse());
  }

  #drawAwaits(awaits: AgentAwait[]): void {
    this.must(".awaits").replaceChildren(
      ...awaits.map((a) => {
        const row = document.createElement("div");
        row.className = "await";
        row.innerHTML = `<adi-icon name="bell" size="14"></adi-icon><span>Awaiting</span>
          <span class="what"></span><button type="button">Stop waiting</button>`;
        const what = row.querySelector<HTMLElement>(".what")!;
        what.textContent = a.note?.trim() || a.summary;
        what.title = a.summary + (a.check ? `\n\nchecks: ${a.check}` : "");
        const stop = row.querySelector("button")!;
        stop.title =
          "Drop this wake. The chat stops waiting for it and stays where it is — nothing is cancelled at the other end.";
        stop.addEventListener("click", () => this.#ignoreAwait(a.id));
        return row;
      }),
    );
  }

  #pictures(turn: AgentTurn): Picture[] {
    return (turn.images ?? []).map((img) => ({
      url: `${this.#base()}/api/agents/attachment/${encodeURIComponent(img.id)}`,
      name: img.name || img.id,
      picture: String(img.media_type).startsWith("image/"),
    }));
  }

  /** The transcript's entries, oldest first. Queued turns are the lead's, but keep their index. */
  #entries(turns: AgentTurn[]): Entry[] {
    const out: Entry[] = [];
    turns.forEach((turn, position) => {
      if (turn.queued) return;
      const at = turn.seq || position;
      out.push(...this.#feedTurn(at, turn));
    });
    return out;
  }

  #feedTurn(turnIndex: number, turn: AgentTurn): Entry[] {
    const key = (partIndex: number) => (
      partIndex === 0 ? `adi-turn-${turnIndex}` : `adi-turn-${turnIndex}-${partIndex}`
    );
    if (turn.role === "user") {
      const note = platformNote(turn);
      if (note) return [{ key: key(0), kind: "note", note }];
      return [{ key: key(0), kind: "said", role: "user", body: turn.text, images: this.#pictures(turn), by: senderOf(turn.markers) }];
    }

    const parts: Part[] = [];
    let calls: Call[] = [];
    let firstCallIndex = 0;
    const finishCalls = () => {
      if (calls.length) {
        const id = `adi-run-${turnIndex}-${firstCallIndex}`;
        parts.push({ kind: "did", run: loadedRun(id, calls) });
      }
      calls = [];
    };

    for (const [stepIndex, step] of (turn.steps ?? []).entries()) {
      switch (step.kind) {
        case "message":
          finishCalls();
          if (step.text?.trim()) parts.push({ kind: "said", role: "agent", body: step.text });
          break;
        case "thinking":
        case "tool":
          if (!calls.length) firstCallIndex = stepIndex;
          calls.push(step.kind === "tool" ? toolCall(turnIndex, stepIndex, step) : thinkingCall(step.text));
          break;
        case "calls":
          finishCalls();
          parts.push({ kind: "did", run: this.#foldedCallRun(turnIndex, step) });
          break;
      }
    }

    finishCalls();
    if (turn.text?.trim()) parts.push({ kind: "said", role: "agent", body: turn.text });
    return parts.map((part, partIndex) => ({ key: key(partIndex), ...part }));
  }

  #foldedCallRun(turnIndex: number, step: FoldedCallsStep): CallRun {
    const cacheKey = runKey(turnIndex, step.from, step.to, step.count, step.status);
    const fetchedSteps = this.#steps.get(cacheKey);
    let calls: Call[] | null = null;
    if (fetchedSteps) {
      calls = [];
      for (const [offset, fetchedStep] of fetchedSteps.entries()) {
        if (fetchedStep.kind === "tool") calls.push(toolCall(turnIndex, step.from + offset, fetchedStep));
        else if (fetchedStep.kind === "thinking") calls.push(thinkingCall(fetchedStep.text));
      }
    }

    return {
      id: `adi-run-${turnIndex}-${step.from}`,
      count: step.count,
      tools: step.tools ?? [],
      preview: step.preview ?? "",
      state: TOOL_STATE[step.status] ?? "ok",
      calls,
    };
  }

  /** A folded run was opened: fetch its calls, once per shape. */
  async #toggleRun({ id, open }: Toggled): Promise<void> {
    if (!open || !this.#peek) return;
    for (const [position, turn] of (this.#peek.turns ?? []).entries()) {
      const at = turn.seq || position;
      for (const step of turn.steps ?? []) {
        if (step.kind !== "calls" || `adi-run-${at}-${step.from}` !== id) continue;
        const k = runKey(at, step.from, step.to, step.count, step.status);
        if (this.#steps.has(k)) return;
        const epoch = this.#epoch;
        try {
          const res = await this.#call<{ steps?: AgentStep[] }>("/agents/run/steps", {
            name: this.#agent, run_id: this.#run, turn: at, from: step.from, to: step.to,
          });
          if (epoch !== this.#epoch) return;
          this.#steps.set(k, res.steps ?? []);
          this.#draw();
        } catch {
          // Keep the loading line; opening again retries the fetch.
        }
        return;
      }
    }
  }

  async #say(text: string, mode: "regular" | "asap"): Promise<void> {
    if (this.#busy || !this.#agent) return;
    const attachments = this.#files.filter((f) => f.state === "ready").map((f) => f.id);
    this.#busy = true;
    this.#draw();
    const epoch = this.#epoch;
    const composer = this.must<AdiComposer>("adi-composer");
    try {
      if (this.#run) {
        const peek = await this.#call<AgentPeek>("/agents/run/reply", {
          name: this.#agent, run_id: this.#run, message: text, mode, attachments, ...this.#view(),
        });
        composer.value = "";
        this.#clearFiles();
        if (epoch === this.#epoch) this.#peek = peek;
      } else {
        const res = await this.#call<{ run_id?: string }>("/agents/run", {
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
      this.must(".error").textContent = "";
    } catch (err) {
      this.#fail(err);
    } finally {
      this.#busy = false;
      this.#draw();
      composer.focus();
    }
    this.#tick();
  }

  async #stop(): Promise<void> {
    if (!this.#run) return;
    try {
      await this.#call("/agents/run/stop", { name: this.#agent, run_id: this.#run });
    } catch (err) {
      this.#fail(err);
    }
    this.#tick();
  }

  async #unqueue(index: number): Promise<void> {
    try {
      const peek = await this.#call<AgentPeek>("/agents/run/unqueue", {
        name: this.#agent, run_id: this.#run, index, ...this.#view(),
      });
      this.#peek = peek;
      this.#draw();
    } catch (err) {
      this.#fail(err);
    }
  }

  async #answer({ id, replies }: Answered): Promise<void> {
    this.#answering = true;
    this.#draw();
    try {
      const peek = await this.#call<AgentPeek>("/agents/run/answer", {
        name: this.#agent, run_id: this.#run, ask: id, replies, ...this.#view(),
      });
      this.#peek = peek;
    } catch (err) {
      // A 404 means the question was settled; the next poll clears it.
      if (!/404|not found/i.test(String(err))) this.#fail(err);
    } finally {
      this.#answering = false;
      this.#draw();
    }
    this.#tick();
  }

  async #loadGoals(): Promise<void> {
    if (!this.#run || !this.#agent) return this.#drawGoals();
    const [epoch, run] = [this.#epoch, this.#run];
    try {
      const res = await this.#call<{ goals?: AgentGoal[] }>("/agents/goals", { name: this.#agent, run_id: run });
      if (epoch !== this.#epoch) return;
      this.#goals = res.goals ?? [];
      this.#drawGoals();
    } catch {
      // Older panels may not support goals.
    }
  }

  #drawGoals(): void {
    const box = this.must(".goals");
    if (!this.#run) {
      this.#goalsDrawn = "";
      return box.replaceChildren();
    }
    const editor = this.#goalEditor;
    if (editor) {
      this.#drawGoalEditor(box, editor);
      return;
    }

    const openGoals = this.#goals.filter((goal) => goal.state === "open");
    const signature = JSON.stringify([openGoals, this.#goalBusy]);
    if (signature === this.#goalsDrawn && box.firstChild) return;
    this.#goalsDrawn = signature;
    if (!openGoals.length) {
      box.innerHTML = `<div class="goal"><button class="link" type="button"
        title="Set what would make this chat done. It is put back to the agent every time the chat falls quiet, until it is met or given up on.">+ Set a goal</button></div>`;
      box.querySelector(".link")!.addEventListener("click", () => this.#openGoalEditor(null, ""));
      return;
    }
    box.replaceChildren(...openGoals.map((goal) => this.#goalRow(goal)));
  }

  #drawGoalEditor(box: HTMLElement, editor: GoalEditor): void {
    // Polling must keep the existing input and its selection intact.
    if (box.querySelector("input")) return this.#goalButtons();
    box.innerHTML = `
      <div class="goal">
        <input type="text" placeholder="what would make this chat done">
        <button class="small default save" type="button">Save</button>
        <button class="small cancel" type="button">Cancel</button>
      </div>
      <div class="explain">Put back to the agent every time this chat falls quiet, until it is met or given up on.</div>`;
    const input = box.querySelector("input")!;
    input.value = editor.text;
    input.addEventListener("input", () => {
      editor.text = input.value;
      this.#goalButtons();
    });
    input.addEventListener("keydown", (event) => {
      if (event.key === "Enter" && !event.isComposing) this.#saveGoal();
      else if (event.key === "Escape") this.#closeGoalEditor();
    });
    box.querySelector(".save")!.addEventListener("click", () => this.#saveGoal());
    box.querySelector(".cancel")!.addEventListener("click", () => this.#closeGoalEditor());
    this.#goalButtons();
    input.focus();
  }

  #goalRow(goal: AgentGoal): HTMLDivElement {
    const row = document.createElement("div");
    row.className = "goal";
    const setByAgent = goal.set_by === "agent";
    row.innerHTML = `
      <span title="This chat has a goal">Goal</span>
      <button class="text" type="button"></button>
      ${setByAgent ? `<span title="The agent set this goal for itself">self-set</span>` : ""}
      ${goal.nudges > 1 ? `<span>asked ${goal.nudges}×</span>` : ""}
      <button class="small met" type="button" title="Close this goal as met">Met</button>
      <button class="small danger gave" type="button" title="Stop working toward this goal, and stop being asked about it">Give up</button>`;
    const text = row.querySelector<HTMLButtonElement>(".text")!;
    text.textContent = goal.text;
    text.title = `${goal.text} — click to reword${setByAgent ? " (the agent set this itself)" : ""}`;
    text.addEventListener("click", () => this.#openGoalEditor(goal.id, goal.text));
    row.querySelector(".met")!.addEventListener("click", () => this.#closeGoal(goal.id, "met"));
    row.querySelector(".gave")!.addEventListener("click", () => this.#closeGoal(goal.id, "given_up"));
    for (const button of row.querySelectorAll<HTMLButtonElement>(".small")) button.disabled = this.#goalBusy;
    return row;
  }

  #goalButtons(): void {
    const box = this.must(".goals");
    const save = box.querySelector<HTMLButtonElement>(".save");
    if (save) save.disabled = this.#goalBusy || !this.#goalEditor?.text.trim();
    const input = box.querySelector("input");
    if (input) input.disabled = this.#goalBusy;
  }

  #openGoalEditor(id: string | null, text: string): void {
    this.#goalEditor = { id, text };
    this.#goalsDrawn = "";
    this.must(".goals").replaceChildren();
    this.#drawGoals();
  }

  #closeGoalEditor(): void {
    this.#goalEditor = null;
    this.#goalsDrawn = "";
    this.#drawGoals();
  }

  async #saveGoal(): Promise<void> {
    const editor = this.#goalEditor;
    if (!editor || !editor.text.trim() || this.#goalBusy) return;
    this.#goalBusy = true;
    this.#goalButtons();
    try {
      const res = await this.#call<{ goals?: AgentGoal[] }>("/agents/goal/set", {
        name: this.#agent, run_id: this.#run, text: editor.text, goal: editor.id,
      });
      this.#goals = res.goals ?? [];
      // Keep rejected goal text available for correction.
      this.#goalEditor = null;
      this.#goalsDrawn = "";
    } catch (err) {
      this.#fail(err);
    } finally {
      this.#goalBusy = false;
      this.#drawGoals();
    }
  }

  async #closeGoal(goal: string, as: "met" | "given_up"): Promise<void> {
    this.#goalBusy = true;
    this.#drawGoals();
    try {
      const res = await this.#call<{ goals?: AgentGoal[] }>("/agents/goal/close", { goal, as_: as, note: "" });
      this.#goals = res.goals ?? [];
    } catch (err) {
      this.#fail(err);
    } finally {
      this.#goalBusy = false;
      this.#drawGoals();
    }
  }

  #attach(files: File[]): void {
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
      const entry: Attachment = {
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

  async #upload(entry: Attachment, file: File, type: string): Promise<void> {
    try {
      const res = await fetch(`${this.#base()}/api/agents/attachment`, {
        method: "POST",
        // Headers require Latin-1; replace unsupported filename characters.
        headers: { "content-type": type, "x-adi-filename": entry.name.replace(/[^\x20-\x7e]/g, "_") },
        body: file,
      });
      const data = (await res.json().catch(() => ({}))) as { id?: string; error?: string };
      if (!res.ok || !data.id) throw new Error(data.error || `${res.status}`);
      if (entry.preview) URL.revokeObjectURL(entry.preview);
      entry.state = "ready";
      entry.id = data.id;
      entry.preview = entry.image ? `${this.#base()}/api/agents/attachment/${encodeURIComponent(data.id)}` : "";
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      entry.state = "failed";
      entry.error = message;
      this.#fail(`${entry.name}: ${message}`);
    }
    this.#files = [...this.#files];
    this.#drawFiles();
  }

  #drawFiles(): void {
    this.must<AdiComposer>("adi-composer").attachments = this.#files;
  }

  #clearFiles(): void {
    for (const f of this.#files) if (f.preview.startsWith("blob:")) URL.revokeObjectURL(f.preview);
    this.#files = [];
    this.#drawFiles();
  }

  async #ignoreAwait(id: string): Promise<void> {
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

declare global {
  interface HTMLElementTagNameMap {
    "adi-chat": AdiChat;
  }
}
