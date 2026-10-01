import assert from 'node:assert/strict';
import { test } from './helpers.mjs';

async function recordEvents(page, types) {
  await page.evaluate((eventTypes) => {
    window.events = [];
    for (const type of eventTypes) {
      document.body.addEventListener(type, (event) => {
        window.events.push({ type, detail: event.detail, composed: event.composed });
      });
    }
  }, types);
}

async function events(page) {
  return page.evaluate(() => window.events);
}

async function setAttachments(page, attachments) {
  await page.locator('#subject').evaluate((element, value) => { element.attachments = value; }, attachments);
}

async function setAsk(page, ask) {
  await page.locator('#subject').evaluate((element, value) => { element.ask = value; }, ask);
}

test('composer sends the exact draft by Enter or button and leaves clearing to its caller', async ({ page, mount }) => {
  await mount('<adi-composer id="subject" placeholder="Write a reply"></adi-composer>');
  await recordEvents(page, ['send']);
  const input = page.locator('#subject textarea');
  const send = page.locator('#subject .send');

  assert.equal(await input.getAttribute('placeholder'), 'Write a reply');
  assert.equal(await send.isDisabled(), true);
  await input.fill('   ');
  await input.press('Enter');
  assert.equal(await send.isDisabled(), true);
  assert.deepEqual(await events(page), []);

  await input.fill('  hello there  ');
  await input.press('Enter');
  assert.equal(await input.inputValue(), '  hello there  ');
  await send.click();
  assert.deepEqual(await events(page), [
    { type: 'send', detail: { text: '  hello there  ' }, composed: true },
    { type: 'send', detail: { text: '  hello there  ' }, composed: true },
  ]);

  await page.locator('#subject').evaluate((element) => { element.value = ''; });
  assert.equal(await input.inputValue(), '');
  assert.equal(await send.isDisabled(), true);
});

test('composer preserves Shift-Enter newlines and does not submit modified or composing Enter', async ({ page, mount }) => {
  await mount('<adi-composer id="subject"></adi-composer>');
  await recordEvents(page, ['send']);
  const input = page.locator('#subject textarea');
  await input.fill('first');
  await input.press('End');
  await input.press('Shift+Enter');
  await input.press('x');
  assert.equal(await input.inputValue(), 'first\nx');
  assert.deepEqual(await events(page), []);

  // Synthetic events exercise IME state and every modifier without relying on OS shortcuts.
  const prevented = await input.evaluate((element) => ['shiftKey', 'altKey', 'ctrlKey', 'metaKey', 'isComposing'].map((flag) => {
    const event = new KeyboardEvent('keydown', { key: 'Enter', [flag]: true, bubbles: true, cancelable: true });
    element.dispatchEvent(event);
    return event.defaultPrevented;
  }));
  assert.deepEqual(prevented, [false, false, false, false, false]);
  assert.deepEqual(await events(page), []);
  await input.press('Enter');
  assert.deepEqual((await events(page)).map((event) => event.detail), [{ text: 'first\nx' }]);
});

test('composer busy state blocks submission while stop and send-asap follow their attributes', async ({ page, mount }) => {
  await mount('<adi-composer id="subject" stoppable asap></adi-composer>');
  await recordEvents(page, ['send', 'asap', 'stop']);
  const input = page.locator('#subject textarea');
  const stop = page.locator('#subject .stop');
  const asap = page.locator('#subject .asap');
  await input.fill('urgent');
  await asap.click();
  await page.locator('#subject').evaluate((element) => { element.setAttribute('busy', ''); });
  assert.equal(await input.isDisabled(), true);
  assert.equal(await page.locator('#subject .send').isDisabled(), true);
  assert.equal(await asap.isDisabled(), true);
  await input.dispatchEvent('keydown', { key: 'Enter' });
  await stop.click();
  assert.deepEqual(await events(page), [
    { type: 'asap', detail: { text: 'urgent' }, composed: true },
    { type: 'stop', detail: null, composed: true },
  ]);

  await page.locator('#subject').evaluate((element) => {
    element.removeAttribute('busy');
    element.removeAttribute('stoppable');
  });
  assert.equal(await input.isEnabled(), true);
  assert.equal(await page.locator('#subject .send').isEnabled(), true);
  assert.equal(await stop.isVisible(), false);
  assert.equal(await asap.isVisible(), false);
});

