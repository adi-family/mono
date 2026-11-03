import type { AgentStep, AgentToolStatus, AgentTurn, ConversationRef } from "./app-chat";
import { appHref, IS_PRIMARY_APP } from "./app-api";

export interface ToolArgument {
  name: string;
  value: string;
}

function argumentText(value: unknown, depth = 0): string {
  if (typeof value === "string") return value;
  if (value === null) return "None";
  if (typeof value !== "object") return String(value);
  if (depth > 8) return "…";
  if (Array.isArray(value)) return value.map((item) => argumentText(item, depth + 1)).join("\n");
  return Object.entries(value).map(([name, item]) => `${name}: ${argumentText(item, depth + 1)}`).join("\n");
}

/** Arguments remain readable text, including the actual newlines in edits and commands. */
export function toolArguments(input: string): ToolArgument[] {
  if (!input.trim()) return [];
  try {
    const value: unknown = JSON.parse(input);
    if (value !== null && typeof value === "object" && !Array.isArray(value)) {
      return Object.entries(value).map(([name, item]) => ({ name, value: argumentText(item) }));
    }
    return [{ name: "Input", value: argumentText(value) }];
  } catch {
    return [{ name: "Input", value: input }];
  }
}

export function activityStatus(status: AgentToolStatus): "running" | "completed" | "error" | "cancelled" {
  switch (status) {
    case "running": return "running";
    case "error": return "error";
    case "unanswered": return "cancelled";
    default: return "completed";
  }
}

export function toolDisplayName(name: string): string {
  return name.split(/__|[.:/]/).filter(Boolean).at(-1) || name;
}

export function toolSummary(name: string, input: string): { title: string; kind: "git" | "terminal" | "edit" | "tool" } {
  const args = toolArguments(input);
  const command = args.find((arg) => ["command", "cmd"].includes(arg.name))?.value;
  const terminal = /(?:bash|shell|terminal|exec_command|exec)$/i.test(name) || Boolean(command);
  const edit = /(?:edit|write|apply_patch|patch)$/i.test(name);
  const git = terminal && /(?:^|[;&|]\s*)git\s/.test(command?.trim() || "");
  return {
    title: toolDisplayName(name),
    kind: git ? "git" : edit ? "edit" : terminal ? "terminal" : "tool",
  };
}

/** Only the CLI's successful launch receipt identifies a particular child conversation. */
export function startedAgents(output: string): ConversationRef[] {
  const receipts = /^Started agent ([^\s]+) (?:as background process \d+\.|through its lease at \S+\.)\r?\n[\t ]+run:[\t ]+([^\s]+)[\t ]*$/gm;
  const refs: ConversationRef[] = [];
  const seen = new Set<string>();
  for (const match of output.matchAll(receipts)) {
    const [, agent, runId] = match;
    const key = JSON.stringify([agent, runId]);
    if (!seen.has(key)) {
      refs.push({ agent, runId });
      seen.add(key);
    }
  }
  return refs;
}

/** A receipt inside a file read is still file content, not an agent launch. */
export function agentLaunches(step: Extract<AgentStep, { kind: "tool" }>): ConversationRef[] {
  if (step.status === "error" || !/(?:^|[._:-])(?:bash|shell|terminal|exec_command|exec)$/i.test(step.name)) return [];
  const args = toolArguments(step.input);
  const command = args.find((arg) => ["command", "cmd", "code", "Input"].includes(arg.name))?.value ?? "";
  if (!/(?:^|[\s/'"])(?:adi(?:-mono)?\s+agents\s+run|adi-agents\s+run)(?=\s|$)/.test(command)) return [];
  return startedAgents(step.output);
}

export function conversationHref({ agent, runId, source }: ConversationRef): string {
  return `/interface?agent=${encodeURIComponent(agent)}&run=${encodeURIComponent(runId)}${source ? `&node=${encodeURIComponent(source)}` : ""}`;
}

export function appConversationHref(ref: ConversationRef): string {
  if (IS_PRIMARY_APP) return conversationHref(ref);
  const { agent, runId, source } = ref;
  return appHref(`/chat/${source ? `@${encodeURIComponent(source)}/` : ""}${encodeURIComponent(agent)}/${encodeURIComponent(runId)}`);
}

type ActivityStep = Exclude<AgentStep, { kind: "message" } | { kind: "unknown" }>;

export type TranscriptPart =
  | { kind: "message"; text: string; key: string }
  | { kind: "activity"; steps: { step: ActivityStep; index: number }[]; key: string };

/** Preserve text as the divider between runs of activity; final text lives outside steps. */
export function transcriptParts(turn: AgentTurn): TranscriptPart[] {
  const parts: TranscriptPart[] = [];
  let activity: { step: ActivityStep; index: number }[] = [];
  const finishActivity = () => {
    if (activity.length) parts.push({ kind: "activity", steps: activity, key: `activity-${activity[0].index}` });
    activity = [];
  };
  turn.steps.forEach((step, index) => {
    if (step.kind === "message") {
      finishActivity();
      if (step.text.trim()) parts.push({ kind: "message", text: step.text, key: `message-${index}` });
    } else if (step.kind === "tool" || step.kind === "thinking" || step.kind === "calls") {
      activity.push({ step, index });
    }
  });
  finishActivity();
  if (turn.text.trim()) parts.push({ kind: "message", text: turn.text, key: "final" });
  return parts;
}
