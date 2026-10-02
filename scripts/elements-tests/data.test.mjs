import assert from 'node:assert/strict';
import { test } from './helpers.mjs';

test('table sorting uses numeric order, places missing values last, and emits the selected direction', async ({ page, mount }) => {
  await mount('<adi-table id="subject"></adi-table>');
  await page.evaluate(() => {
    const table = document.querySelector('#subject');
    table.columns = [{ key: 'name', label: 'Name' }, { key: 'count', label: 'Count' }];
    table.rows = [
      { name: 'ten', count: 10 },
      { name: 'missing', count: null },
      { name: 'two', count: 2 },
      { name: 'zero', count: 0 },
    ];
    window.sortEvents = [];
    document.body.addEventListener('sort', (event) => window.sortEvents.push(event.detail));
  });

  await page.getByRole('button', { name: 'Count', exact: true }).click();
  assert.deepEqual(await page.locator('#subject tbody tr td:first-child').allTextContents(), ['zero', 'two', 'ten', 'missing']);
  assert.equal(await page.locator('#subject th').nth(1).getAttribute('aria-sort'), 'ascending');

  await page.getByRole('button', { name: 'Count', exact: true }).press('Enter');
  assert.deepEqual(await page.locator('#subject tbody tr td:first-child').allTextContents(), ['missing', 'ten', 'two', 'zero']);
  assert.equal(await page.locator('#subject th').nth(1).getAttribute('aria-sort'), 'descending');
  assert.deepEqual(await page.evaluate(() => ({
    events: window.sortEvents,
    originalRows: document.querySelector('#subject').rows.map((row) => row.name),
  })), {
    events: [{ key: 'count', dir: 'asc' }, { key: 'count', dir: 'desc' }],
    originalRows: ['ten', 'missing', 'two', 'zero'],
  });

  await page.getByRole('button', { name: 'Name', exact: true }).click();
  assert.equal(await page.locator('#subject th').nth(0).getAttribute('aria-sort'), 'ascending');
  assert.equal(await page.locator('#subject th').nth(1).getAttribute('aria-sort'), null);
  assert.deepEqual(await page.evaluate(() => window.sortEvents.at(-1)), { key: 'name', dir: 'asc' });
});

test('table selects the displayed row but ignores nested action controls', async ({ page, mount }) => {
  await mount('<adi-table id="subject" selectable sort="name"></adi-table>');
  await page.evaluate(() => {
    const table = document.querySelector('#subject');
    table.columns = [
      { key: 'name', label: 'Name' },
      { key: 'actions', label: 'Actions', actions: true, format: () => {
        const controls = document.createElement('div');
        controls.innerHTML = '<button type="button"><span>Edit</span></button> <a href="#details"><span>Details</span></a> <adi-button>Run</adi-button>';
        controls.querySelector('a').addEventListener('click', (event) => event.preventDefault());
        return controls;
      } },
    ];
    table.rows = [{ name: 'Zulu' }, { name: 'Alpha' }];
    window.selections = [];
    document.body.addEventListener('select', (event) => window.selections.push(event.detail));
  });

  const firstRow = page.locator('#subject tbody tr').first();
  await firstRow.locator('td').first().click();
  await firstRow.getByRole('button', { name: 'Edit', exact: true }).locator('span').click();
  await firstRow.getByRole('link', { name: 'Details', exact: true }).locator('span').click();
  await firstRow.getByRole('button', { name: 'Run', exact: true }).click();
  assert.deepEqual(await page.evaluate(() => window.selections), [{ row: { name: 'Alpha' }, index: 0 }]);
  assert.equal(await page.locator('#subject th').nth(1).locator('button').count(), 0);

  await page.getByRole('button', { name: 'Name', exact: true }).click();
  await page.locator('#subject tbody tr').first().locator('td').first().click();
  assert.deepEqual(await page.evaluate(() => window.selections.at(-1)), { row: { name: 'Zulu' }, index: 0 });
});

test('table replaces empty state when rows arrive and treats row text as text', async ({ page, mount }) => {
  await mount('<adi-table id="subject" empty="No matching records"></adi-table>');
  assert.equal(await page.locator('#subject .empty').isVisible(), true);
  assert.equal(await page.locator('#subject .empty').textContent(), 'No matching records');
  assert.equal(await page.locator('#subject table').isVisible(), false);

  await page.evaluate(() => {
    const table = document.querySelector('#subject');
    table.columns = [{ key: 'value', label: 'Value', sortable: false }];
    table.rows = [{ value: '<strong>literal text</strong>' }, { value: '' }, { value: false }, { value: 0 }];
  });
  assert.equal(await page.locator('#subject .empty').isVisible(), false);
  assert.deepEqual(await page.locator('#subject tbody td').allTextContents(), ['<strong>literal text</strong>', '—', 'false', '0']);
  assert.equal(await page.locator('#subject tbody strong').count(), 0);
  assert.equal(await page.locator('#subject thead button').count(), 0);

  await page.evaluate(() => { document.querySelector('#subject').rows = null; });
  assert.equal(await page.locator('#subject .empty').isVisible(), true);
  assert.equal(await page.locator('#subject tbody tr').count(), 0);
});

