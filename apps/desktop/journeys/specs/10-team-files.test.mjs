// Edit part with files (kicad-parts-3b8b): replace a resistor's footprint
// with a corrected .kicad_mod, see it drawn before saving, save it as a new
// footprint revision that only this part uses, and find the file in the
// history; then give the part a new symbol from a .kicad_sym that holds two
// symbols, picking the second.

import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { after, before, describe, it } from 'node:test';
import {
  FOOTPRINT,
  SYMBOL,
  assertAccessible,
  editAsSomeoneElse,
  images,
  launchApp,
  sampleLibrary,
  screenshot,
  searchFor,
  selectResult,
  startTeamServer,
  teamSignIn,
  tempDir,
  textOf,
  waitForLibraries,
  waitForLiveUpdates,
  waitForPreview,
  waitForText,
} from '../harness.mjs';

const IPN = 'RES-0000-0470';
const MPN = 'RC0603FR-0747RL';
const BADGE = '.team-settings > summary';
const COLUMN = '.part-fields';
const NOTICE = '.team-notice';
const FOOTPRINT_REFERENCE = 'g-Resistor_SMD:R_0603_1608Metric';

/** The resistor footprint with a circle added to its silkscreen. */
function correctedFootprint(folder) {
  const original = fs.readFileSync(
    path.join(sampleLibrary, 'footprints', 'g-Resistor_SMD.pretty', 'R_0603_1608Metric.kicad_mod'),
    'utf8',
  );
  const corrected = original.replace(
    '\t(fp_line',
    '\t(fp_circle (center 0 0) (end 1.6 0) (stroke (width 0.3) (type solid)) (fill no) (layer "F.SilkS"))\n\t(fp_line',
  );
  assert.notEqual(corrected, original);
  const file = path.join(folder, 'R_0603_1608Metric.kicad_mod');
  fs.writeFileSync(file, corrected);
  return file;
}

/** The resistor footprint renamed R_Journey, naming a Catalog 3D model the Catalog lacks. */
function footprintWithModel(folder) {
  const original = fs.readFileSync(
    path.join(sampleLibrary, 'footprints', 'g-Resistor_SMD.pretty', 'R_0603_1608Metric.kicad_mod'),
    'utf8',
  );
  const changed = original
    .replace('(footprint "R_0603_1608Metric"', '(footprint "R_Journey"')
    .replace(
      '${KICAD10_3DMODEL_DIR}/Resistor_SMD.3dshapes/R_0603_1608Metric.step',
      '${GITPLM_PARTS}/3d-models/g-Resistor_SMD.3dshapes/R_Journey.step',
    );
  const file = path.join(folder, 'R_Journey.kicad_mod');
  fs.writeFileSync(file, changed);
  const model = path.join(folder, 'R_Journey.step');
  fs.writeFileSync(model, 'ISO-10303-21;\nEND-ISO-10303-21;\n');
  return { file, model };
}

/** A symbol library with two symbols, a box and a circle. */
function twoSymbols(folder) {
  const symbol = (name, body) =>
    `\t(symbol "${name}"\n\t\t(pin_names (offset 0))\n\t\t(exclude_from_sim no)\n\t\t(in_bom yes)\n\t\t(on_board yes)\n` +
    `\t\t(property "Reference" "R" (at 0 6.35 0) (effects (font (size 1.27 1.27))))\n` +
    `\t\t(property "Value" "${name}" (at 0 -6.35 0) (effects (font (size 1.27 1.27))))\n` +
    `\t\t(symbol "${name}_0_1"\n\t\t\t${body}\n\t\t)\n` +
    `\t\t(symbol "${name}_1_1"\n` +
    `\t\t\t(pin passive line (at 0 3.81 270) (length 1.27) (name "~" (effects (font (size 1.27 1.27)))) (number "1" (effects (font (size 1.27 1.27)))))\n` +
    `\t\t\t(pin passive line (at 0 -3.81 90) (length 1.27) (name "~" (effects (font (size 1.27 1.27)))) (number "2" (effects (font (size 1.27 1.27)))))\n` +
    `\t\t)\n\t)\n`;
  const text =
    '(kicad_symbol_lib\n\t(version 20241209)\n\t(generator "kicad_symbol_editor")\n\t(generator_version "9.0")\n' +
    symbol('R_Box', '(rectangle (start -1.016 -2.54) (end 1.016 2.54) (stroke (width 0.254) (type default)) (fill (type none)))') +
    symbol('R_Round', '(circle (center 0 0) (radius 2.54) (stroke (width 0.254) (type default)) (fill (type none)))') +
    ')\n';
  const file = path.join(folder, 'two-symbols.kicad_sym');
  fs.writeFileSync(file, text);
  return file;
}

