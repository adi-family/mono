#!/usr/bin/env node
// Smoke test for prependHistory validation logic.
//
// Replicates the validator from index.ts because index.ts imports the
// `adi:workforce/host` interface that only resolves under jco. We
// duplicate (not import) — keep this in sync with `validatePrependHistory`
// in index.ts.

const camelToSnake = (s) =>
  s.replace(/([a-z0-9])([A-Z])/g, '$1_$2').replace(/([A-Z])([A-Z][a-z])/g, '$1_$2').toLowerCase();

const validatePrependHistory = (history, tools, schema) => {
  const knownToolNames = new Set();
  for (const t of tools) {
    const toolId = String(t.toolId);
    knownToolNames.add(`${t.pluginId}.${toolId}`);
    knownToolNames.add(toolId);
    knownToolNames.add(camelToSnake(toolId));
  }
  if (schema) knownToolNames.add(schema.name ?? 'record_decision');

  const seenToolUseIds = new Set();
  const pendingToolUseIds = new Set();
  let lastEntryToolUseIds = new Set();

  for (let i = 0; i < history.length; i++) {
    const t = history[i];
    if (t.role === 'assistant') {
      const localIds = new Set();
      for (const b of t.blocks) {
        if (b.type === 'tool_use') {
          if (!b.id) throw new Error(`prependHistory[${i}]: tool_use missing 'id'`);
          if (!b.name) throw new Error(`prependHistory[${i}]: tool_use missing 'name'`);
          if (!knownToolNames.has(b.name)) {
            throw new Error(`prependHistory[${i}]: tool_use.name='${b.name}' is not in the loop's declared tools`);
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
          if (!b.tool_use_id) throw new Error(`prependHistory[${i}]: tool_result missing 'tool_use_id'`);
          if (!seenToolUseIds.has(b.tool_use_id)) {
            throw new Error(`prependHistory[${i}]: tool_result.tool_use_id='${b.tool_use_id}' has no prior tool_use in prependHistory`);
          }
          pendingToolUseIds.delete(b.tool_use_id);
        }
      }
    } else {
      throw new Error(`prependHistory[${i}]: unknown role '${t.role}'`);
    }
  }

  for (const id of pendingToolUseIds) {
    if (!lastEntryToolUseIds.has(id)) {
      throw new Error(`prependHistory: tool_use.id='${id}' has no matching tool_result; unmatched tool_use blocks must appear in the LAST prependHistory entry only`);
    }
  }
};

let passed = 0;
let failed = 0;
const t = (name, fn) => {
  try {
    fn();
    console.log(`  ok  ${name}`);
    passed++;
  } catch (e) {
    console.log(`  FAIL ${name}: ${e.message || e}`);
    failed++;
  }
};

const expectThrow = (fn, msgFragment) => {
  try {
    fn();
  } catch (e) {
    const msg = String(e.message || e);
    if (msgFragment && !msg.includes(msgFragment)) {
      throw new Error(`expected error to contain '${msgFragment}', got: ${msg}`);
    }
    return;
  }
  throw new Error('expected throw, got pass');
};

const tools = [
  { pluginId: 'adi.workforce.capability.code', toolId: 'file_read', config: {} },
  { pluginId: 'adi.workforce.capability.code', toolId: 'ls', config: {} },
];

const camelTools = [
  { pluginId: 'adi.workforce.capability.tasks', toolId: 'TaskGet', config: {} },
  { pluginId: 'adi.workforce.capability.tasks', toolId: 'TaskHistory', config: {} },
];

console.log('prependHistory validation tests:');

// happy paths

t('empty history is fine', () => {
  validatePrependHistory([], tools, undefined);
});

t('plain user text is fine', () => {
  validatePrependHistory(
    [{ role: 'user', blocks: [{ type: 'text', text: 'hi' }] }],
    tools,
    undefined,
  );
});

t('thinking block is fine', () => {
  validatePrependHistory(
    [{ role: 'assistant', blocks: [{ type: 'thinking', text: 'plan: …' }] }],
    tools,
    undefined,
  );
});

t('paired tool_use + tool_result by pluginId.toolId is fine', () => {
  validatePrependHistory(
    [
      { role: 'assistant', blocks: [{
        type: 'tool_use', id: 't1',
        name: 'adi.workforce.capability.code.ls',
        input: { path: '.' },
      }] },
      { role: 'user', blocks: [{
        type: 'tool_result', tool_use_id: 't1', content: 'a/ b/',
      }] },
    ],
    tools,
    undefined,
  );
});

t('paired tool_use using bare toolId is fine', () => {
  validatePrependHistory(
    [
      { role: 'assistant', blocks: [{ type: 'tool_use', id: 't1', name: 'ls', input: {} }] },
      { role: 'user', blocks: [{ type: 'tool_result', tool_use_id: 't1', content: 'ok' }] },
    ],
    tools,
    undefined,
  );
});

t('unmatched tool_use is fine if it is the LAST entry', () => {
  validatePrependHistory(
    [
      { role: 'assistant', blocks: [{ type: 'thinking', text: 'planning' }] },
      { role: 'assistant', blocks: [{ type: 'tool_use', id: 't1', name: 'ls', input: { path: '.' } }] },
    ],
    tools,
    undefined,
  );
});

t('decision tool name is accepted when schema is set', () => {
  validatePrependHistory(
    [
      { role: 'assistant', blocks: [{
        type: 'tool_use', id: 'd1', name: 'record_decision', input: { branch: 'foo' },
      }] },
      { role: 'user', blocks: [{ type: 'tool_result', tool_use_id: 'd1', content: 'ok' }] },
    ],
    tools,
    { parametersJson: '{}' },
  );
});

t('snake_case name accepted for CamelCase toolId', () => {
  validatePrependHistory(
    [
      { role: 'assistant', blocks: [{ type: 'tool_use', id: 'g1', name: 'task_get', input: { id: 1 } }] },
      { role: 'user', blocks: [{ type: 'tool_result', tool_use_id: 'g1', content: '...' }] },
    ],
    camelTools,
    undefined,
  );
});

t('CamelCase toolId still accepted under its own name', () => {
  validatePrependHistory(
    [
      { role: 'assistant', blocks: [{ type: 'tool_use', id: 'g1', name: 'TaskGet', input: { id: 1 } }] },
      { role: 'user', blocks: [{ type: 'tool_result', tool_use_id: 'g1', content: '...' }] },
    ],
    camelTools,
    undefined,
  );
});

t('custom decision tool name is accepted', () => {
  validatePrependHistory(
    [
      { role: 'assistant', blocks: [{
        type: 'tool_use', id: 'd1', name: 'classify', input: {},
      }] },
      { role: 'user', blocks: [{ type: 'tool_result', tool_use_id: 'd1', content: 'ok' }] },
    ],
    tools,
    { name: 'classify', parametersJson: '{}' },
  );
});

// failure paths

t('rejects unknown tool name', () => {
  expectThrow(() => validatePrependHistory(
    [{ role: 'assistant', blocks: [{ type: 'tool_use', id: 't1', name: 'rm_rf', input: {} }] }],
    tools,
    undefined,
  ), 'not in the loop');
});

t('rejects duplicate tool_use.id', () => {
  expectThrow(() => validatePrependHistory(
    [
      { role: 'assistant', blocks: [{ type: 'tool_use', id: 't1', name: 'ls', input: {} }] },
      { role: 'user',      blocks: [{ type: 'tool_result', tool_use_id: 't1', content: 'a' }] },
      { role: 'assistant', blocks: [{ type: 'tool_use', id: 't1', name: 'ls', input: {} }] },
    ],
    tools,
    undefined,
  ), 'duplicate');
});

t('rejects tool_result with no prior tool_use', () => {
  expectThrow(() => validatePrependHistory(
    [{ role: 'user', blocks: [{ type: 'tool_result', tool_use_id: 'orphan', content: 'a' }] }],
    tools,
    undefined,
  ), 'no prior tool_use');
});

t('rejects unmatched tool_use that is NOT the last entry', () => {
  expectThrow(() => validatePrependHistory(
    [
      { role: 'assistant', blocks: [{ type: 'tool_use', id: 't1', name: 'ls', input: {} }] },
      { role: 'assistant', blocks: [{ type: 'thinking', text: 'next…' }] },
    ],
    tools,
    undefined,
  ), 'no matching tool_result');
});

t('rejects empty tool_use.id', () => {
  expectThrow(() => validatePrependHistory(
    [{ role: 'assistant', blocks: [{ type: 'tool_use', id: '', name: 'ls', input: {} }] }],
    tools,
    undefined,
  ), "missing 'id'");
});

t('rejects unknown role', () => {
  expectThrow(() => validatePrependHistory(
    [{ role: 'system', blocks: [] }],
    tools,
    undefined,
  ), 'unknown role');
});

console.log(`\n${passed} passed, ${failed} failed`);
process.exit(failed ? 1 : 0);
