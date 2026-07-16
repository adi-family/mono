/// <reference path="./host.d.ts" />
export interface PluginConfigs {
}
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
    triggers: PluginConfigs[Id] extends {
        triggers: infer T;
    } ? T : never;
    tools: PluginConfigs[Id] extends {
        tools: infer T;
    } ? MapTools<Id, T> : never;
    runners: PluginConfigs[Id] extends {
        runners: infer T;
    } ? MapRunners<Id, T> : never;
    filesystems: PluginConfigs[Id] extends {
        filesystems: infer T;
    } ? MapFilesystems<Id, T> : never;
    functions: PluginConfigs[Id] extends {
        functions: infer T;
    } ? T : never;
};
export interface MiddlewareAction {
    type: 'ok' | 'refused' | 'panicTurn' | 'panicLoop' | 'finish';
    reason?: string;
}
export interface MiddlewareControls {
    run(): MiddlewareAction;
    panicTurn(opts: {
        reason: string;
    }): MiddlewareAction;
    panicLoop(opts: {
        reason: string;
    }): MiddlewareAction;
    finish(opts: {
        reason: string;
    }): MiddlewareAction;
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
    refuse(opts: {
        reason: string;
    }): MiddlewareAction;
}
export type InitMiddleware = (ctx: InitContext & {
    run: () => void;
}) => void;
export type TurnMiddleware = (ctx: TurnContext & MiddlewareControls) => MiddlewareAction;
export type ToolCallMiddleware = (ctx: ToolCallContext) => MiddlewareAction;
export interface Middlewares {
    init: InitMiddleware[];
    turn: TurnMiddleware[];
    toolCall: ToolCallMiddleware[];
}
export interface EmployeeRegistration {
    name: string;
    description?: string;
    labels?: Record<string, string>;
}
export interface SystemMessageContext {
    tools: Array<{
        name: string;
        proposedDescription: string;
    }>;
}
export interface LoopConfig {
    name: string;
    runner: RunnerRef;
    filesystem?: FilesystemRef;
    tools: ToolRef[];
    systemMessage: string | ((ctx: SystemMessageContext) => string);
    middlewares?: Partial<Middlewares>;
    schema?: DecisionSchema;
    /// Arbitrary metadata attached to the loop run. Forwarded to the host
    /// and available on `LoopRunContext.metadata` for observability,
    /// filtering, and middleware decisions. Not interpreted by the host.
    metadata?: Record<string, unknown>;
    /// Pre-fill the conversation with synthetic turns before the model's
    /// first real turn. Validated at construction time:
    ///   - Every `tool_use.name` must exist in `tools` (or match
    ///     `schema.name` / 'record_decision' when schema is set).
    ///   - Every `tool_use.id` must be unique within prependHistory.
    ///   - Every `tool_result.tool_use_id` must reference a prior
    ///     `tool_use` in prependHistory.
    ///   - Unmatched `tool_use` blocks are allowed only in the LAST
    ///     entry — the loop will execute them on first turn.
    prependHistory?: PrependedTurn[];
}
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
    name?: string;
    description?: string;
    parametersJson: string;
}
export type LoopEndReason =
    | 'done'
    | 'turn_limit'
    | 'middleware_stop'
    | 'error'
    | 'max_turns';
export type ToolCallOutcome = 'ok' | 'error' | 'refused' | 'decision';
export interface ToolCallRecord {
    name: string;
    outcome: ToolCallOutcome;
    turn: number;
}
export interface LoopResult {
    lastAssistantMessage: string;
    loopId: string;
    reason: LoopEndReason;
    reasonDetail: string;
    decision?: unknown;
    /// Ordered log of tool calls the model made during the loop —
    /// names and outcomes only, no args/results. Capped at 500 entries.
    toolCalls: ToolCallRecord[];
}
export declare const MAX_TOOL_CALL_RECORDS = 500;
export interface Loop {
    run(): LoopResult;
}
export declare const log: {
    info: (msg: string) => any;
    warn: (msg: string) => any;
    error: (msg: string) => any;
    debug: (msg: string) => any;
};
export declare const ctx: {
    get: (key: string) => string | undefined;
    workdir: () => string;
    employee: () => string;
    loopId: () => string;
};
export declare const fs: {
    read: (path: string) => string;
    write: (path: string, content: string) => void;
};
export declare const sdk: {
    register(meta: EmployeeRegistration): void;
    plugin<Val extends keyof PluginConfigs>(id: Val): PluginAPI<Val>;
    loop(config: LoopConfig): Loop;
    runForTask(opts: { taskId: string | number; vault?: string; config: LoopConfig }): LoopResult;
};
export declare const getRegistration: () => string;
export declare const dispatch: (handler: string, dataJson: string) => string;
export declare const onLoopStart: () => string;
export declare const onTurnStart: (turn: number, _turnsJson: string) => string;
export declare const onTurnEnd: (_turn: number, _responseJson: string, _turnsJson: string) => string;
export declare const beforeToolCall: (toolName: string, argsJson: string) => string;
export declare const afterToolCall: (_toolName: string, _argsJson: string, _toolResult: string) => string;
export declare const onLoopEnd: (_resultJson: string) => string;
export declare const main: () => void;
export {};
