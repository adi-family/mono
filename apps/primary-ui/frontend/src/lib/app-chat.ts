import { appApiBase } from "./app-api";

export interface ConversationRef {
  agent: string;
  runId: string;
  /** A paired node's petname; omitted for this machine. */
  source?: string;
}

export interface FleetNode {
  node: string;
  locked: boolean;
}

export interface AgentCapabilities {
  answerable?: boolean;
  interactive?: boolean;
  images?: boolean;
  [key: string]: unknown;
}

export interface Agent {
  name: string;
  source?: string;
  starred?: boolean;
  project?: string;
  createdBy?: string;
  runnable?: boolean;
  running: boolean;
  backend?: string;
  caps?: AgentCapabilities;
}

export interface Conversation extends ConversationRef {
  title: string;
  message: string;
  running: boolean;
  lastActivity: number;
  answerable: boolean;
  interactive: boolean;
  state?: string;
  launchedBy?: string;
  pendingQuestion?: boolean;
  awaiting?: boolean;
  hidden?: boolean;
  starred?: boolean;
  agentStarred?: boolean;
  askingUser?: string | null;
  project?: string;
  projectName?: string;
}

export interface ConversationIndexOptions {
  hidden?: boolean;
  filter?: "all" | "starred" | "mine" | "me";
  limit?: number;
  group?: "project";
  groupLimit?: number;
  groupMore?: Record<string, number>;
}

export interface ConversationGroup {
  project?: string;
  label: string;
  conversations: Conversation[];
  total: number;
}

export interface ConversationIndex {
  conversations: Conversation[];
  total: number;
  askingUser: string | null;
  groups: ConversationGroup[];
}

export type AgentToolStatus = "running" | "ok" | "error" | "unanswered";

export type AgentStep =
  | { kind: "message"; text: string }
  | { kind: "thinking"; text: string }
  | { kind: "tool"; name: string; input: string; status: AgentToolStatus; output: string }
  | { kind: "calls"; from: number; to: number; count: number; tools: string[]; preview: string; status: AgentToolStatus }
  | { kind: "unknown" };

export interface AgentAttachment {
  id: string;
  name: string;
  media_type: string;
  size: number;
}

export type TurnMarker =
  | { kind: "from"; node: string; user: string }
  | { kind: "await-woken"; id: string; cause: string; event: string; check: boolean }
  | { kind: "ask-answered"; id: string; by: string }
  | { kind: "goal-check"; open: number }
  | { kind: "pre-run"; ran: number; dropped: number }
  | { kind: "unknown" };

export interface AgentTurn {
  role: string;
  text: string;
  seq: number;
  at: number;
  pending: boolean;
  queued: boolean;
  mode?: "regular" | "asap";
  images: AgentAttachment[];
  steps: AgentStep[];
  markers?: TurnMarker[];
}

export interface AgentQuestion {
  header: string;
  question: string;
  options: { label: string; description: string }[];
  multi_select: boolean;
}

export interface AgentAsk {
  id: string;
  note: string;
  headline: string;
  questions: AgentQuestion[];
  asked_at: number;
  deadline?: number | null;
}

export interface AgentAwait {
  id: string;
  note: string;
  summary: string;
  check?: string;
}

/** The server caps failure references, but the per-tool totals cover the whole chat. */
export interface AgentChatStats {
  failed: { turn: number; step: number }[];
  by_tool: { tool: string; calls: number; failed: number }[];
}

export interface AgentPeek {
  name: string;
  run_id: string;
  running: boolean;
  answerable: boolean;
  interactive: boolean;
  caps: AgentCapabilities;
  cwd: string;
  output: string;
  turns: AgentTurn[];
  total_turns: number;
  state?: string;
  pending_question?: AgentAsk;
  awaits: AgentAwait[];
  stats?: AgentChatStats;
}

export class AppChatError extends Error {
  readonly status: number;

  constructor(message: string, status: number) {
    super(message);
    this.name = "AppChatError";
    this.status = status;
  }
}

