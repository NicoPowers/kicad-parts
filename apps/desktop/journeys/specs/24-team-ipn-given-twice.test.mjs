// One IPN given to two parts (kicad-parts-3b72). While a team moves from
// the CSV catalog to the app, someone adds a CSV row while Add part is
// open. Add part reads the CSV catalog folder again just before saving, so
// it never gives that row's IPN to another part: it saves nothing and
// offers the next free IPN. Then a CSV row gets the IPN of the part added
// in the app, with another MPN. The import leaves the part as it is, and
// the part says that the row is another part and needs another IPN.
//
// The app reads a copy of the CSV catalog, which the journey changes; the
// repository is only read.

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
  repoRoot,
  screenshot,
  startTeamServer,
  tempDir,
  textOf,
  waitForLiveUpdates,
  waitForText,
  zipOf,
} from '../harness.mjs';

const MPN = 'AR0603-10K';
const OTHER_MPN = 'ERJ-3EKF1002V';
/** The next free resistor IPN: the CSV catalog's resistors are RES-0000-xxxx. */
const OFFERED = 'RES-0001-0001';
const NEXT = 'RES-0002-0001';
const BADGE = '.team-settings > summary';
const NOTICE = '.team-notice';
const SAVE_PROBLEM = '.add-form .team-problem';
const OTHER_PART = '.csv-notice.other-part';

/** A copy of the CSV catalog for the app to read. */
function catalogCopy() {
  const copy = tempDir('catalog');
  for (const folder of ['database', 'symbols', 'footprints', '3d-models']) {
    fs.cpSync(path.join(repoRoot, folder), path.join(copy, folder), { recursive: true });
  }
  return copy;
}

/** Adds a 10k resistor row with `ipn` and `mpn` to the copy's g-res.csv. */
function addRow(copy, ipn, mpn) {
  const file = path.join(copy, 'database', 'g-res.csv');
  const text = fs.readFileSync(file, 'utf8');
  assert.ok(text.endsWith('\n'));
  fs.writeFileSync(
    file,
    `${text}"${ipn}","${mpn}","Panasonic","RES 10K OHM 1% 1/10W 0603","10k","g-res:R_US","g-Resistor_SMD:R_0603_1608Metric","1%","","","",""\n`,
  );
}

/** The repository's resistor symbol and footprint, renamed, as a vendor ZIP. */
function download(folder) {
  const read = (file) => fs.readFileSync(path.join(repoRoot, file), 'utf8');
  const symbol = read('symbols/g-res.kicad_sym')
    .replaceAll('(symbol "R_US_', '(symbol "R_New_')
    .replace('(symbol "R_US"', '(symbol "R_New"')
    .replace('"Value" ".Value"', '"Value" "10k"')
    .replace('"Description" "Resistor, US symbol"', '"Description" "RES 10K OHM 1% 1/10W 0603"')
    .replace(
      '\t\t(property "Reference"',
      `\t\t(property "Manufacturer_Name" "Acme Resistors" (at 0 0 0) (effects (hide yes)))\n` +
        `\t\t(property "Manufacturer_Part_Number" "${MPN}" (at 0 0 0) (effects (hide yes)))\n` +
        '\t\t(property "Reference"',
    );
  const footprint = read('footprints/g-Resistor_SMD.pretty/R_0603_1608Metric.kicad_mod')
    .replace('(footprint "R_0603_1608Metric"', '(footprint "R_0603_New"')
    .replace('${KICAD9_3DMODEL_DIR}/Resistor_SMD.3dshapes/R_0603_1608Metric.step', '${KIPRJMOD}/3D/R_0603_New.stp');
  const file = path.join(folder, `${MPN}.zip`);
  fs.writeFileSync(
    file,
    zipOf([
      ['AR0603/KiCad/R_New.kicad_sym', Buffer.from(symbol, 'utf8')],
      ['AR0603/KiCad/R_0603_New.kicad_mod', Buffer.from(footprint, 'utf8')],
      ['AR0603/3D/R_0603_New.stp', Buffer.from('ISO-10303-21;\nEND-ISO-10303-21;\n')],
    ]),
  );
  return file;
}

