// Add part from our libraries (kicad-parts-37a7, kicad-parts-bc6f): the app
// reads a copy of the sample library, committed in a git repository of its
// own, in which an engineer drew a capacitor symbol and its footprint with a
// 3D model by hand in KiCad. The Library check lists them as not in the Catalog yet;
// Add part from it opens Add part with the symbol chosen, its footprint
// and 3D model come along, and the part saves with the references exactly
// as in the library. The Library check then no longer lists them.

import assert from 'node:assert/strict';
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
  sampleLibraryCopy,
  screenshot,
  startTeamServer,
  textOf,
  waitForLibraries,
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

/** A committed copy of the sample library with the hand-drawn items. */
function catalogCopy() {
  return sampleLibraryCopy('catalog', (copy) => {
    const symbols = path.join(copy, 'symbols/g-cap.kicad_sym');
    const library = fs.readFileSync(symbols, 'utf8');
    const start = library.indexOf('(symbol "C"');
    assert.ok(start > 0);
    const hand = library
      .slice(start, listEnd(library, start))
      .replaceAll('(symbol "C_', '(symbol "C_Hand_')
      .replace('(symbol "C"', '(symbol "C_Hand"')
      .replace('(property "Footprint" ""', `(property "Footprint" "${HAND_FOOTPRINT}"`)
      .replace('"Description" "Capacitor, not polarised"', '"Description" "CAP CER 0.1UF 50V X7R 0603"')
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
      .replace('${GITPLM_PARTS}/3d-models/g-Capacitor_SMD.3dshapes/C_0603_1608Metric.step', `\${GITPLM_PARTS}/${MODEL}`);
    fs.writeFileSync(path.join(copy, 'footprints/g-Capacitor_SMD.pretty/C_0603_Hand.kicad_mod'), footprint);
    fs.writeFileSync(path.join(copy, MODEL), 'ISO-10303-21;\nEND-ISO-10303-21;\n');
  });
}

describe('team catalog: add a part from our libraries', () => {
  let server;
  let browser;
  let editor;
  let head;
  let newItems;

  before(async () => {
    server = await startTeamServer();
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
    await waitForText(browser, '#add-download', `From the libraries: ${HAND_SYMBOL} and ${HAND_FOOTPRINT}.`);
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
    assert.match(checks, new RegExp(`${HAND_SYMBOL} is read from commit ${head.slice(0, 12)}\\.`));
    assert.match(checks, /The symbol's 2 pins match the footprint's numbered pads\./);
    assert.equal(await browser.execute(() => document.querySelectorAll('.check-error').length), 0);
    // Both say how they are used: added from our libraries when saved.
    const uses = await browser.execute(() =>
      [...document.querySelectorAll('.add-form .cad-actions .file-name')].map((p) => p.textContent.trim()),
    );
    assert.deepEqual(uses, [
      'From our libraries: added to the Catalog when you save, as committed.',
      'From our libraries: added to the Catalog when you save, as committed.',
    ]);
    // Choose... opens the Libraries browser for symbols only, on the chosen
    // symbol, with the list focused so Enter would choose it again; Escape
    // closes it, chooses nothing, and focus comes back.
    await browser.execute(() => document.getElementById('add-symbol-choose').scrollIntoView({ block: 'center' }));
    await (await browser.$('#add-symbol-choose')).click();
    await waitForLibraries(browser);
    await waitForText(browser, '#library-detail-title', HAND_SYMBOL);
    await browser.waitUntil(async () => (await browser.execute(() => document.activeElement?.id)) === 'library-items', {
      timeout: 5_000,
      timeoutMsg: 'the list did not take the focus',
    });
    assert.deepEqual(
      await browser.execute(() =>
        [...document.querySelectorAll('.libraries-dialog [role="tab"]')].filter((tab) => !tab.hidden).map((tab) => tab.textContent.trim()),
      ),
      ['Symbols'],
    );
    await browser.keys(['Escape']);
    await waitForLibraries(browser, { open: false });
    await browser.waitUntil(async () => (await browser.execute(() => document.activeElement?.id)) === 'add-symbol-choose', {
      timeout: 5_000,
      timeoutMsg: 'focus did not come back to Choose…',
    });
    assert.equal(await valueOf('#add-symbol'), HAND_SYMBOL, 'closing chooses nothing');
    await waitForDrawings(`${SYMBOL} .file-preview img`);
    await waitForDrawings(`${FOOTPRINT} .file-preview img`);
    await browser.execute(() => {
      document.querySelector('.part-fields').scrollTop = 0;
    });
    await assertAccessible(browser, '63-team-add-part-from-library');
    // The commit ID changes with every change to the CSV catalog's files,
    // and is checked above, so it is blanked out: each time it appears, its
    // text goes in a span of its own for the screenshot.
    const commit = head.slice(0, 12);
    const wrapped = await browser.execute((id) => {
      const walker = document.createTreeWalker(document.querySelector('.add-checks'), NodeFilter.SHOW_TEXT);
      const found = [];
      for (let node = walker.nextNode(); node; node = walker.nextNode()) {
        if (node.textContent.includes(id)) found.push(node);
      }
      for (const node of found) {
        const rest = node.splitText(node.textContent.indexOf(id));
        rest.splitText(id.length);
        const span = document.createElement('span');
        span.dataset.journeyCommit = '';
        rest.replaceWith(span);
        span.append(rest);
      }
      return found.length;
    }, commit);
    assert.equal(wrapped, 2, `the commit ID ${commit} is in the symbol's and the footprint's checks`);
    await screenshot(browser, '63-team-add-part-from-library', { blankOut: ['[data-journey-commit]'] });
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
    // The two hand-drawn items were all that was new in the sample library,
    // so with nothing left to say, the line goes.
    assert.equal(newItems, 2);
    await browser.waitUntil(async () => (await browser.$$(LINE)).length === 0, {
      timeout: 30_000,
      timeoutMsg: 'the library check line stayed after its items were added',
    });
  });
});
