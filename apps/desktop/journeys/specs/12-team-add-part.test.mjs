// Add part (kicad-parts-9693): choose a downloaded ZIP file (built here,
// in the layout SamacSys uses, from the repository's own capacitor symbol
// and footprint), see the guessed category with its reason and the next
// free IPN, save, and find the new part in the search with its drawings
// and history. Then a download whose MPN the Catalog already has shows
// that part, which opens.

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
  sampleLibrary,
  screenshot,
  searchFor,
  startTeamServer,
  tempDir,
  textOf,
  waitForLiveUpdates,
  waitForSelectedInList,
  waitForText,
  zipOf,
} from '../harness.mjs';

const MPN = 'CL10B104KB8NNNC';
const KNOWN_MPN = 'CL10C020CB8NNNC';
const IPN = 'CAP-0004-0001';
const BADGE = '.team-settings > summary';
const COLUMN = '.part-fields';
const NOTICE = '.team-notice';

/** The sample library's capacitor symbol and footprint, renamed, as a vendor ZIP. */
function download(folder, mpn) {
  const read = (file) => fs.readFileSync(path.join(sampleLibrary, file), 'utf8');
  const symbol = read('symbols/g-cap.kicad_sym')
    .replaceAll('(symbol "C_', '(symbol "C_New_')
    .replace('(symbol "C"', '(symbol "C_New"')
    .replace('"Value" "C"', '"Value" "100n"')
    .replace('"Datasheet" ""', '"Datasheet" "https://example.test/cl10b104.pdf"')
    .replace('"Description" "Capacitor, not polarised"', '"Description" "CAP CER 0.1UF 50V X7R 0603"')
    .replace(
      '\t\t(property "Reference"',
      `\t\t(property "Manufacturer_Name" "Samsung Electro-Mechanics" (at 0 0 0) (effects (hide yes)))\n` +
        `\t\t(property "Manufacturer_Part_Number" "${mpn}" (at 0 0 0) (effects (hide yes)))\n` +
        `\t\t(property "Mouser Part Number" "187-${mpn}" (at 0 0 0) (effects (hide yes)))\n` +
        '\t\t(property "Reference"',
    );
  const footprint = read('footprints/g-Capacitor_SMD.pretty/C_0603_1608Metric.kicad_mod')
    .replace('(footprint "C_0603_1608Metric"', '(footprint "C_0603_New"')
    .replace('${GITPLM_PARTS}/3d-models/g-Capacitor_SMD.3dshapes/C_0603_1608Metric.step', '${KIPRJMOD}/3D/C_0603_New.stp');
  const zip = zipOf([
    ['CL10B104/KiCad/C_New.kicad_sym', Buffer.from(symbol, 'utf8')],
    ['CL10B104/KiCad/C_0603_New.kicad_mod', Buffer.from(footprint, 'utf8')],
    ['CL10B104/3D/C_0603_New.stp', Buffer.from('ISO-10303-21;\nEND-ISO-10303-21;\n')],
    ['CL10B104/Altium/C_New.SchLib', Buffer.from('not KiCad')],
  ]);
  const file = path.join(folder, `${mpn}.zip`);
  fs.writeFileSync(file, zip);
  return file;
}