describe('team catalog: one IPN given to two parts', () => {
  let server;
  let browser;
  let editor;
  let copy;

  before(async () => {
    server = await startTeamServer({ csv: ['g-res'] });
    [editor] = server.accounts;
    copy = catalogCopy();
    ({ browser } = await launchApp({
      args: [`--team-server=${server.url}`, `--team-email=${editor.email}`, `--catalog-root=${copy}`],
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

  it('saves nothing when a CSV row took the offered IPN, and offers the next free one', async () => {
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
    await (await browser.$('#add-files')).addValue(download(tempDir('download')));
    await waitForText(browser, '#add-guess', /^Best guess: Resistors, because /);
    assert.equal(await valueOf('#add-ipn'), OFFERED);
    await (await browser.$('#add-reason')).setValue('New 10k resistor for the sensor board');

    // Someone working in the CSV catalog gives the same IPN to a row, and
    // the app is not told.
    addRow(copy, OFFERED, OTHER_MPN);
    await clickButton('.add-form', 'Save');
    await waitForText(
      browser,
      SAVE_PROBLEM,
      `Someone gave the IPN ${OFFERED} to a row of database/g-res.csv, so nothing was saved. The next free IPN is ${NEXT}.`,
      30_000,
    );
    assert.equal(await valueOf('#add-ipn'), NEXT, 'the next free IPN is filled in');
    assert.equal(await browser.$('#add-heading').isExisting(), true, 'the form stays open');
    await waitForDrawings(`${SYMBOL} .file-preview img`);
    await waitForDrawings(`${FOOTPRINT} .file-preview img`);
    await browser.execute(() => document.querySelector('.add-form .team-problem').scrollIntoView({ block: 'center' }));
    await assertAccessible(browser, '97-team-add-part-ipn-in-csv');
    await screenshot(browser, '97-team-add-part-ipn-in-csv');

    await clickButton('.add-form', 'Save');
    await waitForText(browser, NOTICE, `Added ${NEXT} as revision 1.`, 30_000);
    await waitForText(browser, '#part-title', MPN);
  });

  it('shows on the part that a CSV row has its IPN for another part, and changes nothing', async () => {
    assert.equal((await browser.$$(OTHER_PART)).length, 0, 'no notice before any CSV row has the IPN');
    const before = await textOf(browser, '.part-fields table');
    // Someone else gives the part's IPN to another resistor in the CSV
    // catalog, and the CSV catalog is imported.
    addRow(copy, NEXT, OTHER_MPN);
    const outcome = await server.reimport(copy);
    record({ event: 'reimport', outcome });
    assert.ok(['applied', 'nothing_changed'].includes(outcome), outcome);
    await waitForText(browser, '#other-part-heading', 'A CSV row has this IPN for another part', 30_000);
    assert.equal(
      await textOf(browser, OTHER_PART),
      'A CSV row has this IPN for another part' +
        `Row 31 of g-res.csv has this part's IPN, but its MPN, ${OTHER_MPN}, is not one of this part's.` +
        'The import did not change this part. Ask whoever looks after the CSV catalog to give that row another IPN.',
    );
    assert.equal(
      await browser.execute((s) => document.querySelector(s).getAttribute('role'), OTHER_PART),
      'status',
      'screen readers hear the notice when it appears',
    );
    // Nothing of the row is offered for review, and the part is unchanged.
    assert.equal(await browser.$('#csv-review-open').isExisting(), false);
    assert.equal(await textOf(browser, '.part-fields table'), before);
    await browser.execute(() => {
      document.querySelector('.part-fields').scrollTop = 0;
    });
    await assertAccessible(browser, '98-team-csv-row-for-another-part');
    await screenshot(browser, '98-team-csv-row-for-another-part');
  });
});
