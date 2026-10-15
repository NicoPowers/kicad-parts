// Choosing symbols and footprints from the Libraries browser
// (kicad-parts-bc6f): no CAD name is typed. Add part starts From a library
// with KiCad's generic capacitor; the footprint's Choose... opens the
// browser with KiCad's two filters on for that symbol, and the new part
// refers to KiCad's own symbol and footprint. Edit part then adds a second
// footprint choice from another installed library (fixture KiCad settings,
// never the developer's own): it is copied into the library chosen from a
// list, with its 3D model, and the history names where it came from.
// Remove takes the choice out again.

import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { after, before, describe, it } from 'node:test';
import {
  assertAccessible,
  chooseInLibraries,
  launchApp,
  record,
  repoRoot,
  screenshot,
  startTeamServer,
  tempDir,
  textOf,
  waitForLibraries,
  waitForLiveUpdates,
  waitForText,
} from '../harness.mjs';

const BADGE = '.team-settings > summary';
const NOTICE = '.team-notice';
const COLUMN = '.part-fields';
const KICAD_SYMBOL = 'Device:C';
const KICAD_FOOTPRINT = 'Capacitor_SMD:C_0603_1608Metric';
const COPIED = 'g-Capacitor_SMD:C_0603_Mine';

/** KiCad settings with one installed footprint library, whose footprint
 * links a 3D model through the person's path variable. */
function kicadSettings() {
  const mine = tempDir('my-footprints');
  const pretty = path.join(mine, 'Mine.pretty');
  fs.mkdirSync(pretty, { recursive: true });
  const footprint = fs
    .readFileSync(path.join(repoRoot, 'footprints/g-Capacitor_SMD.pretty/C_0603_1608Metric.kicad_mod'), 'utf8')
    .replace('(footprint "C_0603_1608Metric"', '(footprint "C_0603_Mine"')
    .replace('${KICAD9_3DMODEL_DIR}/Capacitor_SMD.3dshapes/C_0603_1608Metric.step', '${MY_LIBS}/models/C_0603_Mine.step');
  assert.ok(footprint.includes('${MY_LIBS}/models/C_0603_Mine.step'));
  fs.writeFileSync(path.join(pretty, 'C_0603_Mine.kicad_mod'), footprint);
  fs.mkdirSync(path.join(mine, 'models'), { recursive: true });
  fs.writeFileSync(path.join(mine, 'models', 'C_0603_Mine.step'), 'ISO-10303-21;\nEND-ISO-10303-21;\n');
  const config = tempDir('kicad-settings');
  fs.writeFileSync(path.join(config, 'kicad_common.json'), JSON.stringify({ environment: { vars: { MY_LIBS: mine } } }));
  fs.writeFileSync(path.join(config, 'sym-lib-table'), '(sym_lib_table (version 7)\n)\n');
  fs.writeFileSync(
    path.join(config, 'fp-lib-table'),
    '(fp_lib_table (version 7)\n  (lib (name "MyFootprints")(type "KiCad")(uri "${MY_LIBS}/Mine.pretty")(options "")(descr "My own footprints"))\n)\n',
  );
  return config;
}

