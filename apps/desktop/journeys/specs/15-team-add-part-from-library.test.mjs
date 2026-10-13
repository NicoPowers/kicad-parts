// Add part from our libraries (kicad-parts-37a7): the app reads a copy of
// the CSV catalog, committed in a git repository of its own, in which an
// engineer drew a capacitor symbol and its footprint with a 3D model by
// hand in KiCad. The Library check lists them as not in the Catalog yet;
// Add part from it opens Add part with the symbol chosen, its footprint
// and 3D model come along, and the part saves with the references exactly
// as in the library. The Library check then no longer lists them.

import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { after, before, describe, it } from 'node:test';
import {
  FOOTPRINT,
  SYMBOL,
  assertAccessible,
  images,
  launchApp,
  record,
  repoRoot,
  screenshot,
  startTeamServer,
  tempDir,
  textOf,
  waitForLiveUpdates,
  waitForText,
} from '../harness.mjs';

const BADGE = '.team-settings > summary';
const LINE = '#library-check-open';
const NOTICE = '.team-notice';
const MPN = 'CL10B104KB8NNNC';
const HAND_SYMBOL = 'g-cap:C_Hand';
const HAND_FOOTPRINT = 'g-Capacitor_SMD:C_0603_Hand';
const MODEL = '3d-models/g-Capacitor_SMD.3dshapes/C_0603_Hand.step';
const FOLDERS = ['database', 'symbols', 'footprints', '3d-models'];

/** Runs git in `root` with a fixture identity, no hooks and no signing. */
function git(root, args) {
  const result = spawnSync(
    'git',
    [
      '-C',
      root,
      '-c',
      'user.name=fixture',
      '-c',
      'user.email=fixture@example.test',
      '-c',
      'commit.gpgsign=false',
      '-c',
      'core.autocrlf=false',
      '-c',
      `core.hooksPath=${path.join(root, '.git', 'no-hooks')}`,
      ...args,
    ],
    {
      encoding: 'utf8',
      windowsHide: true,
      // A fixed time, so the commit ID the form shows is the same every run.
      env: {
        ...process.env,
        GIT_AUTHOR_DATE: '2026-10-12T12:00:00Z',
        GIT_COMMITTER_DATE: '2026-10-12T12:00:00Z',
      },
    },
  );
  assert.equal(result.status, 0, `git ${args.join(' ')}: ${result.stderr}`);
  return result.stdout.trim();
}

/** Where the list starting at `open` (its "(") ends, past its ")". */
function listEnd(text, open) {
  let depth = 0;
  for (let i = open; i < text.length; i++) {
    const c = text[i];
    if (c === '"') {
      for (i++; i < text.length && text[i] !== '"'; i++) if (text[i] === '\\') i++;
    } else if (c === '(') {
      depth++;
    } else if (c === ')' && --depth === 0) {
      return i + 1;
    }
  }
  throw new Error('list not closed');
}

/** A committed copy of the CSV catalog with the hand-drawn items. */
function catalogCopy() {
  const copy = tempDir('catalog');
  for (const folder of FOLDERS) {
    fs.cpSync(path.join(repoRoot, folder), path.join(copy, folder), { recursive: true });
  }
  const symbols = path.join(copy, 'symbols/g-cap.kicad_sym');
  const library = fs.readFileSync(symbols, 'utf8');
  const start = library.indexOf('(symbol "C"');
  assert.ok(start > 0);
  const hand = library
    .slice(start, listEnd(library, start))
    .replaceAll('(symbol "C_', '(symbol "C_Hand_')
    .replace('(symbol "C"', '(symbol "C_Hand"')
    .replace('(property "Footprint" ""', `(property "Footprint" "${HAND_FOOTPRINT}"`)
    .replace('"Description" "Unpolarized capacitor"', '"Description" "CAP CER 0.1UF 50V X7R 0603"')
    .replace(
      '\t\t(property "Reference"',
      `\t\t(property "MPN" "${MPN}" (at 0 0 0) (effects (hide yes)))\n` +
        '\t\t(property "Manufacturer" "Samsung Electro-Mechanics" (at 0 0 0) (effects (hide yes)))\n' +
        '\t\t(property "Reference"',
    );
  const close = library.lastIndexOf(')');
  fs.writeFileSync(symbols, `${library.slice(0, close)}\t${hand}\n${library.slice(close)}`);
  const footprint = fs
    .readFileSync(path.join(copy, 'footprints/g-Capacitor_SMD.pretty/C_0603_1608Metric.kicad_mod'), 'utf8')
    .replace('(footprint "C_0603_1608Metric"', '(footprint "C_0603_Hand"')
    .replace('${KICAD9_3DMODEL_DIR}/Capacitor_SMD.3dshapes/C_0603_1608Metric.step', `\${GITPLM_PARTS}/${MODEL}`);
  fs.writeFileSync(path.join(copy, 'footprints/g-Capacitor_SMD.pretty/C_0603_Hand.kicad_mod'), footprint);
  fs.mkdirSync(path.join(copy, '3d-models/g-Capacitor_SMD.3dshapes'), { recursive: true });
  fs.writeFileSync(path.join(copy, MODEL), 'ISO-10303-21;\nEND-ISO-10303-21;\n');
  git(copy, ['init', '-q']);
  git(copy, ['add', '--', ...FOLDERS]);
  git(copy, ['commit', '-q', '-m', 'Drawn by hand in KiCad']);
  return { copy, head: git(copy, ['rev-parse', 'HEAD']) };
}

