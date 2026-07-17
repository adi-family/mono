// Starter employee: each inbox message runs one LLM loop and logs the
// reply. Copy this file to start a new employee — one file per employee,
// `npm run build` compiles every *.ts here to build/<name>.wasm.
import { sdk, log } from '@adi-family/workforce-sdk';
export * from '@adi-family/workforce-sdk';

// Untyped plugin handles: the typed surface comes from tsp-gen'd type
// packages we don't port; the runtime contract is plain strings anyway.
const plugin = sdk.plugin as (id: string) => any;
const orch = plugin('adi.workforce.capability.orchestration');
const claude = plugin('adi.workforce.runner.claude');
const shell = plugin('adi.workforce.capability.shell');

sdk.register({
  name: 'hello',
  description: 'starter employee: one LLM loop per inbox message',
  labels: { team: 'starter' },
});

export const main = () => {
  orch.triggers.EmployeeMessage({}, (args: { from: string; message: string }) => {
    log.info(`hello received from=${args.from}`);
    const result = sdk
      .loop({
        name: 'answer',
        runner: claude.runners.ClaudeCodeApi({ model: 'claude-sonnet-5' }),
        tools: [shell.tools.Shell({})],
        // The systemMessage doubles as the loop's first user turn, so the
        // request goes here — see driveLoop in sdk/workforce/index.ts.
        systemMessage:
          'You are "hello", a starter adi-workforce employee. Answer briefly; ' +
          'use Shell only when the request actually needs it.\n\n' +
          `Request from ${args.from}:\n${args.message}`,
      })
      .run();
    log.info(`loop ended (${result.reason}): ${result.lastAssistantMessage}`);
  });
};
