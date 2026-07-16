// adi-sdk — Workforce SDK for WASM employee configs
// Types declared in index.d.ts
/// <reference path="./host.d.ts" />

import {
  callTool as _callTool,
  callLlm as _callLlm,
  log as _log,
  getContext as _getContext,
  readFile as _readFile,
  writeFile as _writeFile,
  loopInit as _loopInit,
  loopLlm as _loopLlm,
  loopTool as _loopTool,
  loopFinish as _loopFinish,
  subscribe as _subscribe,
} from "adi:workforce/host";

// ── Augmentable plugin registry ──

export interface PluginConfigs {}

// ── Ref types ──

export type ToolRef<PluginId = unknown, ToolId = unknown, Config = unknown> = {
  pluginId: PluginId;
  toolId: ToolId;
  config: Config;
};

export type RunnerRef<PluginId = unknown, RunnerId = unknown, Config = unknown> = {
  pluginId: PluginId;
  runnerId: RunnerId;
  config: Config;
};

export type FilesystemRef<PluginId = unknown, FsId = unknown, Config = unknown> = {
  pluginId: PluginId;
  fsId: FsId;
  config: Config;
};

// ── Mapped types (compile-time only) ──

type MapTools<Id, T> = {
  [K in keyof T]: (config: T[K]) => ToolRef<Id, K, T[K]>;
};

type MapRunners<Id, T> = {
  [K in keyof T]: (config: T[K]) => RunnerRef<Id, K, T[K]>;
};

type MapFilesystems<Id, T> = {
  [K in keyof T]: (config: T[K]) => FilesystemRef<Id, K, T[K]>;
};

type PluginAPI<Id extends keyof PluginConfigs> = {
  triggers: PluginConfigs[Id] extends { triggers: infer T } ? T : never;
  tools: PluginConfigs[Id] extends { tools: infer T } ? MapTools<Id, T> : never;
  runners: PluginConfigs[Id] extends { runners: infer T } ? MapRunners<Id, T> : never;
  filesystems: PluginConfigs[Id] extends { filesystems: infer T } ? MapFilesystems<Id, T> : never;
  functions: PluginConfigs[Id] extends { functions: infer T } ? T : never;
};

// ── Middleware types ──

export interface MiddlewareAction {
  type: 'ok' | 'refused' | 'panicTurn' | 'panicLoop' | 'finish';
  reason?: string;
}

export interface MiddlewareControls {
  run(): MiddlewareAction;
  panicTurn(opts: { reason: string }): MiddlewareAction;
  panicLoop(opts: { reason: string }): MiddlewareAction;
  finish(opts: { reason: string }): MiddlewareAction;
}

export interface InitContext {
  tools: ToolRef[];
  systemMessage: string;
}

export interface TurnContext {
  loopId: string;
  turnIndex: number;
}

export interface ToolCallContext extends MiddlewareControls {
  tool: ToolRef;
  getArguments(): unknown;
  updateArguments(args: unknown): void;
  refuse(opts: { reason: string }): MiddlewareAction;
}

export type InitMiddleware = (ctx: InitContext & { run: () => void }) => void;
export type TurnMiddleware = (ctx: TurnContext & MiddlewareControls) => MiddlewareAction;
export type ToolCallMiddleware = (ctx: ToolCallContext) => MiddlewareAction;

export interface Middlewares {
  init: InitMiddleware[];
  turn: TurnMiddleware[];
  toolCall: ToolCallMiddleware[];
}

// ── Employee registration ──

export interface EmployeeRegistration {
  name: string;
  description?: string;
  labels?: Record<string, string>;
}

// ── Loop types ──

export interface SystemMessageContext {
  tools: Array<{ name: string; proposedDescription: string }>;
}