test('composer waits for uploads, allows ready attachments without text, and delegates removal', async ({ page, mount }) => {
  await mount('<adi-composer id="subject" attach></adi-composer>');
  await recordEvents(page, ['send', 'unattach']);
  const ready = { key: 'ready', name: '<notes>.txt', state: 'ready' };
  const upload = { key: 'upload', name: 'photo.png', state: 'uploading' };
  const failed = { key: 'failed', name: 'failed.txt', state: 'failed', error: 'Upload failed' };
  const send = page.locator('#subject .send');
  await setAttachments(page, [ready, upload]);
  await page.locator('#subject textarea').fill('with files');
  assert.equal(await send.isDisabled(), true);
  assert.equal(await page.locator('#subject .line').textContent(), 'attaching…');
  await page.locator('#subject textarea').press('Enter');
  assert.deepEqual(await events(page), []);

  await setAttachments(page, [failed]);
  await page.locator('#subject textarea').fill('');
  assert.equal(await send.isDisabled(), true);
  assert.equal(await page.locator('#subject .thumb').getAttribute('title'), 'failed.txt — Upload failed');
  await setAttachments(page, [ready]);
  assert.equal(await page.locator('#subject .thumb span').textContent(), '<notes>.txt');
  assert.equal(await send.isEnabled(), true);
  await send.click();
  await page.locator('#subject .remove').click();
  assert.deepEqual(await events(page), [
    { type: 'send', detail: { text: '' }, composed: true },
    { type: 'unattach', detail: { key: 'ready' }, composed: true },
  ]);
  assert.equal(await page.locator('#subject .thumb').count(), 1);
  await setAttachments(page, []);
  assert.equal(await page.locator('#subject .thumb').count(), 0);
  assert.equal(await send.isDisabled(), true);
});

test('composer file picker emits File objects and resets so the same file can be picked again', async ({ page, mount }) => {
  await mount('<adi-composer id="subject" attach></adi-composer>');
  await page.evaluate(() => {
    window.fileEvents = [];
    document.body.addEventListener('files', (event) => {
      window.fileEvents.push(event.detail.files.map((file) => ({ name: file.name, type: file.type, size: file.size, isFile: file instanceof File })));
    });
  });
  const file = { name: 'notes.txt', mimeType: 'text/plain', buffer: Buffer.from('hello') };
  const chooserPromise = page.waitForEvent('filechooser');
  await page.locator('#subject .clip').click();
  const chooser = await chooserPromise;
  assert.equal(chooser.isMultiple(), true);
  await chooser.setFiles(file);
  assert.equal(await page.locator('#subject input[type=file]').inputValue(), '');
  await page.locator('#subject input[type=file]').setInputFiles(file);
  assert.deepEqual(await page.evaluate(() => window.fileEvents), [
    [{ name: 'notes.txt', type: 'text/plain', size: 5, isFile: true }],
    [{ name: 'notes.txt', type: 'text/plain', size: 5, isFile: true }],
  ]);
});

test('composer accepts pasted and dropped files only when attachment support is enabled', async ({ page, mount }) => {
  await mount('<adi-composer id="subject" refusal="Files unavailable"></adi-composer>');
  assert.equal(await page.locator('#subject .clip').isVisible(), false);
  assert.equal(await page.locator('#subject .line').textContent(), 'Files unavailable');

  const result = await page.locator('#subject').evaluate((element) => {
    const received = [];
    element.addEventListener('files', (event) => { received.push(event.detail.files.map((file) => file.name)); });
    const transfer = new DataTransfer();
    transfer.items.add(new File(['hello'], 'pasted.txt', { type: 'text/plain' }));
    function dispatch(type, data = transfer) {
      const event = type === 'paste'
        ? new ClipboardEvent(type, { clipboardData: data, bubbles: true, cancelable: true })
        : new DragEvent(type, { dataTransfer: data, bubbles: true, cancelable: true });
      element.shadowRoot.querySelector(type === 'paste' ? 'textarea' : '.box').dispatchEvent(event);
      return event.defaultPrevented;
    }
    const unsupported = ['paste', 'drop', 'dragover'].map((type) => dispatch(type));
    element.setAttribute('attach', '');
    const supported = ['paste', 'drop', 'dragover'].map((type) => dispatch(type));
    const empty = ['paste', 'drop'].map((type) => dispatch(type, new DataTransfer()));
    return { unsupported, supported, empty, received };
  });
  assert.deepEqual(result, {
    unsupported: [false, false, false], supported: [true, true, true], empty: [false, false],
    received: [['pasted.txt'], ['pasted.txt']],
  });
  assert.equal(await page.locator('#subject .clip').isVisible(), true);
  assert.equal(await page.locator('#subject .line').textContent(), '');
});

test('ask submits a single selected option immediately with its question id and typed context', async ({ page, mount }) => {
  await mount('<adi-ask id="subject"></adi-ask>');
  await recordEvents(page, ['answer']);
  assert.equal(await page.locator('#subject').isVisible(), false);
  await setAsk(page, {
    id: 'choice', questions: [{ question: 'Which environment?', options: [{ label: 'Preview', description: 'Try it first' }, { label: 'Production' }] }],
  });
  assert.equal(await page.locator('#subject .foot').isVisible(), false);
  await page.locator('#subject input').fill('  with the current data  ');
  await page.locator('#subject .option').first().click();
  assert.deepEqual(await events(page), [
    { type: 'answer', detail: { id: 'choice', replies: ['Preview — with the current data'] }, composed: true },
  ]);
  assert.equal(await page.locator('#subject .option').first().getAttribute('aria-pressed'), 'true');
});

