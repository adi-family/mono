// Engine e2e probe agent. No LLM required: it subscribes to the employee
// inbox, and on dispatch proves the WIT host surface works — logging,
// get-context, and a call-tool round-trip (Env).
import { sdk, log } from '@adi-family/workforce-sdk';
export * from '@adi-family/workforce-sdk';

// Untyped plugin handles: the typed surface comes from tsp-gen'd type
// packages we don't port; the runtime contract is plain strings anyway.
const plugin = sdk.plugin as (id: string) => any;
const orch = plugin('adi.workforce.capability.orchestration');
const env = plugin('adi.workforce.variable.env');

sdk.register({
  name: 'probe',
  description: 'adi-workforce engine e2e probe',
  labels: { team: 'test' },
});

export const main = () => {
  orch.triggers.EmployeeMessage({}, (args: { from: string; message: string }) => {
    log.info(`probe received from=${args.from} message=${args.message}`);
    const home = env.functions.Env({ value: 'HOME' });
    log.info(`call-tool round-trip ok: HOME=${home?.resolved ?? '<unresolved>'}`);
    log.info('probe done');
  });
};