export interface LoopConfig {
  name: string;
  runner: RunnerRef;
  filesystem?: FilesystemRef;
  tools: ToolRef[];
  systemMessage: string | ((ctx: SystemMessageContext) => string);
  middlewares?: Partial<Middlewares>;
  /// Structured output. When set, the host advertises a synthetic
  /// tool (default name `record_decision`) to the LLM whose schema is
  /// `parametersJson`. When the model calls it, the loop terminates
  /// and the tool args arrive on `LoopResult.decision` — skipping
  /// `loop_tool` entirely.
  ///
  /// Intended for decision loops (classify, resolve, gate). Build
  /// `parametersJson` with zod + zod-to-json-schema on the caller
  /// side, then `Schema.parse(result.decision)` to validate.
  schema?: DecisionSchema;
  /// Arbitrary metadata attached to the loop run. Forwarded verbatim to
  /// the host and exposed on `LoopRunContext.metadata` so middlewares,
  /// triggers, and observability can filter/group runs by caller-defined
  /// keys (e.g. `taskId`). Not interpreted by the host.
  metadata?: Record<string, unknown>;
  /// Pre-fill the conversation with synthetic turns before the model's
  /// first real turn. Each entry is a full turn (user or assistant)
  /// in the same wire shape the runtime uses. The model sees these
  /// turns as if it had just emitted them, so it continues from a
  /// chosen starting state.
  ///
  /// Use cases:
  ///   - **Pre-loaded plan**: an `assistant` `thinking` block plants
  ///     the next steps without spending a real reasoning turn.
  ///   - **Skip mechanical lookups**: bake in `ls` / `file_read` /
  ///     `task_get` results the operator already knows. Saves a
  ///     round-trip and the tokens of the response.
  ///   - **Few-shot shaping**: a worked example primes the loop's
  ///     output style without bloating the system prompt.
  ///
  /// Validated at construction time:
  ///   - Every `tool_use.name` must be in the loop's declared `tools`
  ///     (or match the synthetic decision tool name when `schema` is
  ///     set).
  ///   - Every `tool_use.id` must be unique within `prependHistory`.
  ///   - Every `tool_result.tool_use_id` must reference a `tool_use`
  ///     emitted earlier in `prependHistory`.
  ///   - A `tool_use` without a matching `tool_result` is allowed
  ///     only if it appears in the LAST entry — the loop will then
  ///     execute it on first turn. Otherwise the config is rejected.
  ///
  /// Token cost: prepended blocks are part of EVERY LLM request, so
  /// large pre-fills pre-spend tokens against the loop's budget.
  prependHistory?: PrependedTurn[];
}

/// A turn pre-spliced into the conversation via `LoopConfig.prependHistory`.
///
/// Same wire shape as runtime turns:
///   - `user` turn: `text` blocks (plain user content) and `tool_result`
///     blocks (paired with a prior `tool_use.id`).
///   - `assistant` turn: `text`, `thinking`, and `tool_use` blocks.
///
/// Block ordering is preserved verbatim on the wire.
export type PrependedTurn =
  | { role: 'user'; blocks: Array<
      | { type: 'text'; text: string }
      | { type: 'tool_result'; tool_use_id: string; content: string }
    > }
  | { role: 'assistant'; blocks: Array<
      | { type: 'text'; text: string }
      | { type: 'thinking'; text: string; signature?: string; redacted?: boolean }
      | { type: 'tool_use'; id: string; name: string; input: unknown }
    > };

export interface DecisionSchema {
  /// Synthetic tool name. Defaults to `record_decision`. Override
  /// only when a loop already exposes a real tool with that name.
  name?: string;
  description?: string;
  /// JSON Schema string advertised as the tool's input schema.
  parametersJson: string;
}

/// How a loop ended. Callers use this to decide whether a re-run would
/// be worthwhile.
///
/// * `'done'` — model returned a final answer (no tool calls). Normal
///   success path; don't retry.
/// * `'turn_limit'` — a `turn` middleware (typically `turnLimit`) called
///   `panicLoop`. Usually the loop ran out of its own self-imposed
///   budget; callers may want to retry with a bigger budget BEFORE
///   bouncing work back to the task level.
/// * `'middleware_stop'` — some other middleware deliberately stopped
///   the loop. Usually a refusal or policy gate; retry at a higher
///   level (escalation), not at the loop level.
/// * `'error'` — LLM call or response parse failed. Transient; safe to
///   retry.
/// * `'max_turns'` — raw `session.maxTurns` ceiling hit without any
///   middleware intervening. Should be rare; treat like `turn_limit`.
export type LoopEndReason =
  | 'done'
  | 'turn_limit'
  | 'middleware_stop'
  | 'error'
  | 'max_turns';