describe('team catalog: add a part from a download', () => {
  let server;
  let browser;
  let editor;
  let files;

  before(async () => {
    server = await startTeamServer();
    [editor] = server.accounts;
    files = tempDir('download');
    ({ browser } = await launchApp({
      args: [`--team-server=${server.url}`, `--team-email=${editor.email}`],
    }));
  });

  after(async () => {
    await browser?.deleteSession();
    await server?.stop();
  });

  async function clickButton(scope, text) {
    const buttons = await browser.$$(`${scope} button`);
    for (const button of buttons) {
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

  async function fieldValue(name) {
    return browser.execute(
      (label) => {
        const found = [...document.querySelectorAll('.add-form .edit-row label')].find(
          (l) => l.textContent.trim() === label,
        );
        return found ? document.getElementById(found.htmlFor)?.value ?? null : null;
      },
      name,
    );
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

  it('signs in, and Add part is offered to someone who can edit', async () => {
    await (await browser.$(BADGE)).click();
    await (await browser.$('aria/Password')).setValue(editor.password);
    await (await browser.$('form[aria-label="Sign in to the team catalog"] button[type="submit"]')).click();
    await waitForText(browser, BADGE, `Team catalog · ${editor.name}`);
    await waitForLiveUpdates(browser);
    await browser.execute(() => {
      document.querySelector('.team-settings').open = false;
    });
    await (await browser.$('#add-part-open')).click();
    await waitForText(browser, '#add-heading', 'Add part');
    // The form takes the keyboard focus.
    await browser.waitUntil(async () => (await browser.execute(() => document.activeElement?.id)) === 'add-files', {
      timeout: 5_000,
      timeoutMsg: 'focus did not move into the form',
    });
  });

  it('reads a ZIP file, guesses the category with its reason and offers the next free IPN', async () => {
    await (await browser.$('#add-files')).addValue(download(files, MPN));
    // The sample capacitors were added by people, so their reference
    // letter C is a remembered clue, which ranks first.
    await waitForText(browser, '#add-guess', /^Best guess: Capacitors, because people chose this category \d+ times for reference letter C\.$/);
    record({ event: 'guess', text: await textOf(browser, '#add-guess') });
    assert.equal(await valueOf('#add-category'), 'cap');
    assert.equal(await valueOf('#add-ipn'), IPN);
    await waitForText(browser, '#add-ipn-hint', 'The next free IPN in this category. An IPN is never reused.');
    // What the symbol says, in the vendor's spellings.
    assert.equal(await valueOf('#add-mpn'), MPN);
    assert.equal(await valueOf('#add-manufacturer'), 'Samsung Electro-Mechanics');
    assert.equal(await fieldValue('Description'), 'CAP CER 0.1UF 50V X7R 0603');
    assert.equal(await fieldValue('Value'), '100n');
    assert.match(await textOf(browser, '.add-unused'), /Mouser Part Number/);
    assert.equal(await valueOf('#add-symbol'), 'g-cap:C_New');
    assert.equal(await valueOf('#add-footprint'), 'g-Capacitor_SMD:C_0603_New');
    assert.equal(await valueOf('#add-from'), 'SamacSys (Component Search Engine)');
    // The checks: the Catalog has a footprint with the same pads; the pins
    // and pads agree; the license needs review.
    assert.match(await textOf(browser, '#add-known-footprint'), /g-Capacitor_SMD:C_0603_1608Metric \(the same pads\)/);
    assert.equal(await textOf(browser, '#add-pins'), "The symbol's 2 pins match the footprint's numbered pads.");
    assert.match(await textOf(browser, '#add-license'), /^License: Unknown, needs review/);
    assert.equal(await textOf(browser, '#add-download'), `From ${MPN}.zip: 3 KiCad files used, 1 other file left out.`);
    await waitForDrawings(`${SYMBOL} .file-preview img`);
    await waitForDrawings(`${FOOTPRINT} .file-preview img`);
    await browser.execute(() => {
      document.querySelector('.part-fields').scrollTop = 0;
    });
    await assertAccessible(browser, '61-team-add-part');
    await screenshot(browser, '61-team-add-part');
    // An MPN typed by hand is checked too.
    await (await browser.$('#add-mpn')).setValue(KNOWN_MPN);
    await waitForText(browser, '#add-same-mpn p', `The MPN ${KNOWN_MPN} is already in the Catalog:`);
    await (await browser.$('#add-mpn')).setValue(MPN);
    await browser.waitUntil(async () => !(await browser.$('#add-same-mpn').isExisting()), {
      timeout: 10_000,
      timeoutMsg: 'the MPN check never cleared',
    });
  });

  it('saves the part, which the search finds with its drawings and history', async () => {
    await (await browser.$('#add-reason')).setValue('New 100n capacitor for the sensor board');
    await clickButton('.add-form', 'Save');
    await waitForText(browser, NOTICE, `Added ${IPN} as revision 1.`, 30_000);
    await waitForText(browser, '#part-title', MPN);
    await waitForText(browser, `${SYMBOL} .provenance`, /^team catalog, symbol revision 1 · /, 60_000);
    await waitForText(browser, `${FOOTPRINT} .provenance`, /^team catalog, footprint revision 1 · /, 60_000);
    await waitForText(browser, '#part-category', 'Category: Capacitors');
    // The new part is in its category's section, which opened by itself.
    await waitForSelectedInList(browser, IPN, 'Capacitors', 20_000);
    await searchFor(browser, MPN, '1 matching part');
    assert.match(await textOf(browser, 'ul[aria-label="Search results"]'), new RegExp(IPN));
    await clickButton(COLUMN, 'History (1)');
    await (await browser.$('.history li:nth-child(1) button')).click();
    const rows = await browser.execute(() =>
      [...document.querySelectorAll('.revision .changes tbody tr')].map((tr) =>
        [...tr.children].map((cell) => cell.textContent.trim()),
      ),
    );
    record({ event: 'added-history', rows });
    assert.deepEqual(rows[0], ['Downloaded from', `SamacSys (Component Search Engine): ${MPN}.zip`]);
    assert.ok(
      rows.some(([label, after]) => label === 'Footprint file' && after.startsWith('CL10B104/KiCad/C_0603_New.kicad_mod')),
      JSON.stringify(rows),
    );
  });

  it('shows a part that already has the MPN, and opens it', async () => {
    // Capacitors closed and a search hiding CAP-0000-0020, so opening it
    // from the notice must open its section once the search is cleared.
    await searchFor(browser, '', /^\d+ matching parts$/);
    await browser.execute(() => {
      const toggle = document.querySelector('.section-toggle[aria-controls="parts-in-cap"]');
      if (toggle.getAttribute('aria-expanded') === 'true') toggle.click();
    });
    await browser.waitUntil(
      async () => browser.execute(() => document.querySelector('.section-toggle[aria-controls="parts-in-cap"]').getAttribute('aria-expanded') === 'false'),
      { timeout: 5_000, timeoutMsg: 'Capacitors did not close' },
    );
    await searchFor(browser, MPN, '1 matching part');
    await (await browser.$('#add-part-open')).click();
    await waitForText(browser, '#add-heading', 'Add part');
    await (await browser.$('#add-files')).addValue(download(files, KNOWN_MPN));
    await waitForText(browser, '#add-same-mpn p', `The MPN ${KNOWN_MPN} is already in the Catalog:`);
    assert.match(await textOf(browser, '#add-same-mpn'), /CAP-0000-0020: CAP CER 2PF 50V C0G\/NP0 0603/);
    await clickButton('#add-same-mpn', 'Open CAP-0000-0020');
    await waitForText(browser, '#part-title', KNOWN_MPN);
    assert.equal(await browser.$('#add-heading').isExisting(), false);
    // Opened from the notice, the part is shown in its section, which
    // opens once the search (still the new part's MPN) no longer hides it.
    await searchFor(browser, '', /^\d+ matching parts$/);
    await waitForSelectedInList(browser, 'CAP-0000-0020', 'Capacitors');
  });
});