describe('team catalog: add a part from our libraries', () => {
  let server;
  let browser;
  let editor;
  let head;
  let newItems;

  before(async () => {
    server = await startTeamServer({ csv: ['g-res', 'g-cap'] });
    [editor] = server.accounts;
    const made = catalogCopy();
    head = made.head;
    ({ browser } = await launchApp({
      args: [`--team-server=${server.url}`, `--team-email=${editor.email}`, `--catalog-root=${made.copy}`],
    }));
  });

  after(async () => {
    await browser?.deleteSession();
    await server?.stop();
  });

  async function clickButton(scope, text) {
    for (const button of await browser.$$(`${scope} button`)) {
      if ((await button.getText()).trim() === text && (await button.isDisplayed())) {
        await button.click();
        return;
      }
    }
    assert.fail(`no visible button "${text}" in ${scope}`);
  }

  async function valueOf(selector) {
    return browser.execute((s) => document.querySelector(s)?.value ?? null, selector);
  }

  async function waitForDrawings(selector) {
    await browser.waitUntil(
      async () => {
        const drawn = await images(browser, selector);
        return drawn.length > 0 && drawn.every((image) => image.loaded && image.svg);
      },
      { timeout: 60_000, interval: 100, timeoutMsg: `${selector} was never drawn` },
    );
  }

  it('lists the hand-drawn symbol and footprint, each with Add part from it', async () => {
    await (await browser.$(BADGE)).click();
    await (await browser.$('aria/Password')).setValue(editor.password);
    await (await browser.$('form[aria-label="Sign in to the team catalog"] button[type="submit"]')).click();
    await waitForText(browser, BADGE, `Team catalog · ${editor.name}`);
    await waitForLiveUpdates(browser);
    await browser.execute(() => {
      document.querySelector('.team-settings').open = false;
    });
    await waitForText(browser, LINE, /^Library check: \d+ new items$/);
    newItems = Number((await textOf(browser, LINE)).match(/(\d+) new items/)[1]);
    await (await browser.$(LINE)).click();
    await waitForText(browser, '#library-check-title', 'Library check');
    const buttons = await browser.execute(() =>
      [...document.querySelectorAll('.library-check-add')].map((b) => b.dataset.reference),
    );
    assert.ok(buttons.includes(HAND_SYMBOL), `${buttons}`);
    assert.ok(buttons.includes(HAND_FOOTPRINT), `${buttons}`);
    // Only what is not in the Catalog yet starts a part.
    const groups = await browser.execute(() =>
      [...document.querySelectorAll('.library-check-add')].map((button) =>
        button.closest('.library-check-group').querySelector('h3').textContent.trim(),
      ),
    );
    assert.ok(
      groups.every((group) => group.startsWith('Not in the Catalog yet')),
      `${groups}`,
    );
  });

  it('opens Add part with the symbol chosen; its footprint and 3D model come along', async () => {
    const add = await browser.$(`.library-check-add[data-reference="${HAND_SYMBOL}"]`);
    // Screen readers hear which item.
    assert.equal(
      await browser.execute((b) => b.textContent.replace(/\s+/g, ' ').trim(), add),
      `Add part from it (${HAND_SYMBOL})`,
    );
    await add.click();
    await waitForText(browser, '#add-heading', 'Add part');
    await waitForText(
      browser,
      '#add-download',
      `From our libraries: ${HAND_SYMBOL} and ${HAND_FOOTPRINT}, read from commit ${head.slice(0, 12)}.`,
    );
    assert.equal(await browser.$('#add-from-library').getAttribute('aria-expanded'), 'true');
    await waitForText(browser, '#add-guess', /^Best guess: Capacitors, because /);
    assert.equal(await valueOf('#add-category'), 'cap');
    assert.match(await valueOf('#add-ipn'), /^CAP-\d{4}-0001$/);
    // References exactly as in the library.
    assert.equal(await valueOf('#add-symbol'), HAND_SYMBOL);
    assert.equal(await valueOf('#add-footprint'), HAND_FOOTPRINT);
    assert.equal(await valueOf('#add-mpn'), MPN);
    assert.equal(await browser.$('#add-from').isExisting(), false);
    const checks = await textOf(browser, '.add-checks');
    record({ event: 'library-checks', text: checks });
    assert.match(checks, new RegExp(`The footprint's 3D model ${MODEL} from our library is saved with it\\.`));
    assert.match(checks, /The footprint is the one the symbol names, g-Capacitor_SMD:C_0603_Hand\./);
    assert.match(checks, /The symbol's 2 pins match the footprint's numbered pads\./);
    assert.equal(await browser.execute(() => document.querySelectorAll('.check-error').length), 0);
    // The search over our libraries shows both, the symbol chosen.
    await (await browser.$('#add-library-search')).setValue('hand');
    await browser.waitUntil(
      async () =>
        (await browser.execute(() => [...document.querySelectorAll('.library-result')].map((b) => b.textContent))).length === 2,
      { timeout: 5_000, timeoutMsg: 'the search did not narrow the list' },
    );
    const results = await browser.execute(() =>
      [...document.querySelectorAll('.library-result')].map((b) => [
        b.querySelector('.library-kind').textContent.trim(),
        b.querySelector('code').textContent.trim(),
        b.getAttribute('aria-pressed'),
      ]),
    );
    // Screen readers hear how many match.
    assert.equal(
      await textOf(browser, '.library-picker [role="status"]'),
      '2 symbols and footprints match.',
    );
    assert.deepEqual(results, [
      ['Symbol', HAND_SYMBOL, 'true'],
      // Brought by the symbol, so chosen too.
      ['Footprint', HAND_FOOTPRINT, 'true'],
    ]);
    await waitForDrawings(`${SYMBOL} .file-preview img`);
    await waitForDrawings(`${FOOTPRINT} .file-preview img`);
    await browser.execute(() => {
      document.querySelector('.part-fields').scrollTop = 0;
    });
    await assertAccessible(browser, '63-team-add-part-from-library');
    await screenshot(browser, '63-team-add-part-from-library');
  });

  it('saves the part with the references as in the library, and the Library check no longer lists them', async () => {
    const ipn = await valueOf('#add-ipn');
    await (await browser.$('#add-reason')).setValue('Drawn in KiCad for the sensor board');
    await clickButton('.add-form', 'Save');
    await waitForText(browser, NOTICE, `Added ${ipn} as revision 1.`, 30_000);
    await waitForText(browser, '#part-title', MPN);
    await clickButton('.part-fields', 'History (1)');
    await (await browser.$('.history li:nth-child(1) button')).click();
    const rows = await browser.execute(() =>
      [...document.querySelectorAll('.revision .changes tbody tr')].map((tr) =>
        [...tr.children].map((cell) => cell.textContent.trim()),
      ),
    );
    record({ event: 'library-history', rows });
    assert.equal(rows[0][0], 'Added from');
    assert.match(rows[0][1], /^Our libraries: symbols\/g-cap\.kicad_sym at [0-9a-f]{12}, /);
    await waitForText(browser, LINE, `Library check: ${newItems - 2} new items`, 30_000);
  });
});