/// Outcome of a single tool call in the loop.
/// * `ok` — tool returned non-error content.
/// * `error` — tool returned a "Tool error:" / "Bad request:" string.
/// * `refused` — toolCall middleware intercepted with `skip:` / `stop:`
///   before the real tool ran.
/// * `decision` — synthetic `record_decision` call captured client-side
///   via `schema` (no real tool dispatch).
export type ToolCallOutcome = 'ok' | 'error' | 'refused' | 'decision';

export interface ToolCallRecord {
  /// Tool name as advertised to the LLM (e.g. `task_resolve`).
  name: string;
  /// Outcome category — see [`ToolCallOutcome`].
  outcome: ToolCallOutcome;
  /// Zero-based turn index in which the call appeared.
  turn: number;
}

export interface LoopResult {
  lastAssistantMessage: string;
  loopId: string;
  /// How the loop terminated. See [`LoopEndReason`].
  reason: LoopEndReason;
  /// Human-readable detail attached to `reason` — the middleware's
  /// `panicLoop` reason string, the error message, etc. Empty on
  /// normal `'done'` exits.
  reasonDetail: string;
  /// Structured decision payload, present only when the loop was
  /// configured with `schema` and the model called the synthetic
  /// decision tool. Unknown until the caller validates it (e.g.
  /// via `MySchema.parse(result.decision)`).
  decision?: unknown;
  /// Ordered log of tool calls the model made during the loop —
  /// names-and-outcomes only, no args/results. Callers can use this
  /// to verify that a terminal action (e.g. `task_resolve`,
  /// `msg_reply`, `record_decision`) actually happened. Capped at
  /// [`MAX_TOOL_CALL_RECORDS`] entries to bound payload size.
  toolCalls: ToolCallRecord[];
}

/// Per-loop cap on how many tool-call records we keep. Keeps WASM
/// payloads predictable even for runaway loops.
export const MAX_TOOL_CALL_RECORDS = 500;

export interface Loop {
  run(): LoopResult;
}

// ── Logging ──

export const log = {
  info: (msg: string) => _log("info", msg),
  warn: (msg: string) => _log("warn", msg),
  error: (msg: string) => _log("error", msg),
  debug: (msg: string) => _log("debug", msg),
};

// ── Context ──

export const ctx = {
  get: (key: string): string | undefined => _getContext(key) ?? undefined,
  workdir: (): string => _getContext("workdir") ?? ".",
  employee: (): string => _getContext("employee") ?? "unknown",
  loopId: (): string => _getContext("loop_id") ?? "unknown",
};

// ── File I/O ──

export const fs = {
  read: (path: string): string => _readFile(path),
  write: (path: string, content: string): void => _writeFile(path, content),
};

// ── Internal state ──

let _registration: EmployeeRegistration | null = null;

type TriggerHandler = (data: unknown) => void;
const _triggers: Map<string, TriggerHandler> = new Map();

// ── Middleware chain runners ──

const runInitChain = (middlewares: InitMiddleware[], initCtx: InitContext): string => {
  let idx = 0;
  const run = () => {
    if (idx < middlewares.length) middlewares[idx++]({ ...initCtx, run });
  };
  try {
    run();
  } catch (e: any) {
    return `stop:${e.message || e}`;
  }
  return "continue";
};

const runTurnChain = (middlewares: TurnMiddleware[], turnCtx: TurnContext): string => {
  let idx = 0;
  const controls: MiddlewareControls = {
    run: () => {
      if (idx < middlewares.length) return middlewares[idx++]({ ...turnCtx, ...controls });
      return { type: 'ok' };
    },
    panicTurn: (opts) => ({ type: 'panicTurn', reason: opts.reason }),
    panicLoop: (opts) => ({ type: 'panicLoop', reason: opts.reason }),
    finish: (opts) => ({ type: 'finish', reason: opts.reason }),
  };
  try {
    const result = controls.run();
    if (result.type === 'panicLoop') return `stop:${result.reason}`;
    if (result.type === 'panicTurn') return `skip:${result.reason}`;
    if (result.type === 'finish') return `stop:${result.reason}`;
  } catch (e: any) {
    return `stop:${e.message || e}`;
  }
  return "continue";
};

