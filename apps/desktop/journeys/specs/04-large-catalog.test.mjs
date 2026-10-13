// Search stays interactive on a 20,000-row catalog. The rows are synthetic
// and written to a temporary catalog root, never to the repository.

import assert from 'node:assert/strict';
import fs from 'node:fs';
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

const ROWS = 20_000;
/** Product target: results replace the list within this time of typing,
 * including the UI's 120 ms debounce, IPC and rendering. */
const TARGET_MS = 500;

function syntheticRows() {
  const packages = ['0402', '0603', '0805', 'SOT-23', 'SOIC-8', 'QFN-32'];
  return Array.from({ length: ROWS }, (_, n) => {
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

function writeSyntheticCatalog(root, rows) {
  const database = path.join(root, 'database');
  fs.mkdirSync(database, { recursive: true });
  const header = 'IPN,MPN,Manufacturer,Description,Value,Symbol,Footprint';
  const lines = [header, ...rows.map((row) => row.join(','))];
  fs.writeFileSync(path.join(database, 'g-syn.csv'), `${lines.join('\n')}\n`);
}

/** The count line the app should show, from an independent count of rows
 * whose fields contain every query term (case-insensitive). */
function expectedCountText(rows, query) {
  const terms = query.toLowerCase().split(/\s+/).filter(Boolean);
  const total = rows.filter((row) => {
    const fields = [...row, 'g-syn'].map((field) => field.toLowerCase());
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

describe(`search over ${ROWS} rows`, () => {
  let browser;
  const rows = syntheticRows();

  before(async () => {
    const root = tempDir('large-catalog');
    writeSyntheticCatalog(root, rows);
    ({ browser } = await launchApp({ args: [`--catalog-root=${root}`] }));
  });

  after(async () => {
    await browser?.deleteSession();
  });

  it('answers each query within the product target', async () => {
    await waitForText(browser, '.catalog-summary', '20,000 parts from 1 CSV file');
    await waitForText(browser, '#result-count', '20,000 matching parts');
    const timings = {};
    for (const query of ['0603', 'maker 7 soic', 'MPN019999', 'no-such-row']) {
      timings[query] = await searchFor(browser, query, expectedCountText(rows, query));
    }
    record({ event: 'large-catalog', rows: ROWS, timingsMs: timings, targetMs: TARGET_MS });
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
const FILES = 20;
const PER_FILE = 1_000;

describe(`search over ${FILES} CSV files of ${PER_FILE} rows`, () => {
  let browser;
  const header = 'IPN,MPN,Manufacturer,Description,Value,Symbol,Footprint';

  before(async () => {
    const root = tempDir('many-categories');
    const database = path.join(root, 'database');
    fs.mkdirSync(database, { recursive: true });
    for (let f = 0; f < FILES; f++) {
      const code = `c${String(f).padStart(2, '0')}`;
      const lines = Array.from({ length: PER_FILE }, (_, n) =>
        [`CAT-${String(f).padStart(4, '0')}-${String(n).padStart(4, '0')}`, `M${f}X${n}`, `Maker ${n % 37}`, `Part ${n} of ${code}`, `${n}k`, 'Device:R', 'Resistor_SMD:R_0603_1608Metric'].join(','),
      );
      fs.writeFileSync(path.join(database, `g-${code}.csv`), `${[header, ...lines].join('\n')}\n`);
    }
    ({ browser } = await launchApp({ args: [`--catalog-root=${root}`] }));
  });

  after(async () => {
    await browser?.deleteSession();
  });

  it('answers a broad search and opens a large section within the product target', async () => {
    await waitForText(browser, '#result-count', '20,000 matching parts');
    assert.equal(await partsShown(browser), 0, 'every section starts closed, so no part is drawn');
    const timings = {};
    // "0" matches every part (each IPN has a 0), so every section opens.
    timings['0'] = await searchFor(browser, '0', '20,000 matching parts');
    const sections = await browser.execute(() => document.querySelectorAll('.section-toggle[aria-expanded="true"]').length);
    assert.equal(sections, FILES);
    assert.equal(await partsShown(browser), FILES * 25);
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
    record({ event: 'many-categories', files: FILES, rowsPerFile: PER_FILE, timingsMs: timings, targetMs: TARGET_MS });
    for (const [what, ms] of Object.entries(timings)) {
      assert.ok(ms < TARGET_MS, `"${what}" took ${ms} ms (target ${TARGET_MS} ms)`);
    }
  });
});
