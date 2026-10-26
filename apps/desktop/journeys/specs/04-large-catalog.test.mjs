// Search stays interactive over a team of 20,000 parts. The parts are made
// up and written to the journey's own data folder as that team's
// last-known parts (ADR 0020). Its server is not running, so the app shows
// them offline, as it would after a restart away from the network. The
// part list searches a team's parts the same way online or offline.

import assert from 'node:assert/strict';
import fs from 'node:fs';
import net from 'node:net';
import path from 'node:path';
import { after, before, describe, it } from 'node:test';
import {
  assertAccessible,
  expectFocusKeptOnEnter,
  launchApp,
  record,
  screenshot,
  searchFor,
  tempDir,
  textOf,
  waitForText,
} from '../harness.mjs';

const PARTS = 20_000;
/** Product target: results replace the list within this time of typing,
 * including the UI's 120 ms debounce, IPC and rendering. */
const TARGET_MS = 500;

/** One made-up part: [IPN, MPN, manufacturer, description, value, symbol,
 * footprint]. */
function syntheticParts() {
  const packages = ['0402', '0603', '0805', 'SOT-23', 'SOIC-8', 'QFN-32'];
  return Array.from({ length: PARTS }, (_, n) => {
    const pkg = packages[n % packages.length];
    return [
      `SYN-${String(Math.floor(n / 10_000)).padStart(4, '0')}-${String(n % 10_000).padStart(4, '0')}`,
      `MPN${String(n).padStart(6, '0')}`,
      `Maker ${n % 37}`,
      `Synthetic part ${n} in ${pkg}`,
      `${n % 1000}k`,
      'Device:R',
      `Resistor_SMD:R_${pkg}_1005Metric`,
    ];
  });
}

/** A local address no server answers: a port that was free a moment ago. */
async function closedAddress() {
  const server = net.createServer();
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  const { port } = server.address();
  await new Promise((resolve) => server.close(resolve));
  return `http://127.0.0.1:${port}`;
}

/** The parts file's name for `url`, as the app makes it (file_safe.rs). */
function fileSafe(text) {
  return [...text]
    .map((ch) => (/[a-z0-9.-]/.test(ch) ? ch : `_${ch.codePointAt(0).toString(16)}_`))
    .join('');
}

/**
 * A data folder in which the app remembers one team, signed in, whose
 * last-known parts are `parts` (each [category, ...syntheticParts row]).
 * Its server at `url` is not running. The sign-in is a made-up token in
 * the debug build's test file (--sign-ins=test-file), never the person's
 * credential store.
 */
function dataDirWithOfflineTeam(url, parts) {
  const dataDir = tempDir('data');
  fs.mkdirSync(path.join(dataDir, 'config'), { recursive: true });
  fs.writeFileSync(
    path.join(dataDir, 'config', 'team.json'),
    JSON.stringify({ teams: [{ server_url: url, email: 'editor@example.test', code: 'SY', colour: 'blue' }] }),
  );
  fs.writeFileSync(path.join(dataDir, 'test-sign-ins.json'), JSON.stringify({ [url]: 'made-up-token' }));
  const values = {};
  for (const [category, ipn, mpn, manufacturer, description, value, symbol, footprint] of parts) {
    values[ipn] = { category, mpn, manufacturer, description, value, symbol, footprint };
  }
  fs.mkdirSync(path.join(dataDir, 'parts-files'), { recursive: true });
  fs.writeFileSync(
    path.join(dataDir, 'parts-files', `${fileSafe(url)}.json`),
    JSON.stringify({
      version: 1,
      server_url: url,
      as_of_ms: Date.UTC(2026, 9, 26, 12),
      category_names: {},
      parts: values,
    }),
  );
  return dataDir;
}

async function launchWithOfflineTeam(parts) {
  const url = await closedAddress();
  const dataDir = dataDirWithOfflineTeam(url, parts);
  return launchApp({ dataDir, args: ['--sign-ins=test-file'] });
}

/** The count line the app should show, from an independent count of parts
 * whose fields contain every query term (case-insensitive). */
function expectedCountText(rows, query) {
  const terms = query.toLowerCase().split(/\s+/).filter(Boolean);
  const total = rows.filter((row) => {
    const fields = row.map((field) => field.toLowerCase());
    return terms.every((term) => fields.some((field) => field.includes(term)));
  }).length;
  if (total === 0) return 'No parts match.';
  const n = total.toLocaleString('en-US');
  return total === 1 ? '1 matching part' : `${n} matching parts`;
}

/** How many parts the list draws. */
function partsShown(browser) {
  return browser.execute(() => document.querySelectorAll('.results button.result').length);
}