describe('team catalog: edit a part with files', () => {
  let server;
  let browser;
  let editor;
  let files;

  before(async () => {
    server = await startTeamServer();
    [editor] = server.accounts;
    files = tempDir('files');
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

  async function fieldValue(id) {
    return browser.execute((s) => document.querySelector(s)?.value ?? null, id);
  }

  async function waitForFilePreview(what) {
    await browser.waitUntil(
      async () => {
        const drawn = await images(browser, '.edit-file .file-preview img');
        return drawn.length > 0 && drawn.every((image) => image.loaded && image.svg && image.alt.includes(what));
      },
      { timeout: 60_000, interval: 100, timeoutMsg: `the chosen file was never drawn (${what})` },
    );
  }

  async function provenance(section) {
    return textOf(browser, `${section} .provenance`);
  }

  it('signs in and opens the resistor', async () => {
    await waitForText(browser, '#result-count', 'Sign in to a team to see its parts.');
    await browser.execute(() => document.querySelector('.team-settings').open = true);
    await (await browser.$('aria/Password')).setValue(editor.password);
    await (await browser.$('form[aria-label="Sign in to the team catalog"] button[type="submit"]')).click();
    await waitForText(browser, BADGE, `Team catalog · ${editor.name}`);
    await waitForLiveUpdates(browser);
    await browser.execute(() => document.querySelector('.team-settings').open = false);
    await searchFor(browser, MPN, '1 matching part');
    await selectResult(browser, MPN);
    await waitForText(browser, `${COLUMN} #fields-heading`, /^Part details/);
    await waitForPreview(browser, FOOTPRINT);
  });

  it('replaces the footprint with a file and draws it before saving', async () => {
    await clickButton(COLUMN, 'Edit part');
    // A file of the wrong kind is refused plainly.
    const wrong = path.join(files, 'notes.txt');
    fs.writeFileSync(wrong, 'not a footprint');
    await (await browser.$('#edit-cad-file-1')).addValue(wrong);
    await waitForText(browser, '.edit-file [role="alert"]', 'Choose a footprint file (.kicad_mod) for the footprint.');
    await (await browser.$('#edit-cad-file-1')).addValue(correctedFootprint(files));
    await waitForFilePreview('R_0603_1608Metric');
    assert.equal(await fieldValue('#edit-cad-1'), FOOTPRINT_REFERENCE, 'the reference comes from the file');
    assert.equal(await textOf(browser, '.edit-file .file-name'), "From R_0603_1608Metric.kicad_mod Don't use the file");
    assert.equal(await browser.execute(() => document.querySelectorAll('.edit-form [role="alert"]').length), 0);
    await browser.execute(() => document.querySelector('.edit-file .file-preview').scrollIntoView({ block: 'center' }));
    await assertAccessible(browser, '57-team-file-preview');
    await screenshot(browser, '57-team-file-preview');
  });

  it('saves a new footprint revision that only this part uses', async () => {
    await (await browser.$('#edit-reason')).setValue('Silkscreen marks the body');
    await clickButton('.edit-form', 'Save');
    await waitForText(
      browser,
      NOTICE,
      /^Saved as revision 2\. \d+ other parts still use an older revision of the footprint g-Resistor_SMD:R_0603_1608Metric\.$/,
    );
    await waitForText(browser, `${FOOTPRINT} .provenance`, /^team catalog, footprint revision 2 · /, 60_000);
    assert.match(await textOf(browser, COLUMN), /g-Resistor_SMD:R_0603_1608Metric, footprint revision 2/);
    await (await browser.$('#team-tab-history')).click();
    await waitForText(browser, '#revision-heading', 'What changed in revision 2');
    const row = await browser.execute(() => {
      const tr = [...document.querySelectorAll('.revision .changes tbody tr')].find(
        (r) => r.querySelector('th')?.textContent.trim() === 'Footprint file',
      );
      return tr ? [...tr.querySelectorAll('td')].map((td) => td.textContent.trim()) : null;
    });
    // Both files are named the same way: by their file name.
    assert.deepEqual(row, [
      'R_0603_1608Metric.kicad_mod (footprint revision 1 of g-Resistor_SMD:R_0603_1608Metric)',
      'R_0603_1608Metric.kicad_mod (footprint revision 2 of g-Resistor_SMD:R_0603_1608Metric)',
    ]);
    assert.match(await provenance(FOOTPRINT), /^team catalog, footprint revision 2/);
    await assertAccessible(browser, '58-team-file-history');
    await screenshot(browser, '58-team-file-history');
  });

  it('gives the part a new symbol, the second of a file that holds two', async () => {
    await (await browser.$('#team-tab-details')).click();
    await clickButton(COLUMN, 'Edit part');
    await (await browser.$('#edit-cad-file-0')).addValue(twoSymbols(files));
    await waitForFilePreview('R_Box');
    const choices = await browser.execute(() =>
      [...document.querySelectorAll('#edit-cad-symbol-0 option')].map((o) => [o.value, o.selected]),
    );
    assert.deepEqual(choices, [
      ['R_Box', true],
      ['R_Round', false],
    ]);
    assert.equal(await fieldValue('#edit-cad-0'), 'g-res:R_Box');
    await (await browser.$('#edit-cad-symbol-0')).selectByAttribute('value', 'R_Round');
    await waitForFilePreview('R_Round');
    assert.equal(await fieldValue('#edit-cad-0'), 'g-res:R_Round');
    await browser.execute(() => document.querySelector('#edit-cad-0').scrollIntoView({ block: 'start' }));
    await assertAccessible(browser, '59-team-file-symbol-choice');
    await screenshot(browser, '59-team-file-symbol-choice');
    await (await browser.$('#edit-reason')).setValue('Round body');
    await clickButton('.edit-form', 'Save');
    await waitForText(browser, NOTICE, 'Saved as revision 3.');
    await waitForText(browser, `${SYMBOL} .provenance`, /^team catalog, symbol revision 1 · /, 60_000);
    assert.equal(await textOf(browser, `${SYMBOL} .reference`), 'g-res:R_Round');
    assert.match(await textOf(browser, COLUMN), /g-res:R_Round, symbol revision 1/);
  });

  it('keeps the chosen files, and what to put back, when someone else saved first', async () => {
    await clickButton(COLUMN, 'Edit part');
    const before = await fieldValue('#edit-cad-1');
    const { file, model } = footprintWithModel(files);
    await (await browser.$('#edit-cad-file-1')).addValue(file);
    await waitForFilePreview('R_Journey');
    assert.equal(await fieldValue('#edit-cad-1'), 'g-Resistor_SMD:R_Journey');
    await waitForText(browser, '.model-file p', /names the 3D model 3d-models\/g-Resistor_SMD\.3dshapes\/R_Journey\.step/);
    await (await browser.$('#edit-model-file-1-0')).addValue(model);
    await waitForText(browser, '.model-file .file-name', 'From R_Journey.step');
    // Choose... on a reference only the unsaved file has opens its library
    // with nothing selected, never "not in its library any more".
    await browser.execute(() => document.getElementById('edit-cad-1-choose').scrollIntoView({ block: 'center' }));
    await (await browser.$('#edit-cad-1-choose')).click();
    await waitForLibraries(browser);
    await browser.waitUntil(
      async () =>
        browser.execute(() =>
          [...document.querySelectorAll('.library-button[aria-current="true"] .library-name')].some((n) => n.textContent === 'g-Resistor_SMD'),
        ),
      { timeout: 30_000, timeoutMsg: 'the browser did not open on g-Resistor_SMD' },
    );
    await waitForText(browser, '.libraries-detail h3', 'Nothing selected');
    assert.doesNotMatch(await textOf(browser, '.libraries-detail'), /not in its library any more/);
    await browser.keys(['Escape']);
    await waitForLibraries(browser, { open: false });
    // Someone else changes the symbol first, so saving shows a conflict.
    const token = await teamSignIn(server.url, server.accounts[1]);
    await editAsSomeoneElse(server.url, token, IPN, { Symbol: 'g-res:R_US' }, 'Back to the usual symbol');
    await (await browser.$('#edit-reason')).setValue('Own footprint');
    await clickButton('.edit-form', 'Save');
    await waitForText(browser, '#conflict-heading', 'Someone else changed this part first');
    await clickButton('.conflict', 'Edit again');
    // The form opens again with both files still chosen.
    await waitForFilePreview('R_Journey');
    assert.equal(await fieldValue('#edit-cad-1'), 'g-Resistor_SMD:R_Journey');
    await waitForText(browser, '.model-file .file-name', 'From R_Journey.step');
    // Not using the file puts back what the field said before it.
    await browser.execute(() => document.querySelector('.edit-file .file-remove').scrollIntoView({ block: 'center' }));
    await clickButton('.edit-file', "Don't use the file");
    assert.equal(await fieldValue('#edit-cad-1'), before);
    assert.equal(await browser.execute(() => document.querySelectorAll('.model-file').length), 0);
    await clickButton('.edit-form', 'Cancel');
  });
});