test('transcript polling and reordering preserve keyed nodes and the user-expanded tool run', async ({ page, mount }) => {
  await mount('<adi-transcript id="subject"></adi-transcript>');
  await page.evaluate(() => {
    const transcript = document.querySelector('#subject');
    transcript.entries = [
      { key: 'message', kind: 'said', role: 'user', body: 'First message' },
      { key: 'run', kind: 'did', run: { id: 'run-1', count: 1, tools: ['search'], state: 'running', calls: null } },
    ];
    window.originalMessage = transcript.shadowRoot.querySelector('#message');
    window.originalRun = transcript.shadowRoot.querySelector('#run');
    window.unchangedMessageBody = window.originalMessage.shadowRoot.querySelector('.md').firstChild;
    window.toggles = [];
    document.body.addEventListener('toggle', (event) => {
      if (event instanceof CustomEvent) window.toggles.push(event.detail);
    });
  });
  assert.deepEqual(await page.locator('#subject .entries > *').evaluateAll((nodes) => nodes.map((node) => node.id)), ['run', 'message']);
  await page.locator('#run button.head').click();
  assert.equal(await page.locator('#run .fetching').isVisible(), true);

  await page.evaluate(() => {
    const transcript = document.querySelector('#subject');
    // Polling supplies fresh objects, including an unchanged message.
    transcript.entries = JSON.parse(JSON.stringify(transcript.entries));
    transcript.entries = [
      { key: 'run', kind: 'did', run: { id: 'run-1', count: 1, tools: ['search'], state: 'ok', open: false, calls: [{ name: 'search', state: 'ok', result: 'Found it' }] } },
      { key: 'message', kind: 'said', role: 'user', body: 'First message' },
      { key: 'newest', kind: 'said', role: 'agent', body: 'Done' },
    ];
  });
  assert.deepEqual(await page.locator('#subject .entries > *').evaluateAll((nodes) => nodes.map((node) => node.id)), ['newest', 'message', 'run']);
  assert.equal(await page.locator('#run button.head').getAttribute('aria-expanded'), 'true');
  assert.equal(await page.locator('#run .result').isVisible(), true);
  assert.match(await page.locator('#run .result').textContent(), /Found it/);
  assert.deepEqual(await page.evaluate(() => ({
    sameMessage: document.querySelector('#subject').shadowRoot.querySelector('#message') === window.originalMessage,
    sameRun: document.querySelector('#subject').shadowRoot.querySelector('#run') === window.originalRun,
    sameMessageBody: window.originalMessage.shadowRoot.querySelector('.md').firstChild === window.unchangedMessageBody,
    toggles: window.toggles,
  })), { sameMessage: true, sameRun: true, sameMessageBody: true, toggles: [{ id: 'run-1', open: true }] });
});

test('transcript removes absent keys and replaces an entry whose kind changes', async ({ page, mount }) => {
  await mount('<adi-transcript id="subject"></adi-transcript>');
  await page.evaluate(() => {
    const transcript = document.querySelector('#subject');
    transcript.entries = [
      { key: 'old', kind: 'said', role: 'user', body: 'Remove me' },
      { key: 'changing', kind: 'said', role: 'agent', body: 'Pending' },
      { key: 'keep', kind: 'said', role: 'agent', body: 'Retained' },
    ];
    window.removed = transcript.shadowRoot.querySelector('#old');
    window.replaced = transcript.shadowRoot.querySelector('#changing');
    window.retained = transcript.shadowRoot.querySelector('#keep');
    transcript.entries = [
      { key: 'keep', id: 'renamed', kind: 'said', role: 'agent', body: 'Updated' },
      { key: 'changing', kind: 'note', note: { head: [{ text: 'Finished' }], body: 'Ready' } },
    ];
  });
  assert.deepEqual(await page.locator('#subject .entries > *').evaluateAll((nodes) => nodes.map((node) => [node.id, node.localName])), [
    ['changing', 'adi-note'], ['renamed', 'adi-message'],
  ]);
  assert.deepEqual(await page.evaluate(() => ({
    removed: window.removed.isConnected,
    replaced: window.replaced.isConnected,
    retained: document.querySelector('#subject').shadowRoot.querySelector('#renamed') === window.retained,
    body: window.retained.body,
  })), { removed: false, replaced: false, retained: true, body: 'Updated' });
  assert.match(await page.locator('#changing .head').textContent(), /Finished/);

  await page.evaluate(() => { document.querySelector('#subject').entries = null; });
  assert.equal(await page.locator('#subject .entries > *').count(), 0);
  await page.evaluate(() => {
    document.querySelector('#subject').entries = [{ key: 'keep', kind: 'said', role: 'agent', body: 'Back again' }];
  });
  assert.equal(await page.evaluate(() => document.querySelector('#subject').shadowRoot.querySelector('#keep') === window.retained), false);
});