test('ask rejects empty replies and composing Enter, then submits trimmed free text', async ({ page, mount }) => {
  await mount('<adi-ask id="subject"></adi-ask>');
  await recordEvents(page, ['answer']);
  await setAsk(page, { id: 'text', questions: [{ question: 'What should happen?' }] });
  const input = page.locator('#subject input');
  const send = page.locator('#subject .send');
  assert.equal(await send.isDisabled(), true);
  await input.fill('   ');
  await input.press('Enter');
  assert.equal(await send.isDisabled(), true);
  assert.deepEqual(await events(page), []);
  await input.fill('  Make a preview  ');
  await input.dispatchEvent('keydown', { key: 'Enter', isComposing: true });
  assert.deepEqual(await events(page), []);
  assert.equal(await send.isEnabled(), true);
  await input.press('Enter');
  assert.deepEqual(await events(page), [
    { type: 'answer', detail: { id: 'text', replies: ['Make a preview'] }, composed: true },
  ]);
});

test('ask combines multiple selections in option order and permits unanswered questions', async ({ page, mount }) => {
  await mount('<adi-ask id="subject"></adi-ask>');
  await recordEvents(page, ['answer']);
  await setAsk(page, {
    id: 'multiple', questions: [
      { question: 'Which targets?', multi_select: true, options: [{ label: 'Web' }, { label: 'CLI' }, { label: 'Desktop' }] },
      { question: 'Anything else?' },
    ],
  });
  const options = page.locator('#subject .option');
  await options.nth(2).click();
  await options.nth(0).click();
  await options.nth(1).click();
  await options.nth(1).click();
  assert.equal(await options.nth(1).getAttribute('aria-pressed'), 'false');
  assert.deepEqual(await events(page), []);
  await page.locator('#subject input').first().fill('  after review  ');
  await page.locator('#subject .send').click();
  assert.deepEqual(await events(page), [
    { type: 'answer', detail: { id: 'multiple', replies: ['Web, Desktop — after review', ''] }, composed: true },
  ]);
});

test('ask single-select questions replace and toggle selection without early submission in a group', async ({ page, mount }) => {
  await mount('<adi-ask id="subject"></adi-ask>');
  await recordEvents(page, ['answer']);
  await setAsk(page, {
    id: 'group', questions: [
      { question: 'Choose one', options: [{ label: 'First' }, { label: 'Second' }] },
      { question: 'Optional detail' },
    ],
  });
  const options = page.locator('#subject .option');
  await options.first().click();
  await options.last().click();
  assert.equal(await options.first().getAttribute('aria-pressed'), 'false');
  assert.equal(await options.last().getAttribute('aria-pressed'), 'true');
  assert.deepEqual(await events(page), []);
  await options.last().click();
  assert.equal(await page.locator('#subject .send').isDisabled(), true);
  await options.first().click();
  await page.locator('#subject .send').click();
  assert.deepEqual((await events(page)).map((event) => event.detail), [{ id: 'group', replies: ['First', ''] }]);
});

test('ask keeps answers for the same id, blocks changes while busy, and resets for a new id', async ({ page, mount }) => {
  await mount('<adi-ask id="subject"></adi-ask>');
  await recordEvents(page, ['answer']);
  const ask = { id: 'original', questions: [{ question: 'Choose targets', multi_select: true, options: [{ label: 'Web' }] }] };
  await setAsk(page, ask);
  await page.locator('#subject .option').click();
  await page.locator('#subject input').fill('keep this draft');
  await setAsk(page, ask);
  assert.equal(await page.locator('#subject input').inputValue(), 'keep this draft');
  assert.equal(await page.locator('#subject .option').getAttribute('aria-pressed'), 'true');
  await page.locator('#subject').evaluate((element) => { element.setAttribute('busy', ''); });
  assert.equal(await page.locator('#subject input').isDisabled(), true);
  assert.equal(await page.locator('#subject .option').isDisabled(), true);
  assert.equal(await page.locator('#subject .send').isDisabled(), true);
  await page.locator('#subject input').dispatchEvent('keydown', { key: 'Enter' });
  assert.deepEqual(await events(page), []);
  await page.locator('#subject').evaluate((element) => { element.removeAttribute('busy'); });
  assert.equal(await page.locator('#subject .send').isEnabled(), true);

  await setAsk(page, { ...ask, id: 'next' });
  assert.equal(await page.locator('#subject input').inputValue(), '');
  assert.equal(await page.locator('#subject .option').getAttribute('aria-pressed'), 'false');
  assert.equal(await page.locator('#subject .send').isDisabled(), true);
  await setAsk(page, null);
  assert.equal(await page.locator('#subject').isVisible(), false);
});