describe('team catalog: choose symbols and footprints from the libraries', () => {
  let server;
  let browser;
  let editor;
  let ipn;

  before(async () => {
    server = await startTeamServer({ csv: ['g-res', 'g-cap'] });
    [editor] = server.accounts;
    ({ browser } = await launchApp({
      args: [`--team-server=${server.url}`, `--team-email=${editor.email}`, `--kicad-config-dir=${kicadSettings()}`],
    }));
  });

  after(async () => {
    await browser?.deleteSession();
    await server?.stop();
  });

  async function valueOf(selector) {
    return browser.execute((s) => document.querySelector(s)?.value ?? null, selector);
  }

  async function activeId() {
    return browser.execute(() => document.activeElement?.id ?? null);
  }

  async function click(selector) {
    await browser.execute((s) => document.querySelector(s).scrollIntoView({ block: 'center' }), selector);
    await (await browser.$(selector)).click();
  }

  async function clickButton(scope, text) {
    for (const button of await browser.$$(`${scope} button`)) {
      if ((await button.getText()).trim() === text && (await button.isDisplayed())) {
        await button.click();
        return;
      }
    }
    assert.fail(`no visible button "${text}" in ${scope}`);
  }

  async function historyRows() {
    return browser.execute(() =>
      [...document.querySelectorAll('.revision .changes tbody tr')].map((tr) =>
        [...tr.children].map((cell) => cell.textContent.trim()),
      ),
    );
  }

  it('signs in and opens Add part, which starts with two choices', async () => {
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
    assert.equal(await textOf(browser, 'label[for="add-files"]'), 'Upload a ZIP…');
    assert.equal(await textOf(browser, '#add-from-library'), 'From a library…');
  });

  it("adds a part from KiCad's generic capacitor, which refers to KiCad's own symbol and footprint", async () => {
    await (await browser.$('#add-from-library')).click();
    await waitForLibraries(browser);
    // From a library offers both symbols and footprints.
    assert.deepEqual(
      await browser.execute(() =>
        [...document.querySelectorAll('.libraries-dialog [role="tab"]')].filter((tab) => !tab.hidden).map((tab) => tab.textContent.trim()),
      ),
      ['Symbols', 'Footprints'],
    );
    await chooseInLibraries(browser, { group: 'kicad', library: 'Device', name: 'C' });
    await waitForText(browser, '#add-download', /^From the libraries: Device:C/, 30_000);
    assert.equal(await valueOf('#add-symbol'), KICAD_SYMBOL);
    assert.equal(await browser.execute(() => document.getElementById('add-symbol').readOnly), true);
    assert.match(await textOf(browser, '.add-form fieldset:last-of-type'), /KiCad's own: the part refers to it, as imported parts do\./);
    await waitForText(browser, '#add-guess', /^Best guess: Capacitors/);
    ipn = await valueOf('#add-ipn');
    assert.match(ipn, /^CAP-\d{4}-0001$/);
    // The footprint's Choose... opens footprints only, with KiCad's two
    // filters on for the chosen symbol.
    await click('#add-footprint-choose');
    await waitForLibraries(browser);
    await waitForText(browser, '.for-symbol-title', /For the symbol Device:C/);
    assert.deepEqual(
      await browser.execute(() => [
        document.getElementById('filter-match-filters').checked,
        document.getElementById('filter-match-pins').checked,
      ]),
      [true, true],
    );
    await chooseInLibraries(browser, { group: 'kicad', library: 'Capacitor_SMD', name: 'C_0603_1608Metric', enter: true });
    await browser.waitUntil(async () => (await valueOf('#add-footprint')) === KICAD_FOOTPRINT, {
      timeout: 30_000,
      timeoutMsg: `the footprint did not become ${KICAD_FOOTPRINT}: ${await valueOf('#add-footprint')}`,
    });
    await browser.waitUntil(async () => (await activeId()) === 'add-footprint-choose', {
      timeout: 10_000,
      timeoutMsg: 'focus did not come back to Choose…',
    });
    // Nothing is stored for KiCad's own items, so no license to review and
    // no library to choose.
    assert.equal(await browser.$('#add-license').isExisting(), false);
    assert.equal(await browser.$('#add-symbol-library').isExisting(), false);
    assert.equal(await valueOf('#add-category'), 'cap');
    await browser.execute(() => document.querySelector('.add-form fieldset:last-of-type').scrollIntoView({ block: 'start' }));
    await assertAccessible(browser, '70-team-add-part-from-kicad');
    await screenshot(browser, '70-team-add-part-from-kicad');
    const value = await browser.execute(() => {
      const label = [...document.querySelectorAll('.add-form .edit-row label')].find((l) => l.textContent.trim() === 'Value');
      return label?.htmlFor ?? null;
    });
    assert.ok(value, 'the category has a Value field');
    await (await browser.$(`#${value}`)).setValue('100n');
    await (await browser.$('#add-reason')).setValue('A generic capacitor for the prototype');
    await clickButton('.add-form', 'Save');
    await waitForText(browser, NOTICE, `Added ${ipn} as revision 1.`, 30_000);
    await waitForText(browser, '#team-tab-history', 'History (1)', 30_000);
    await clickButton(COLUMN, 'History (1)');
    await (await browser.$('.history li:nth-child(1) button')).click();
    const rows = await historyRows();
    record({ event: 'kicad-part-history', rows });
    assert.deepEqual(rows[0], ['Added from', "KiCad's libraries"]);
    await clickButton(COLUMN, 'Details');
    assert.match(await textOf(browser, COLUMN), /Device:C, KiCad library/);
  });

  it("adds a footprint choice copied from an installed library, into the library chosen, with its 3D model", async () => {
    await clickButton(COLUMN, 'Edit part');
    await waitForText(browser, '#edit-heading', 'Edit part');
    assert.equal(await valueOf('#edit-cad-1'), KICAD_FOOTPRINT);
    await click('#edit-footprint-add');
    await chooseInLibraries(browser, { group: 'installed', library: 'MyFootprints', name: 'C_0603_Mine' });
    await browser.waitUntil(async () => (await valueOf('#edit-cad-1-2')) === COPIED, {
      timeout: 30_000,
      timeoutMsg: `footprint 2 did not become ${COPIED}: ${await valueOf('#edit-cad-1-2')}`,
    });
    assert.equal(await textOf(browser, 'label[for="edit-cad-1-2"]'), 'Footprint 2');
    assert.match(
      await textOf(browser, '.edit-form fieldset:last-of-type'),
      /From the installed library MyFootprints, which teammates may not have: copied into the library you choose\./,
    );
    // The library is one of ours, from a list, with the suggested one chosen.
    assert.equal(await valueOf('#edit-cad-library-1-1'), 'g-Capacitor_SMD');
    assert.equal(await textOf(browser, '#edit-cad-models-1-1 option:checked'), 'Copy its 3D model (C_0603_Mine.step)');
    await browser.waitUntil(
      async () => browser.execute(() => [...document.querySelectorAll('.edit-form .file-preview img')].some((img) => img.complete && img.naturalWidth > 0)),
      { timeout: 60_000, interval: 100, timeoutMsg: 'the copied footprint was never drawn' },
    );
    await browser.execute(() => document.getElementById('edit-cad-1-2').scrollIntoView({ block: 'start' }));
    await assertAccessible(browser, '71-team-edit-installed-copy');
    await screenshot(browser, '71-team-edit-installed-copy');
    await (await browser.$('#edit-reason')).setValue('My own 0603 footprint as a second choice');
    await clickButton('.edit-form', 'Save');
    await waitForText(browser, NOTICE, 'Saved as revision 2.', 30_000);
    await clickButton(COLUMN, 'History (2)');
    await (await browser.$('.history li:nth-child(1) button')).click();
    const rows = await historyRows();
    record({ event: 'copy-history', rows });
    assert.ok(
      rows.some(([label, , after]) => label === 'Footprint' && after === `${KICAD_FOOTPRINT};${COPIED}`),
      JSON.stringify(rows),
    );
    assert.ok(
      rows.some(
        ([label, , after]) =>
          label === 'Footprint file' &&
          after.startsWith('C_0603_Mine.kicad_mod from the installed library MyFootprints (3D model path set to the Catalog\'s)'),
      ),
      JSON.stringify(rows),
    );
  });

  it('removes the second footprint choice again', async () => {
    await clickButton(COLUMN, 'Details');
    await clickButton(COLUMN, 'Edit part');
    await waitForText(browser, '#edit-heading', 'Edit part');
    assert.equal(await valueOf('#edit-cad-1-2'), COPIED);
    await click('#edit-cad-1-2-remove');
    await browser.waitUntil(async () => !(await browser.$('#edit-cad-1-2').isExisting()), {
      timeout: 5_000,
      timeoutMsg: 'footprint 2 was not removed',
    });
    assert.equal(await activeId(), 'edit-cad-1-choose', 'focus goes to the choice before');
    await (await browser.$('#edit-reason')).setValue('One footprint is enough');
    await clickButton('.edit-form', 'Save');
    await waitForText(browser, NOTICE, 'Saved as revision 3.', 30_000);
    // The part's footprint is KiCad's alone again.
    assert.match(await textOf(browser, COLUMN), /FootprintCapacitor_SMD:C_0603_1608Metric, KiCad libraryExcluded from BOM/);
  });
});