test('tool run honors initial open state, loads results, and preserves a user collapse on updates', async ({ page, mount }) => {
  await mount('<adi-tool-run id="subject"></adi-tool-run>');
  await page.evaluate(() => {
    const run = document.querySelector('#subject');
    run.run = { id: 'fetch', count: 1, tools: ['read'], state: 'running', calls: null, open: true };
    window.toggles = [];
    run.addEventListener('toggle', (event) => window.toggles.push(event.detail));
  });
  assert.equal(await page.locator('#subject .fetching').isVisible(), true);
  await page.locator('#subject button.head').press('Enter');
  assert.equal(await page.locator('#subject .body').isVisible(), false);

  await page.evaluate(() => {
    document.querySelector('#subject').run = {
      id: 'fetch', count: 1, tools: ['read'], state: 'failed', open: true,
      calls: [{ name: 'read', state: 'failed', params: [['file', '<file>&notes']], result: '<error> unavailable & retry' }],
    };
  });
  assert.equal(await page.locator('#subject button.head').getAttribute('aria-expanded'), 'false');
  await page.locator('#subject button.head').click();
  assert.equal(await page.locator('#subject .fetching').count(), 0);
  assert.match(await page.locator('#subject .call pre').first().textContent(), /<file>&notes/);
  assert.match(await page.locator('#subject .result').textContent(), /<error> unavailable & retry/);
  assert.equal(await page.locator('#subject .call error, #subject .call file').count(), 0);
  assert.deepEqual(await page.evaluate(() => window.toggles), [{ id: 'fetch', open: false }, { id: 'fetch', open: true }]);
});

test('modal opens natively, closes with Escape and its button, and restores opener focus', async ({ page, mount }) => {
  await mount('<button id="opener">Open settings</button><adi-modal id="subject" label="Settings"><input aria-label="Setting"></adi-modal>');
  await page.evaluate(() => {
    const modal = document.querySelector('#subject');
    window.modalEvents = [];
    modal.addEventListener('open', () => window.modalEvents.push('open'));
    modal.addEventListener('close', () => window.modalEvents.push('close'));
    document.querySelector('#opener').addEventListener('click', () => modal.show());
  });

  await page.getByRole('button', { name: 'Open settings', exact: true }).click();
  assert.equal(await page.locator('#subject dialog').evaluate((dialog) => dialog.open && dialog.matches(':modal')), true);
  assert.equal(await page.locator('#subject h2').textContent(), 'Settings');
  await page.keyboard.press('Escape');
  await page.waitForFunction(() => window.modalEvents.length === 2);
  assert.equal(await page.locator('#subject').getAttribute('open'), null);
  assert.equal(await page.locator('#subject dialog').evaluate((dialog) => dialog.open), false);
  assert.equal(await page.evaluate(() => document.activeElement.id), 'opener');

  await page.getByRole('button', { name: 'Open settings', exact: true }).click();
  await page.getByRole('button', { name: 'Close', exact: true }).click();
  await page.waitForFunction(() => window.modalEvents.length === 4);
  assert.deepEqual(await page.evaluate(() => window.modalEvents), ['open', 'close', 'open', 'close']);
  assert.equal(await page.locator('#subject').getAttribute('open'), null);
  assert.equal(await page.evaluate(() => document.activeElement.id), 'opener');
});

test('modal footer follows dynamically assigned content while open', async ({ page, mount }) => {
  await mount('<adi-modal id="subject" label="Settings" open><p>Body</p></adi-modal>');
  assert.equal(await page.locator('#subject .foot').isVisible(), false);
  await page.evaluate(() => {
    const button = document.createElement('button');
    button.type = 'button';
    button.id = 'save';
    button.slot = 'foot';
    button.textContent = 'Save';
    document.querySelector('#subject').append(button);
  });
  await page.locator('#save').waitFor({ state: 'visible' });
  assert.equal(await page.locator('#subject .foot').isVisible(), true);
  await page.evaluate(() => document.querySelector('#save').remove());
  await page.locator('#subject .foot').waitFor({ state: 'hidden' });
  assert.equal(await page.locator('#subject dialog').evaluate((dialog) => dialog.open), true);
});