// ── SDK ──

const makeRefProxy = <R>(pluginId: string, refKey: string) =>
  new Proxy({} as any, {
    get: (_, name: string) => (config: unknown): R =>
      ({ pluginId, [refKey]: name, config }) as R,
  });

export const sdk = {
  /// Register this WASM module's employee identity. Must be called once at module top-level,
  /// before any loop runs. The host reads this right after instantiation.
  register(meta: EmployeeRegistration): void {
    if (_registration) {
      throw new Error(`sdk.register called twice (existing: "${_registration.name}", new: "${meta.name}")`);
    }
    if (!meta.name) {
      throw new Error('sdk.register: name is required');
    }
    _registration = meta;
  },

  plugin<Val extends keyof PluginConfigs>(id: Val): PluginAPI<Val> {
    const pluginId = id as string;
    return {
      triggers: new Proxy({} as any, {
        get: (_, name: string) => (config: unknown, handler: Function) => {
          const key = `${pluginId}.${name}`;
          _triggers.set(key, handler as TriggerHandler);
          _subscribe(key, JSON.stringify(config ?? {}));
        },
      }),
      tools: makeRefProxy<ToolRef>(pluginId, 'toolId'),
      runners: makeRefProxy<RunnerRef>(pluginId, 'runnerId'),
      filesystems: makeRefProxy<FilesystemRef>(pluginId, 'fsId'),
      functions: new Proxy({} as any, {
        get: (_, name: string) => (args: unknown) => {
          try {
            const result = _callTool(`${pluginId}.${name}`, JSON.stringify(args));
            try { return JSON.parse(result); } catch { return result; }
          } catch (e) {
            _log("warn", `function ${pluginId}.${name} failed: ${e}`);
            return undefined;
          }
        },
      }),
    } as PluginAPI<Val>;
  },

  loop(config: LoopConfig): Loop {
    // Fail-fast: validate prependHistory at loop-construct time so
    // bad configs surface BEFORE the loop spends a single LLM token.
    if (config.prependHistory?.length) {
      validatePrependHistory(config.prependHistory, config.tools, config.schema);
    }
    return {
      // Drive the entire loop synchronously from TS. The host handles
      // LLM calls and tool execution via `loopLlm` / `loopTool`;
      // middleware runs as ordinary TS function calls, so there's no
      // reentry into wasm (which the component model would trap on).
      run(): LoopResult {
        const systemMessage = typeof config.systemMessage === 'function'
          ? config.systemMessage({
            tools: config.tools.map(t => ({
              name: `${t.pluginId}.${t.toolId}`,
              proposedDescription: '',
            })),
          })
          : config.systemMessage;

        const initJson = _loopInit(JSON.stringify({
          name: config.name,
          runner: config.runner,
          filesystem: config.filesystem,
          system: systemMessage,
          tools: config.tools.map(t => ({
            plugin: t.pluginId,
            tool: t.toolId,
            config: t.config,
          })),
          // Opt-in structured output. Host advertises this schema as
          // a synthetic tool; when the model calls it, the response
          // from loop_llm carries `decision` and driveLoop ends the
          // conversation here.
          schema: config.schema,
          metadata: config.metadata,
        }));

        let session: { id: string; maxTurns: number };
        try {
          session = JSON.parse(initJson);
        } catch (e: any) {
          _log('error', `loop ${config.name}: init failed: ${e?.message || e}`);
          return {
            lastAssistantMessage: '',
            loopId: config.name,
            reason: 'error',
            reasonDetail: `init failed: ${e?.message || e}`,
            toolCalls: [],
          };
        }

        const mws = config.middlewares ?? {};
        try {
          return driveLoop(session, config, systemMessage, mws);
        } finally {
          _loopFinish(session.id);
        }
      },
    };
  },

  /// Run a loop on behalf of a specific task, then auto-post the loop's final
  /// assistant message as a comment on that task. Gives every dispatch a
  /// durable trail in task_history so later re-triggers (TaskAssigned, ready)
  /// can read prior run outputs.
  runForTask(opts: { taskId: string | number; vault?: string; config: LoopConfig }): LoopResult {
    const result = sdk.loop(opts.config).run();
    const body = (result.lastAssistantMessage || '').trim();
    if (!body) return result;
    const id = typeof opts.taskId === 'string' ? parseInt(opts.taskId, 10) : opts.taskId;
    if (!Number.isFinite(id) || id <= 0) return result;
    const args: Record<string, unknown> = {
      id,
      body: `[loop:${opts.config.name}]\n\n${body}`,
    };
    if (opts.vault) args.vault = opts.vault;
    try {
      _callTool('adi.workforce.capability.tasks.task_comment', JSON.stringify(args));
    } catch (e) {
      _log('warn', `runForTask: auto-comment failed on #${id}: ${e}`);
    }
    return result;
  },
};

