// The chat: talk to one of this machine's agents — pick the agent, pick a conversation or start a
// new one, read it as it streams, answer it.
//
// One module, two places. The board's page mounts it as a panel (the default export, which the
// shell calls with its context); `widget.html` imports `chat()` and mounts it across the whole
// frame, which is how the chat reaches the ADI home screen. Every read goes to this app's own
// backend (`backend/routes/*`), which relays it to the control panel.
//
// It polls: quickly while an answer is streaming, slowly when nothing is, and repaints only what
// changed — a signature of the transcript is compared before the DOM is touched.

interface Agent {
  name: string;
  runnable?: boolean;
}

interface RunRow {
  run_id: string;
  started_at: number;
  last_activity?: number;
  message: string;
}

interface Step {
  kind?: string;
  name?: string;
  text?: string;
  status?: string;
}

interface Turn {
  role: string;
  text: string;
  pending?: boolean;
  queued?: boolean;
  steps?: Step[];
}

interface Peek {
  running?: boolean;
  answerable?: boolean;
  turns?: Turn[];
}

/** The agent a chat opens on when this browser has not picked one. */
const DEFAULT_AGENT = "adi-agent";
const FAST_MS = 1500;
const SLOW_MS = 8000;

const STYLE = `
.ab-chat { display:flex; flex-direction:column; height:100%; min-height:0; color:var(--ink);
  font:14px/1.5 var(--sans); font-feature-settings:"tnum"; }
.ab-chat__bar { flex:none; display:flex; gap:var(--s2); padding:var(--s3) var(--s4);
  border-bottom:1px solid var(--line); }
.ab-chat select { min-width:0; flex:1; font:inherit; font-size:13px; color:var(--ink);
  background:var(--bg-raise); border:1px solid var(--line-strong); border-radius:var(--r);
  padding:5px 8px; }
.ab-chat select.ab-chat__agent { flex:0 1 160px; }
.ab-chat__log { flex:1; min-height:0; overflow-y:auto; padding:var(--s4);
  display:flex; flex-direction:column; gap:var(--s4); }
.ab-chat__empty { margin:auto; color:var(--ink-3); font-size:13px; }
.ab-turn.is-user { align-self:flex-end; max-width:85%; padding:var(--s2) var(--s3);
  background:var(--chip); border-radius:var(--r-lg); white-space:pre-wrap; }
.ab-turn.is-agent { max-width:100%; }
.ab-turn.is-agent p { margin:0 0 var(--s2); white-space:pre-wrap; overflow-wrap:anywhere; }
.ab-turn.is-agent p:last-child { margin-bottom:0; }
.ab-turn .ab-heading { font-weight:600; }
.ab-turn strong { font-weight:600; }
.ab-turn pre { margin:0 0 var(--s2); padding:var(--s2) var(--s3); overflow-x:auto;
  background:var(--bg-raise); border:1px solid var(--line); border-radius:8px;
  font:12.5px/1.6 var(--mono); color:var(--code); }
.ab-turn code { font:12.5px var(--mono); color:var(--code); background:var(--chip);
  padding:1px 5px; border-radius:var(--r-sm); }
.ab-turn pre code { background:none; padding:0; }
.ab-calls { margin:0 0 var(--s2); padding:4px var(--s2); border:1px solid var(--line);
  border-radius:8px; font-size:12px; color:var(--ink-3); white-space:nowrap; overflow:hidden;
  text-overflow:ellipsis; }
.ab-note { font-size:12px; color:var(--ink-3); }
.ab-chat__foot { flex:none; padding:var(--s3) var(--s4) var(--s4); }
.ab-compose { display:flex; align-items:flex-end; gap:var(--s2); padding:var(--s2);
  background:var(--bg-raise); border:1px solid var(--line-strong); border-radius:var(--r-lg); }
.ab-compose textarea { flex:1; min-height:22px; max-height:160px; resize:none; border:0;
  outline:none; background:none; color:var(--ink); font:inherit; padding:5px 6px; }
.ab-compose textarea::placeholder { color:var(--ink-3); }
.ab-send { flex:none; width:32px; height:32px; display:grid; place-items:center; border:0;
  border-radius:var(--r); background:var(--accent); color:var(--on-accent); cursor:pointer; }
.ab-send:hover { background:var(--accent-hover); }
.ab-send:disabled { opacity:.4; cursor:default; }
.ab-stop { flex:none; height:32px; padding:0 var(--s3); border:0; border-radius:var(--r);
  background:var(--btn); color:var(--ink); font:inherit; font-size:13px; cursor:pointer; }
.ab-stop:hover { background:var(--btn-hover); }
.ab-error { padding:0 var(--s4) var(--s2); font-size:13px; color:var(--err); }
`;

/** Lucide `arrow-up` (ISC), the send button's glyph. */
const SEND_ICON =
  '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" ' +
  'stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">' +
  '<path d="m5 12 7-7 7 7"/><path d="M12 19V5"/></svg>';