type JsonObject = Record<string, unknown>;

function isObject(value: unknown): value is JsonObject {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function object(value: unknown): JsonObject {
  if (!isObject(value)) throw new Error("The app returned an invalid response.");
  return value;
}

function array(value: unknown): unknown[] {
  if (!Array.isArray(value)) throw new Error("The app returned an invalid response.");
  return value;
}

function text(value: unknown): string {
  return typeof value === "string" ? value : "";
}

function requiredText(value: unknown): string {
  const result = text(value);
  if (!result) throw new Error("The app returned an invalid response.");
  return result;
}

function number(value: unknown): number {
  return typeof value === "number" && Number.isFinite(value) ? value : 0;
}

function caps(value: unknown): AgentCapabilities {
  return isObject(value) ? value : {};
}

function isCount(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
}

function statsOf(value: unknown): AgentChatStats | undefined {
  if (!isObject(value) || !Array.isArray(value.failed) || !Array.isArray(value.by_tool)) return;
  const failed: AgentChatStats["failed"] = [];
  const by_tool: AgentChatStats["by_tool"] = [];
  const seen = new Set<string>();
  for (const ref of value.failed) {
    if (!isObject(ref) || !isCount(ref.turn) || !isCount(ref.step)) return;
    const key = `${ref.turn}:${ref.step}`;
    if (seen.has(key)) return;
    seen.add(key);
    failed.push({ turn: ref.turn, step: ref.step });
  }
  let total = 0;
  for (const use of value.by_tool) {
    if (!isObject(use) || typeof use.tool !== "string" || !isCount(use.calls) ||
      !isCount(use.failed) || use.failed > use.calls) return;
    total += use.failed;
    if (!Number.isSafeInteger(total)) return;
    by_tool.push({ tool: use.tool, calls: use.calls, failed: use.failed });
  }
  if (failed.length > total) return;
  return { failed, by_tool };
}

async function request(path: string, signal?: AbortSignal, body?: JsonObject, source?: string): Promise<JsonObject> {
  const response = await fetch(`${appApiBase(source)}${path}`, {
    method: body === undefined ? "GET" : "POST",
    ...(body === undefined ? {} : {
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    }),
    signal,
  });
  const raw = await response.text();
  let data: unknown;
  try {
    data = JSON.parse(raw);
  } catch {
    throw new AppChatError(
      response.ok ? "The app returned an invalid response." : `The app returned HTTP ${response.status}.`,
      response.status,
    );
  }
  if (!response.ok || (isObject(data) && data.ok === false)) {
    throw new AppChatError(
      (isObject(data) && text(data.error)) || (response.ok
        ? "The app could not complete the request."
        : `The app returned HTTP ${response.status}.`),
      response.status,
    );
  }
  return object(data);
}

function runBody(ref: ConversationRef): JsonObject {
  return { name: ref.agent, run_id: ref.runId };
}

function toolStatus(value: unknown): AgentToolStatus {
  return value === "running" || value === "error" || value === "unanswered" ? value : "ok";
}

function stepOf(value: unknown): AgentStep {
  const step = object(value);
  switch (step.kind) {
    case "message":
    case "thinking":
      return { kind: step.kind, text: text(step.text) };
    case "tool":
      return {
        kind: "tool", name: text(step.name), input: text(step.input),
        status: toolStatus(step.status), output: text(step.output),
      };
    case "calls":
      return {
        kind: "calls", from: number(step.from), to: number(step.to), count: number(step.count),
        tools: Array.isArray(step.tools) ? step.tools.filter((tool): tool is string => typeof tool === "string") : [],
        preview: text(step.preview), status: toolStatus(step.status),
      };
    default:
      return { kind: "unknown" };
  }
}

function askOf(value: unknown): AgentAsk {
  const ask = object(value);
  return {
    id: requiredText(ask.id), note: text(ask.note), headline: text(ask.headline),
    asked_at: number(ask.asked_at),
    ...(ask.deadline === null || typeof ask.deadline === "number" ? { deadline: ask.deadline } : {}),
    questions: array(ask.questions).map((value) => {
      const question = object(value);
      return {
        header: text(question.header), question: text(question.question),
        multi_select: question.multi_select === true,
        options: (Array.isArray(question.options) ? question.options : []).map((value) => {
          const option = object(value);
          return { label: text(option.label), description: text(option.description) };
        }),
      };
    }),
  };
}

function markerOf(value: unknown): TurnMarker {
  const marker = object(value);
  switch (marker.kind) {
    case "from": return { kind: "from", node: text(marker.node), user: text(marker.user) };
    case "await-woken": return {
      kind: "await-woken", id: text(marker.id), cause: text(marker.cause),
      event: text(marker.event), check: marker.check === true,
    };
    case "ask-answered": return { kind: "ask-answered", id: text(marker.id), by: text(marker.by) };
    case "goal-check": return { kind: "goal-check", open: number(marker.open) };
    case "pre-run": return { kind: "pre-run", ran: number(marker.ran), dropped: number(marker.dropped) };
    default: return { kind: "unknown" };
  }
}

function peekOf(data: JsonObject): AgentPeek {
  const stats = statsOf(data.stats);
  const turns = (Array.isArray(data.turns) ? data.turns : []).map((value, position): AgentTurn => {
    const turn = object(value);
    return {
      role: text(turn.role), text: text(turn.text),
      seq: number(turn.seq) || position,
      at: number(turn.at), pending: turn.pending === true, queued: turn.queued === true,
      ...(turn.mode === "regular" || turn.mode === "asap" ? { mode: turn.mode } : {}),
      images: (Array.isArray(turn.images) ? turn.images : []).map((value) => {
        const image = object(value);
        return { id: requiredText(image.id), name: text(image.name), media_type: text(image.media_type), size: number(image.size) };
      }),
      steps: (Array.isArray(turn.steps) ? turn.steps : []).map(stepOf),
      ...(Array.isArray(turn.markers) ? { markers: turn.markers.map(markerOf) } : {}),
    };
  });
  return {
    name: text(data.name), run_id: text(data.run_id), running: data.running === true,
    answerable: data.answerable === true, interactive: data.interactive === true,
    caps: caps(data.caps), cwd: text(data.cwd), output: text(data.output),
    turns, total_turns: number(data.total_turns) || turns.length,
    ...(typeof data.state === "string" ? { state: data.state } : {}),
    ...(isObject(data.pending_question) ? { pending_question: askOf(data.pending_question) } : {}),
    awaits: (Array.isArray(data.awaits) ? data.awaits : []).map((value) => {
      const item = object(value);
      return {
        id: text(item.id), note: text(item.note), summary: text(item.summary),
        ...(typeof item.check === "string" ? { check: item.check } : {}),
      };
    }),
    ...(stats ? { stats } : {}),
  };
}

export async function getFleetNodes(signal?: AbortSignal): Promise<FleetNode[]> {
  const data = await request("/fleet/nodes", signal);
  return array(data.nodes).map((value) => {
    const node = object(value);
    return { node: requiredText(node.node), locked: node.locked === true };
  });
}

export async function getAgents(signal?: AbortSignal, source?: string): Promise<Agent[]> {
  const data = await request("/agents", signal, undefined, source);
  return array(data.agents).map((value) => {
    const agent = object(value);
    return {
      name: requiredText(agent.name), running: agent.running === true, starred: agent.starred === true,
      ...(source === undefined ? {} : { source }),
      ...(typeof agent.project === "string" ? { project: agent.project } : {}),
      ...(typeof agent.created_by === "string" ? { createdBy: agent.created_by } : {}),
      ...(typeof agent.runnable === "boolean" ? { runnable: agent.runnable } : {}),
      ...(typeof agent.backend === "string" ? { backend: agent.backend } : {}),
      ...(isObject(agent.caps) ? { caps: caps(agent.caps) } : {}),
    };
  });
}

export async function getConversationIndex(
  signal?: AbortSignal, source?: string, options: ConversationIndexOptions = {},
): Promise<ConversationIndex> {
  const params = new URLSearchParams({ hidden: String(options.hidden ?? false) });
  // Hidden is deliberately unpaged in the main app, so every put-away conversation is reachable.
  if (!options.hidden || options.limit !== undefined) params.set("limit", String(options.limit ?? 100));
  if (options.filter && options.filter !== "all") params.set("filter", options.filter);
  if (options.group === "project") {
    params.set("group", "project");
    params.set("group_limit", String(options.groupLimit ?? 3));
    const more = Object.entries(options.groupMore ?? {}).filter(([, count]) => Number.isSafeInteger(count) && count > 0)
      .sort(([left], [right]) => left < right ? -1 : left > right ? 1 : 0).map(([project, count]) => `${project}:${count}`).join(",");
    if (more) params.set("group_more", more);
  }
  const data = await request(`/agents/runs/all?${params}`, signal, undefined, source);
  const askingUser = typeof data.asking_user === "string" ? data.asking_user : null;
  const groupData = (Array.isArray(data.groups) ? data.groups : []).map((value) => {
    const group = object(value);
    return {
      ...(typeof group.project === "string" ? { project: group.project } : {}),
      label: text(group.label),
      agents: new Set((Array.isArray(group.agents) ? group.agents : []).filter((agent): agent is string => typeof agent === "string")),
      total: number(group.total),
    };
  });
  const conversations = array(data.agents).flatMap((value) => {
    const agent = object(value);
    const agentName = requiredText(agent.name);
    const agentCaps = caps(agent.caps);
    const project = groupData.find((group) => group.agents.has(agentName));
    return array(agent.runs).map((value): Conversation => {
      const run = object(value);
      // A historical session keeps its own backend even if the agent definition changed.
      const runCaps = isObject(run.caps) ? caps(run.caps) : agentCaps;
      const message = text(run.message);
      return {
        agent: agentName, runId: requiredText(run.run_id),
        ...(source === undefined ? {} : { source }),
        title: text(run.title).trim() || message.trim().replace(/\s+/g, " ").slice(0, 120) || "Untitled chat",
        message, running: run.running === true,
        lastActivity: number(run.last_activity) || number(run.started_at),
        answerable: typeof runCaps.answerable === "boolean" ? runCaps.answerable : agent.answerable === true,
        interactive: typeof runCaps.interactive === "boolean" ? runCaps.interactive : agent.interactive === true,
        launchedBy: text(run.launched_by), pendingQuestion: isObject(run.pending_question),
        awaiting: Array.isArray(run.awaits) && run.awaits.length > 0,
        hidden: run.hidden === true, starred: run.starred === true, askingUser,
        ...(typeof agent.starred === "boolean" ? { agentStarred: agent.starred } : {}),
        ...(project ? { project: project.project, projectName: project.label } : {}),
        ...(typeof run.state === "string" ? { state: run.state } : {}),
      };
    });
  }).sort((a, b) => b.lastActivity - a.lastActivity);
  return {
    conversations, total: typeof data.total === "number" ? number(data.total) : conversations.length, askingUser,
    groups: groupData.map(({ agents, ...group }) => ({ ...group, conversations: conversations.filter((conversation) => agents.has(conversation.agent)) })),
  };
}

export async function getConversations(
  signal?: AbortSignal, source?: string, options?: ConversationIndexOptions,
): Promise<Conversation[]> {
  return (await getConversationIndex(signal, source, options)).conversations;
}

export async function getConversation(
  ref: ConversationRef,
  signal?: AbortSignal,
  view: { limit?: number; before?: number } = {},
): Promise<AgentPeek> {
  return peekOf(await request("/agents/run/peek", signal, {
    ...runBody(ref), limit: view.limit ?? 20, fold: true,
    ...(view.before === undefined ? {} : { before: view.before }),
  }, ref.source));
}

export async function getSteps(
  ref: ConversationRef, turn: number, from: number, to: number, signal?: AbortSignal,
): Promise<AgentStep[]> {
  const data = await request("/agents/run/steps", signal, { ...runBody(ref), turn, from, to }, ref.source);
  return array(data.steps).map(stepOf);
}

export async function startConversation(agent: string, message: string, signal?: AbortSignal, source?: string, attachments: string[] = []): Promise<ConversationRef> {
  const data = await request("/agents/run", signal, { name: agent, message, attachments, launched_by: "human" }, source);
  if (!text(data.run_id)) throw new Error("This agent did not open a conversation.");
  return { agent, runId: text(data.run_id), ...(source === undefined ? {} : { source }) };
}

export async function replyToConversation(ref: ConversationRef, message: string, signal?: AbortSignal, attachments: string[] = []): Promise<AgentPeek> {
  return peekOf(await request("/agents/run/reply", signal, {
    ...runBody(ref), message, attachments, mode: "regular", limit: 20, fold: true,
  }, ref.source));
}

export async function stopConversation(ref: ConversationRef, signal?: AbortSignal): Promise<void> {
  await request("/agents/run/stop", signal, runBody(ref), ref.source);
}

export async function answerQuestion(
  ref: ConversationRef, ask: string, replies: string[], signal?: AbortSignal,
): Promise<AgentPeek> {
  return peekOf(await request("/agents/run/answer", signal, {
    ...runBody(ref), ask, replies, limit: 20, fold: true,
  }, ref.source));
}

export function attachmentUrl(id: string, source?: string): string {
  return `${appApiBase(source)}/agents/attachment/${encodeURIComponent(id)}`;
}

export async function uploadImage(file: File, source?: string): Promise<AgentAttachment> {
  if (!["image/png", "image/jpeg", "image/webp", "image/gif"].includes(file.type)) throw new Error("Choose a PNG, JPEG, WebP, or GIF image.");
  if (file.size > 5 * 1024 * 1024) throw new Error("Images must be 5 MB or smaller.");
  const response = await fetch(`${appApiBase(source)}/agents/attachment`, {
    method: "POST", headers: { "Content-Type": file.type, "X-Adi-Filename": file.name.replace(/[^\x20-\x7e]/g, "_") }, body: file,
  });
  const data = await response.json();
  if (!response.ok || data.ok === false) throw new Error(data.error || "Image could not be uploaded. Try again.");
  if (typeof data.id !== "string" || !data.id) throw new Error("The app returned an invalid image reference.");
  return { id: data.id, name: file.name, media_type: file.type, size: file.size };
}

export async function renameConversation(ref: ConversationRef, title: string): Promise<void> {
  await request("/agents/run/rename", undefined, { ...runBody(ref), title: title.trim() }, ref.source);
}

export interface VoiceEngine { id: string; label: string; ready: boolean; in_browser: boolean; detail: string }
export async function getVoice(): Promise<{ engines: VoiceEngine[]; default_engine: string }> {
  const data = await request("/voice");
  return { engines: array(data.engines).map((value) => {
    const item = object(value);
    return { id: text(item.id), label: text(item.label), ready: item.ready === true, in_browser: item.in_browser === true, detail: text(item.detail) };
  }), default_engine: text(data.default_engine) || "browser" };
}
export async function transcribeAudio(audio: Blob, engine: string, signal?: AbortSignal): Promise<string> {
  const response = await fetch(`${appApiBase()}/voice/transcribe?engine=${encodeURIComponent(engine)}`, {
    method: "POST", headers: { "Content-Type": audio.type || "audio/webm" }, body: audio, signal,
  });
  const data = await response.json();
  if (!response.ok || data.ok === false) throw new Error(data.error || "Could not transcribe this recording.");
  if (typeof data.text !== "string") throw new Error("The app returned an invalid transcript.");
  return data.text;
}