// ── TS-side loop driver ──
//
// Mirrors the conversation-turn logic that used to live in
// loop_runner.rs, but runs inside the calling wasm frame so middleware
// executes as plain TS calls. Per-turn limits fire naturally; callers
// can rely on `.run()` returning the real result synchronously.

type TurnMsg =
  | { role: 'user'; blocks: Array<
      | { type: 'text'; text: string }
      | { type: 'tool_result'; tool_use_id: string; content: string }
    > }
  | { role: 'assistant'; blocks: Array<
      | { type: 'text'; text: string }
      | { type: 'thinking'; text: string; signature?: string; redacted?: boolean }
      | { type: 'tool_use'; id: string; name: string; input: unknown }
    > };

// `PrependedTurn` (the public type) and `TurnMsg` (the internal driver
// type) are structurally identical — keep both in sync. The internal
// alias spares us another widening cast on every push into the turns
// array.
const validatePrependHistory = (
  history: PrependedTurn[],
  tools: ToolRef[],
  schema: DecisionSchema | undefined,
): void => {
  // Tool-name set: declared tools rendered three ways, because the
  // name the *model* actually sees can differ from the SDK-side
  // toolId, and prependHistory's `tool_use.name` must match what the
  // model would have emitted:
  //   1) `pluginId.toolId`           (fully-qualified SDK form)
  //   2) bare `toolId`               (CamelCase as authored)
  //   3) snake_case(`toolId`)        (the convention `tool.name()`
  //                                   uses on the host — `TaskGet`
  //                                   → `task_get` — which is what
  //                                   the LLM sees in tool_use.name)
  // Plus the synthetic decision tool name when `schema` is set.
  const camelToSnake = (s: string): string =>
    s.replace(/([a-z0-9])([A-Z])/g, '$1_$2')
      .replace(/([A-Z])([A-Z][a-z])/g, '$1_$2')
      .toLowerCase();
  const knownToolNames = new Set<string>();
  for (const t of tools) {
    const toolId = String(t.toolId);
    knownToolNames.add(`${t.pluginId}.${toolId}`);
    knownToolNames.add(toolId);
    knownToolNames.add(camelToSnake(toolId));
  }
  if (schema) knownToolNames.add(schema.name ?? 'record_decision');

  const seenToolUseIds = new Set<string>();
  const pendingToolUseIds = new Set<string>();
  let lastEntryToolUseIds = new Set<string>();

  for (let i = 0; i < history.length; i++) {
    const t = history[i];
    if (t.role === 'assistant') {
      const localIds = new Set<string>();
      for (const b of t.blocks) {
        if (b.type === 'tool_use') {
          if (!b.id) {
            throw new Error(`prependHistory[${i}]: tool_use missing 'id'`);
          }
          if (!b.name) {
            throw new Error(`prependHistory[${i}]: tool_use missing 'name'`);
          }
          if (!knownToolNames.has(b.name)) {
            throw new Error(
              `prependHistory[${i}]: tool_use.name='${b.name}' is not in the loop's declared tools` +
              (schema ? ` (or schema.name='${schema.name ?? 'record_decision'}')` : ''),
            );
          }
          if (seenToolUseIds.has(b.id)) {
            throw new Error(`prependHistory[${i}]: duplicate tool_use.id='${b.id}'`);
          }
          seenToolUseIds.add(b.id);
          pendingToolUseIds.add(b.id);
          localIds.add(b.id);
        }
      }
      lastEntryToolUseIds = localIds;
    } else if (t.role === 'user') {
      lastEntryToolUseIds = new Set();
      for (const b of t.blocks) {
        if (b.type === 'tool_result') {
          if (!b.tool_use_id) {
            throw new Error(`prependHistory[${i}]: tool_result missing 'tool_use_id'`);
          }
          if (!seenToolUseIds.has(b.tool_use_id)) {
            throw new Error(
              `prependHistory[${i}]: tool_result.tool_use_id='${b.tool_use_id}' has no prior tool_use in prependHistory`,
            );
          }
          pendingToolUseIds.delete(b.tool_use_id);
        }
      }
    } else {
      throw new Error(`prependHistory[${i}]: unknown role '${(t as { role?: string }).role}'`);
    }
  }

  // Unmatched tool_use blocks are allowed only when they belong to the
  // LAST entry — the loop will then execute them on first turn (as if
  // the model had just emitted them). Anywhere else, a tool_use without
  // a tool_result violates Anthropic's content-block pair rule.
  for (const id of pendingToolUseIds) {
    if (!lastEntryToolUseIds.has(id)) {
      throw new Error(
        `prependHistory: tool_use.id='${id}' has no matching tool_result; ` +
        `unmatched tool_use blocks must appear in the LAST prependHistory entry only`,
      );
    }
  }
};

