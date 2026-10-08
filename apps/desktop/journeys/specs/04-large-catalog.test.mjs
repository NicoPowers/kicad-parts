// Search stays interactive on a 20,000-row catalog. The rows are synthetic
// and written to a temporary catalog root, never to the repository.

import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { after, before, describe, it } from 'node:test';
import {
  assertAccessible,
  launchApp,
  record,
  screenshot,
  searchFor,
  tempDir,
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
  if (total > 200) return `Showing 200 of ${n} matching parts`;
  return total === 1 ? '1 matching part' : `${n} matching parts`;
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
    await waitForText(browser, '.catalog-summary', '20,000 parts in 1 category');
    await waitForText(browser, '#result-count', 'Showing 200 of 20,000 matching parts');
    const timings = {};
    for (const query of ['0603', 'maker 7 soic', 'MPN019999', 'no-such-row']) {
      timings[query] = await searchFor(browser, query, expectedCountText(rows, query));
    }
    record({ event: 'large-catalog', rows: ROWS, timingsMs: timings, targetMs: TARGET_MS });
    for (const [query, ms] of Object.entries(timings)) {
      assert.ok(ms < TARGET_MS, `"${query}" took ${ms} ms (target ${TARGET_MS} ms)`);
    }
    await searchFor(browser, '0603', expectedCountText(rows, '0603'));
    await assertAccessible(browser, '30-large-catalog');
    await screenshot(browser, '30-large-catalog');
  });
});