describe(`search over ${PARTS} parts`, () => {
  let browser;
  const rows = syntheticParts();

  before(async () => {
    ({ browser } = await launchWithOfflineTeam(rows.map((row) => ['res', ...row])));
  });

  after(async () => {
    await browser?.deleteSession();
  });

  it('answers each query within the product target', async () => {
    await waitForText(browser, '#result-count', '20,000 matching parts', 60_000);
    const timings = {};
    for (const query of ['0603', 'maker 7 soic', 'MPN019999', 'no-such-row']) {
      timings[query] = await searchFor(browser, query, expectedCountText(rows, query));
    }
    record({ event: 'large-catalog', parts: PARTS, timingsMs: timings, targetMs: TARGET_MS });
    for (const [query, ms] of Object.entries(timings)) {
      assert.ok(ms < TARGET_MS, `"${query}" took ${ms} ms (target ${TARGET_MS} ms)`);
    }
    await searchFor(browser, '0603', expectedCountText(rows, '0603'));
    // While searching, a section shows its first 25 matches, then "Show
    // all" shows up to 200 and says how many there are.
    assert.equal(await partsShown(browser), 25);
    // Chosen with the keyboard, Show all moves focus to the first newly
    // shown part, so focus never drops to the page.
    await browser.execute(() => document.querySelector('.section-show-all').focus());
    await browser.keys('Enter');
    await browser.waitUntil(async () => (await partsShown(browser)) === 200, {
      timeout: 5_000,
      timeoutMsg: 'Show all did not show 200 parts',
    });
    await browser.waitUntil(
      async () =>
        browser.execute(() => document.activeElement === document.querySelector('.section-parts > li:nth-child(26) button.result')),
      { timeout: 5_000, timeoutMsg: 'focus did not move to the 26th part after Show all' },
    );
    assert.equal(await textOf(browser, '.section-more'), 'Showing 200 of 3,343. Search to narrow the list.');
    await browser.execute(() => document.querySelector('.section-more').scrollIntoView({ block: 'end' }));
    await assertAccessible(browser, '30-large-catalog');
    await screenshot(browser, '30-large-catalog');
    // A part beyond the first 25, chosen with the keyboard, keeps focus.
    await expectFocusKeptOnEnter(browser, '.section-parts li:nth-child(30) button.result');
  });
});

// The same, spread over 20 categories of 1,000 parts each: a broad search
// opens every section, and opening a large section loads its parts.
const CATEGORIES = 20;
const PER_CATEGORY = 1_000;

describe(`search over ${CATEGORIES} categories of ${PER_CATEGORY} parts`, () => {
  let browser;

  before(async () => {
    const parts = [];
    for (let c = 0; c < CATEGORIES; c++) {
      const code = `c${String(c).padStart(2, '0')}`;
      for (let n = 0; n < PER_CATEGORY; n++) {
        parts.push([
          code,
          `CAT-${String(c).padStart(4, '0')}-${String(n).padStart(4, '0')}`,
          `M${c}X${n}`,
          `Maker ${n % 37}`,
          `Part ${n} of ${code}`,
          `${n}k`,
          'Device:R',
          'Resistor_SMD:R_0603_1608Metric',
        ]);
      }
    }
    ({ browser } = await launchWithOfflineTeam(parts));
  });

  after(async () => {
    await browser?.deleteSession();
  });

  it('answers a broad search and opens a large section within the product target', async () => {
    await waitForText(browser, '#result-count', '20,000 matching parts', 60_000);
    assert.equal(await partsShown(browser), 0, 'every section starts closed, so no part is drawn');
    const timings = {};
    // "0" matches every part (each IPN has a 0), so every section opens.
    timings['0'] = await searchFor(browser, '0', '20,000 matching parts');
    const sections = await browser.execute(() => document.querySelectorAll('.section-toggle[aria-expanded="true"]').length);
    assert.equal(sections, CATEGORIES);
    assert.equal(await partsShown(browser), CATEGORIES * 25);
    timings['m7x9'] = await searchFor(browser, 'm7x9', /^\d+ matching parts?$/);
    await searchFor(browser, '', '20,000 matching parts');
    // Opening a closed section of 1,000 parts draws its first 200.
    const started = Date.now();
    await (await browser.$('.section-toggle')).click();
    await browser.waitUntil(async () => (await partsShown(browser)) === 200, {
      timeout: 5_000,
      interval: 10,
      timeoutMsg: 'the opened section never showed 200 parts',
    });
    timings['open a section'] = Date.now() - started;
    record({ event: 'many-categories', categories: CATEGORIES, partsPerCategory: PER_CATEGORY, timingsMs: timings, targetMs: TARGET_MS });
    for (const [what, ms] of Object.entries(timings)) {
      assert.ok(ms < TARGET_MS, `"${what}" took ${ms} ms (target ${TARGET_MS} ms)`);
    }
  });
});