interface LlmHostResponse {
  blocks: Array<
    | { type: 'text'; text: string }
    | { type: 'thinking'; text: string; signature?: string; redacted?: boolean }
    | { type: 'tool_use'; id: string; name: string; input: unknown }
  >;
  stopReason?: string;
  usage?: {
    inputTokens: number;
    outputTokens: number;
    cacheCreationInputTokens?: number;
    cacheReadInputTokens?: number;
  };
  /// Present when the loop was configured with `schema` and the model
  /// emitted a `tool_use` for the synthetic decision tool. Host lifts
  /// the tool args here so the driver can end the loop without going
  /// through `loop_tool`.
  decision?: unknown;
}

type LoopDriverResult = LoopResult;

const driveLoop = (
  session: { id: string; maxTurns: number },
  config: LoopConfig,
  systemMessage: string,
  mws: Partial<Middlewares>,
): LoopResult => {
  // Track why the loop ended so callers can decide between retry,
  // escalate, or accept. Default to 'max_turns' — if the for-loop
  // exits naturally (rare: normally `done` or middleware stop fires
  // first), we keep it; any earlier path overwrites before return.
  let reason: LoopEndReason = 'max_turns';
  let reasonDetail = '';
  let decision: unknown = undefined;
  const toolCalls: ToolCallRecord[] = [];
  const recordCall = (name: string, outcome: ToolCallOutcome, turn: number) => {
    if (toolCalls.length < MAX_TOOL_CALL_RECORDS) {
      toolCalls.push({ name, outcome, turn });
    }
  };

  // onLoopStart / init middleware
  if (mws.init?.length) {
    const action = runInitChain(mws.init, { tools: config.tools, systemMessage });
    if (action.startsWith('stop:')) {
      return {
        lastAssistantMessage: '',
        loopId: config.name,
        reason: 'middleware_stop',
        reasonDetail: action.slice('stop:'.length),
        toolCalls,
      };
    }
  }

  // systemMessage is both system prompt AND the initial user turn.
  // This mirrors the prior wasm_config_loader behaviour — we pass the
  // system string as `user_message` to the runner. Keeps the LLM
  // seeing the task description even when it's also the system prompt.
  const turns: TurnMsg[] = [
    { role: 'user', blocks: [{ type: 'text', text: systemMessage }] },
  ];

  // Splice declarative seeding turns BEFORE the loop's first LLM call,
  // AFTER the system-as-user-turn. The model sees these turns as if it
  // had just emitted them, so its first real turn picks up from the
  // chosen state instead of cold-starting on the systemMessage alone.
  // Validated at loop-construct time — see `validatePrependHistory`.
  if (config.prependHistory?.length) {
    for (const t of config.prependHistory) {
      turns.push(t as TurnMsg);
    }
  }

  let lastAssistantMessage = '';

  outer: for (let turn = 0; turn < session.maxTurns; turn++) {
    // onTurnStart middleware
    if (mws.turn?.length) {
      const action = runTurnChain(mws.turn, { loopId: config.name, turnIndex: turn });
      if (action.startsWith('stop:')) {
        const detail = action.slice('stop:'.length);
        // Turn-middleware-initiated stops are almost always turnLimit
        // (see nakit-yok/.adi/workforce/shared.ts). Bucket them
        // under `turn_limit` so callers can retry with bigger budget.
        // Other turn middlewares would need to avoid the literal
        // "Turn limit" substring to stay classified as generic
        // middleware_stop.
        reason = detail.toLowerCase().includes('turn limit') ? 'turn_limit' : 'middleware_stop';
        reasonDetail = detail;
        break outer;
      }
      // "skip:" on turn-start has no natural mapping — treat as continue.
    }

    let responseJson: string;
    try {
      responseJson = _loopLlm(session.id, JSON.stringify(turns));
    } catch (e: any) {
      _log('error', `loop ${config.name}: llm failed: ${e?.message || e}`);
      reason = 'error';
      reasonDetail = `llm failed: ${e?.message || e}`;
      break outer;
    }

    let response: LlmHostResponse;
    try {
      response = JSON.parse(responseJson);
    } catch {
      _log('error', `loop ${config.name}: bad llm response json`);
      reason = 'error';
      reasonDetail = 'bad llm response json';
      break outer;
    }

    // Capture assistant text for lastAssistantMessage
    const assistantText = response.blocks
      .filter((b): b is { type: 'text'; text: string } => b.type === 'text')
      .map(b => b.text)
      .join('');
    if (assistantText) lastAssistantMessage = assistantText;

    turns.push({ role: 'assistant', blocks: response.blocks });

    // Structured output short-circuit: the host lifted the synthetic
    // decision tool's args to `response.decision`. We terminate
    // immediately without calling `loop_tool` — there's no host
    // implementation for the synthetic tool anyway.
    if (response.decision !== undefined) {
      decision = response.decision;
      reason = 'done';
      reasonDetail = 'decision';
      const decisionName = config.schema?.name ?? 'record_decision';
      recordCall(decisionName, 'decision', turn);
      break outer;
    }

    const turnToolCalls = response.blocks.filter(
      (b): b is { type: 'tool_use'; id: string; name: string; input: unknown } =>
        b.type === 'tool_use',
    );

    if (!turnToolCalls.length) {
      reason = 'done';
      reasonDetail = '';
      break outer; // final answer
    }

    // Execute each tool, running beforeToolCall / afterToolCall middleware
    const toolResults: Array<{ type: 'tool_result'; tool_use_id: string; content: string }> = [];
    for (const tc of turnToolCalls) {
      let args: unknown = tc.input;

      if (mws.toolCall?.length) {
        const ref: ToolRef = { pluginId: '', toolId: tc.name, config: {} };
        // runToolCallChain mutates `args` via updateArguments and returns a
        // control string — keep its contract intact.
        const { action, finalArgs } = runToolCallChainWithArgs(mws.toolCall, ref, args);
        args = finalArgs;
        if (action.startsWith('stop:')) {
          recordCall(tc.name, 'refused', turn);
          reason = 'middleware_stop';
          reasonDetail = action.slice('stop:'.length);
          break outer;
        }
        if (action.startsWith('skip:')) {
          recordCall(tc.name, 'refused', turn);
          toolResults.push({
            type: 'tool_result',
            tool_use_id: tc.id,
            content: action.slice('skip:'.length) || 'skipped by middleware',
          });
          continue;
        }
      }

      let result: string;
      try {
        result = _loopTool(session.id, tc.name, JSON.stringify(args));
      } catch (e: any) {
        result = `Tool error: ${e?.message || e}`;
      }
      const isError = result.startsWith('Tool error:') || result.startsWith('Bad request:');
      recordCall(tc.name, isError ? 'error' : 'ok', turn);
      toolResults.push({ type: 'tool_result', tool_use_id: tc.id, content: result });
    }

    turns.push({ role: 'user', blocks: toolResults });
  }

  return {
    lastAssistantMessage,
    loopId: config.name,
    reason,
    reasonDetail,
    decision,
    toolCalls,
  };
};