async function call<T>(path: string, payload?: unknown): Promise<T> {
  const res = await fetch(`/api${path}`, {
    method: payload === undefined ? "GET" : "POST",
    headers: payload === undefined ? {} : { "content-type": "application/json" },
    body: payload === undefined ? undefined : JSON.stringify(payload),
  });
  const text = await res.text();
  const data = text ? JSON.parse(text) : {};
  if (!res.ok) throw new Error(data.error ?? `${res.status}`);
  return data as T;
}

function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  cls?: string,
  text?: string,
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (cls) node.className = cls;
  if (text !== undefined) node.textContent = text;
  return node;
}

/** One line's inline Markdown: `code` and **bold**. */
function inline(line: string, into: HTMLElement): void {
  line.split(/(`[^`\n]+`|\*\*[^*\n]+\*\*)/).forEach((bit, j) => {
    if (j % 2 === 0) into.append(document.createTextNode(bit));
    else if (bit.startsWith("`")) into.append(el("code", undefined, bit.slice(1, -1)));
    else into.append(el("strong", undefined, bit.slice(2, -2)));
  });
}

/** An agent's text as paragraphs, headings and fenced code — no more of Markdown than that. Built
 *  from text nodes, so nothing an agent writes is ever parsed as HTML. */
function prose(text: string): DocumentFragment {
  const out = document.createDocumentFragment();
  const parts = text.split(/```[^\n]*\n?/);
  parts.forEach((part, i) => {
    if (i % 2 === 1) {
      const pre = el("pre");
      pre.append(el("code", undefined, part.replace(/\n$/, "")));
      out.append(pre);
      return;
    }
    for (const para of part.split(/\n{2,}/)) {
      if (!para.trim()) continue;
      const heading = para.match(/^#{1,6}\s+(.*)$/);
      const p = el("p", heading ? "ab-heading" : undefined);
      inline(heading ? heading[1] : para, p);
      out.append(p);
    }
  });
  return out;
}

/** A turn's tool calls, collapsed to one receipt line: "6 calls · Bash". */
function calls(steps: Step[]): HTMLElement | null {
  const tools = steps.filter((s) => s.kind === "tool");
  if (!tools.length) return null;
  const last = tools[tools.length - 1].name ?? "tool";
  return el("div", "ab-calls", `${tools.length} ${tools.length === 1 ? "call" : "calls"} · ${last}`);
}

function title(run: RunRow): string {
  const line = run.message.split("\n").find((l) => l.trim()) ?? "Conversation";
  return line.length > 60 ? `${line.slice(0, 60)}…` : line;
}

/** Mount the chat into `root`, filling it. */
export function chat(root: HTMLElement): void {
  if (!document.getElementById("ab-chat-style")) {
    const style = el("style", undefined, STYLE);
    style.id = "ab-chat-style";
    document.head.append(style);
  }

  const agentPick = el("select", "ab-chat__agent");
  agentPick.setAttribute("aria-label", "Agent");
  const runPick = el("select");
  runPick.setAttribute("aria-label", "Conversation");
  const bar = el("div", "ab-chat__bar");
  bar.append(agentPick, runPick);

  const log = el("div", "ab-chat__log");
  const error = el("div", "ab-error");
  const input = el("textarea");
  input.rows = 1;
  const send = el("button", "ab-send");
  send.type = "button";
  send.innerHTML = SEND_ICON;
  send.setAttribute("aria-label", "Send");
  const stop = el("button", "ab-stop", "Stop");
  stop.type = "button";
  const compose = el("div", "ab-compose");
  compose.append(input, stop, send);
  const foot = el("div", "ab-chat__foot");
  foot.append(compose);

  const box = el("div", "ab-chat");
  box.append(bar, log, error, foot);
  root.replaceChildren(box);

  let agent = localStorage.getItem("ab-agent") ?? DEFAULT_AGENT;
  let runs: RunRow[] = [];
  /** The open conversation; `null` is a new one, started by the next message. */
  let runId: string | null = null;
  let peek: Peek | null = null;
  let signature = "";
  let timer: number | undefined;
  let sending = false;

  const remember = () => {
    localStorage.setItem("ab-agent", agent);
    if (runId) localStorage.setItem(`ab-run:${agent}`, runId);
    else localStorage.removeItem(`ab-run:${agent}`);
  };

  function fail(err: unknown) {
    error.textContent = err instanceof Error ? err.message : String(err);
  }

  function drawControls() {
    const busy = Boolean(peek?.running);
    stop.hidden = !busy || !runId;
    send.disabled = sending || !input.value.trim();
    input.placeholder = `Write to ${agent}…`;
  }

  function drawRuns() {
    runPick.replaceChildren(el("option", undefined, "New conversation"));
    runPick.options[0].value = "";
    for (const run of runs) {
      const opt = el("option", undefined, title(run));
      opt.value = run.run_id;
      runPick.append(opt);
    }
    runPick.value = runId ?? "";
  }

  function drawLog() {
    const turns = peek?.turns ?? [];
    const sig = JSON.stringify([runId, turns.map((t) => [t.role, t.text, t.pending, t.queued, t.steps?.length])]);
    if (sig === signature) return;
    signature = sig;
    const atBottom = log.scrollHeight - log.scrollTop - log.clientHeight < 40;
    log.replaceChildren();
    if (!turns.length) {
      log.append(el("div", "ab-chat__empty", runId ? "Loading…" : `Ask ${agent} anything.`));
      return;
    }
    for (const turn of turns) {
      if (turn.role === "user") {
        const t = el("div", "ab-turn is-user", turn.text);
        log.append(t);
        if (turn.queued) log.append(el("div", "ab-note", "Queued — sent when the answer in flight is done"));
        continue;
      }
      const t = el("div", "ab-turn is-agent");
      const receipt = calls(turn.steps ?? []);
      if (receipt) t.append(receipt);
      if (turn.text) t.append(prose(turn.text));
      else if (turn.pending) t.append(el("div", "ab-note", "Thinking…"));
      log.append(t);
    }
    if (atBottom) log.scrollTop = log.scrollHeight;
  }

  async function loadAgents() {
    const { agents } = await call<{ agents: Agent[] }>("/agents");
    const names = (agents ?? []).filter((a) => a.runnable !== false).map((a) => a.name).sort(
      (a, b) => Number(a !== DEFAULT_AGENT) - Number(b !== DEFAULT_AGENT) || a.localeCompare(b),
    );
    agentPick.replaceChildren(
      ...names.map((n) => {
        const o = el("option", undefined, n);
        o.value = n;
        return o;
      }),
    );
    if (!names.includes(agent)) agent = names[0] ?? DEFAULT_AGENT;
    agentPick.value = agent;
  }

  async function loadRuns() {
    const res = await call<{ runs?: RunRow[] }>("/runs", { name: agent });
    runs = (res.runs ?? []).sort(
      (a, b) => (b.last_activity ?? b.started_at) - (a.last_activity ?? a.started_at),
    );
    drawRuns();
  }

  async function loadPeek() {
    if (!runId) {
      peek = null;
    } else {
      peek = await call<Peek>("/peek", { name: agent, run_id: runId });
    }
    error.textContent = "";
    drawLog();
    drawControls();
  }

  function schedule() {
    window.clearTimeout(timer);
    const streaming = peek?.running || peek?.turns?.some((t) => t.pending || t.queued);
    timer = window.setTimeout(tick, streaming ? FAST_MS : SLOW_MS);
  }

  async function tick() {
    try {
      await loadPeek();
    } catch (err) {
      fail(err);
    }
    schedule();
  }

  /** Open `agent`, on the conversation this browser last had open with it — or its latest. */
  async function openAgent() {
    signature = "";
    await loadRuns();
    const saved = localStorage.getItem(`ab-run:${agent}`);
    runId = saved === "" ? null : runs.some((r) => r.run_id === saved) ? saved : runs[0]?.run_id ?? null;
    runPick.value = runId ?? "";
    remember();
    await tick();
  }

  async function submit() {
    const message = input.value.trim();
    if (!message || sending) return;
    sending = true;
    drawControls();
    try {
      if (runId && peek?.answerable !== false) {
        peek = await call<Peek>("/reply", { name: agent, run_id: runId, message });
      } else {
        const res = await call<{ run_id?: string }>("/start", { name: agent, message });
        runId = res.run_id ?? null;
        remember();
        await loadRuns();
      }
      input.value = "";
      input.style.height = "";
      await tick();
    } catch (err) {
      fail(err);
    } finally {
      sending = false;
      drawControls();
    }
  }

  agentPick.addEventListener("change", () => {
    agent = agentPick.value;
    runId = null;
    remember();
    void openAgent().catch(fail);
  });
  runPick.addEventListener("change", () => {
    runId = runPick.value || null;
    remember();
    signature = "";
    void tick();
  });
  input.addEventListener("input", () => {
    input.style.height = "";
    input.style.height = `${Math.min(input.scrollHeight, 160)}px`;
    drawControls();
  });
  input.addEventListener("keydown", (ev) => {
    if (ev.key === "Enter" && !ev.shiftKey && !ev.isComposing) {
      ev.preventDefault();
      void submit();
    }
  });
  send.addEventListener("click", () => void submit());
  stop.addEventListener("click", () => {
    if (runId) void call("/stop", { name: agent, run_id: runId }).then(tick, fail);
  });

  drawControls();
  drawLog();
  void loadAgents()
    .then(openAgent)
    .catch(fail);
}

/** The board's page: the chat as its panel. */
export default function chatPanel(ctx: { panel(title?: string): HTMLElement }): void {
  const body = ctx.panel("Chat");
  // The shell lays panels out in 320px columns; the chat is the whole page, so it takes the row.
  if (body.parentElement) body.parentElement.style.gridColumn = "1 / -1";
  body.style.height = "calc(100vh - 160px)";
  chat(body);
}
