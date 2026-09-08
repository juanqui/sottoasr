import fs from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import { createHash } from 'node:crypto';
import { pathToFileURL } from 'node:url';
import { installFixture } from './fixture.mjs';

const server = new URL(process.env.SOTTO_UI_SERVER ?? 'http://127.0.0.1:14517');
if (!['127.0.0.1', 'localhost', '[::1]'].includes(server.hostname)) throw new Error('Use a loopback Vite server.');
const label = process.env.SOTTO_UI_LABEL ?? 'current';
if (!/^[a-zA-Z0-9_-]+$/.test(label)) throw new Error('SOTTO_UI_LABEL must be a filename-safe label.');
const layout = process.env.SOTTO_UI_LAYOUT ?? 'current';
if (!['legacy', 'current'].includes(layout)) throw new Error('SOTTO_UI_LAYOUT must be legacy or current.');
const output = process.env.SOTTO_UI_OUTPUT ?? await fs.mkdtemp(path.join(os.tmpdir(), 'sotto-ui-'));
const source = path.resolve(process.env.SOTTO_UI_SOURCE ?? '.');
const playwrightPath = process.env.SOTTO_UI_PLAYWRIGHT;
let chromium;
try {
  ({ chromium } = await import(playwrightPath ? pathToFileURL(path.resolve(playwrightPath)).href : 'playwright'));
} catch (error) {
  throw new Error('Playwright is optional tooling. Set SOTTO_UI_PLAYWRIGHT to its index.mjs; see benchmarks/ui/README.md.', { cause: error });
}
await fs.mkdir(output, { recursive: true });
const browser = await chromium.launch({
  ...(process.env.SOTTO_UI_BROWSER_EXECUTABLE ? { executablePath: process.env.SOTTO_UI_BROWSER_EXECUTABLE } : { channel: 'chrome' }),
  headless: true,
});

async function sourceFingerprint() {
  const hash = createHash('sha256');
  async function visit(relative) {
    for (const entry of (await fs.readdir(path.join(source, relative), { withFileTypes: true })).sort((a, b) => a.name.localeCompare(b.name))) {
      const name = path.join(relative, entry.name);
      if (entry.isDirectory()) await visit(name);
      else if (entry.isFile()) hash.update(name).update('\0').update(await fs.readFile(path.join(source, name))).update('\0');
    }
  }
  await visit('src');
  return hash.digest('hex');
}

async function newPage(options = {}) {
  const context = await browser.newContext({ viewport: { width: 520, height: 600 } });
  // Prevent fixture pages from making external requests or reaching native state.
  await context.route('**/*', (route) => new URL(route.request().url()).origin === server.origin ? route.continue() : route.abort());
  await context.addInitScript(installFixture, options);
  const page = await context.newPage();
  page.setDefaultTimeout(10_000);
  return { page, context };
}

async function metrics(page) {
  const value = await page.evaluate(() => window.__uiBench);
  if (value.errors.length || value.unknownCommands.length) throw new Error(JSON.stringify(value));
  return value;
}

const results = {
  schemaVersion: 1, label, layout, recordedAt: new Date().toISOString(),
  browser: await browser.version(), node: process.version, platform: process.platform, architecture: process.arch,
  mode: 'Vite development, synthetic IPC, isolated headless Chromium',
  sourceFingerprint: null, history: [], settings: [], overlay: null,
};
try {
  results.sourceFingerprint = await sourceFingerprint();
  for (const historyCount of [500, 5000]) {
    const { page, context } = await newPage({ historyCount, label: 'history' });
    const start = performance.now();
    await page.goto(new URL('history.html', server).href);
    await page.waitForFunction((count) => document.querySelectorAll('.history-item').length === count,
      layout === 'current' ? Math.min(historyCount, 50) : historyCount);
    const readyMs = performance.now() - start;
    const timings = await page.evaluate(async () => {
      const input = document.querySelector('.search-input');
      const values = [];
      for (const query of ['synthetic', 'Item 499', '', 'absent term', '']) {
        const start = performance.now();
        input.value = query;
        input.dispatchEvent(new Event('input', { bubbles: true }));
        await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
        values.push({ query, toSecondFrameMs: performance.now() - start, visibleItems: document.querySelectorAll('.history-item').length });
      }
      return { nodes: document.querySelectorAll('*').length, values };
    });
    results.history.push({ historyCount, readyMs, ...timings, ...await metrics(page) });
    await context.close();
  }
  for (const dictionaryCount of [0, 200]) {
    const { page, context } = await newPage({ dictionaryCount });
    await page.goto(new URL('settings.html', server).href);
    if (layout === 'current') {
      await page.getByRole('tab', { name: 'Vocabulary' }).click();
      await page.getByRole('textbox', { name: 'Correct spelling', exact: true }).waitFor();
    } else await page.getByText('AI Transcript Cleanup', { exact: true }).waitFor();
    const timings = await page.evaluate(async () => {
      const input = document.querySelector('input[aria-label="Heard alias 1"]') ?? document.querySelector('#max-history') ?? document.querySelector('input[placeholder^="Qwen"]');
      if (!input) throw new Error('Expected Settings editing control not found.');
      const values = [];
      for (let index = 0; index < 5; index++) {
        const start = performance.now();
        input.value = input.type === 'number' ? String(500 + index) : `Updated alias ${index}`;
        input.dispatchEvent(new Event('input', { bubbles: true }));
        await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
        values.push(performance.now() - start);
      }
      return { nodes: document.querySelectorAll('*').length, control: input.getAttribute('aria-label') || input.id || input.placeholder, toSecondFrameMs: values };
    });
    await page.screenshot({ path: path.join(output, `${label}-settings-${dictionaryCount}.png`) });
    results.settings.push({ dictionaryCount, ...timings, ...await metrics(page) });
    await context.close();
  }
  const { page, context } = await newPage({ label: 'overlay' });
  await page.goto(new URL('overlay.html', server).href);
  await page.locator('canvas').waitFor({ state: 'attached' });
  await page.waitForTimeout(100); // Let one-time mount/resize work settle before the idle window.
  const before = await metrics(page);
  await page.waitForTimeout(1000);
  const after = await metrics(page);
  const batches = await page.evaluate(async (layout) => {
    if (layout === 'current') window.__emit('overlay-state', { revision: 1, generation: 1, state: 'Recording', started_at_ms: Date.now(), error: null });
    else window.__emit('state-changed', 'Recording');
    await Promise.resolve();
    const values = [];
    for (let batch = 0; batch < 10; batch++) {
      const start = performance.now();
      for (let index = 0; index < 300; index++) window.__emit('audio-level', { level: 0.01 });
      await Promise.resolve();
      values.push({ samples: (batch + 1) * 300, ms: performance.now() - start });
    }
    if (layout === 'current') window.__emit('overlay-state', { revision: 2, generation: 1, state: 'Idle', started_at_ms: null, error: null });
    else window.__emit('state-changed', 'Idle');
    return values;
  }, layout);
  results.overlay = { idleOneSecond: { raf: after.raf - before.raf, draw: after.draw - before.draw }, batches, ...await metrics(page) };
  await context.close();
  if (await sourceFingerprint() !== results.sourceFingerprint) throw new Error('Frontend source changed during the run; repeat against a stable checkout.');
  await fs.writeFile(path.join(output, `${label}.json`), JSON.stringify(results, null, 2));
  console.log(JSON.stringify(results, null, 2));
  console.error(`Synthetic UI artifacts: ${output}`);
} finally {
  await browser.close();
}