/**
 * Variant of runToolCallChain that also returns the (possibly-mutated)
 * args so the host can forward them to the real tool. The original
 * `runToolCallChain` swallows updates — fine when the host didn't
 * route tool args through middleware, but we need them now.
 */
const runToolCallChainWithArgs = (
  middlewares: ToolCallMiddleware[],
  tool: ToolRef,
  args: unknown,
): { action: string; finalArgs: unknown } => {
  let currentArgs = args;
  let idx = 0;

  const next = (): MiddlewareAction => {
    if (idx >= middlewares.length) return { type: 'ok' };
    return middlewares[idx++]({
      tool,
      getArguments: () => currentArgs,
      updateArguments: (a) => { currentArgs = a; },
      run: next,
      refuse: (opts) => ({ type: 'refused', reason: opts.reason }),
      panicTurn: (opts) => ({ type: 'panicTurn', reason: opts.reason }),
      panicLoop: (opts) => ({ type: 'panicLoop', reason: opts.reason }),
      finish: (opts) => ({ type: 'finish', reason: opts.reason }),
    });
  };

  let action: string = 'continue';
  try {
    const r = next();
    if (r.type === 'refused') action = `skip:${r.reason ?? 'refused'}`;
    else if (r.type === 'panicLoop') action = `stop:${r.reason}`;
    else if (r.type === 'panicTurn') action = `skip:${r.reason}`;
    else if (r.type === 'finish') action = `stop:${r.reason}`;
  } catch (e: any) {
    action = `stop:${e?.message || e}`;
  }
  return { action, finalArgs: currentArgs };
};

// ── WIT exports: registration ──

export const getRegistration = (): string =>
  _registration ? JSON.stringify(_registration) : '';

// ── WIT exports: event dispatch ──

export const dispatch = (handler: string, dataJson: string): string => {
  try {
    const fn = _triggers.get(handler);
    if (!fn) return "continue";
    fn(JSON.parse(dataJson));
    return "continue";
  } catch (e: any) {
    const name = e?.name || typeof e;
    const msg = e?.message || String(e);
    const stack = e?.stack || '<no stack>';
    _log('error', `dispatch ${handler} crashed: ${name}: ${msg}\n${stack}`);
    return "continue";
  }
};

// ── WIT exports: lifecycle hooks ──
//
// Middleware runs inside `driveLoop` as plain TS calls now — the host
// never calls these hooks anymore. They remain as WIT exports only so
// the component link stays stable; treat each as a no-op.

export const onLoopStart = (): string => "continue";
export const onTurnStart = (_turn: number, _turnsJson: string): string => "continue";
export const onTurnEnd = (_turn: number, _responseJson: string, _turnsJson: string): string =>
  "continue";
export const beforeToolCall = (_toolName: string, _argsJson: string): string => "continue";
export const afterToolCall = (_toolName: string, _argsJson: string, _toolResult: string): string =>
  "continue";
export const onLoopEnd = (_resultJson: string): string => "continue";

// ── Runtime entry (overridden by user script) ──
// Put trigger subscribes + any host calls (env resolution, etc.) inside the
// user's `export const main = () => { ... }`. Module top-level runs at
// Wizer snapshot time and cannot reach host imports.
export const main = (): void => {};
