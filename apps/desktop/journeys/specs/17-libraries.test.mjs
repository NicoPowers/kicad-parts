// The Libraries browser (kicad-parts-d152): opened from the header, it
// lists our libraries, KiCad's own and the libraries in the person's KiCad
// library tables (fixture tables here, never the developer's own), with a
// search, filters, KiCad's footprint filters for a symbol, a drawing of the
// chosen item and its details. It closes with Close, Escape or a click
// outside it.

import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { after, before, describe, it } from 'node:test';
import { assertAccessible, launchApp, screenshot, tempDir, textOf, waitForPreviewImages, waitForText } from '../harness.mjs';

const DIALOG = 'dialog.libraries-dialog';
const PREVIEW = '.libraries-detail .preview';

/** A small symbol with a body and pins, for the fixture libraries. */
function symbol(name, pins, extra = '') {
  const pinList = pins
    .map(
      (number, n) =>
        `(pin passive line (at -5.08 ${2.54 - n * 2.54} 0) (length 2.54) (name "P${number}" (effects (font (size 1.27 1.27)))) (number "${number}" (effects (font (size 1.27 1.27)))))`,
    )
    .join(' ');
  return `(symbol "${name}" ${extra} (property "Reference" "U" (at 0 5.08 0) (effects (font (size 1.27 1.27)))) (property "Value" "${name}" (at 0 -5.08 0) (effects (font (size 1.27 1.27)))) (property "Footprint" "" (at 0 0 0) (effects (font (size 1.27 1.27)) (hide yes))) (property "Description" "${name}, a fixture symbol" (at 0 0 0) (effects (font (size 1.27 1.27)) (hide yes))) (property "ki_fp_filters" "SOT?23*" (at 0 0 0) (effects (font (size 1.27 1.27)) (hide yes))) (symbol "${name}_0_1" (rectangle (start -2.54 3.81) (end 2.54 -3.81) (stroke (width 0.254) (type default)) (fill (type background)))) (symbol "${name}_1_1" ${pinList}))`;
}

/** KiCad settings with one symbol folder library and one footprint library. */
function kicadSettings() {
  const libs = tempDir('my-libraries');
  const symdir = path.join(libs, 'Mine.kicad_symdir');
  fs.mkdirSync(symdir, { recursive: true });
  const library = (body) => `(kicad_symbol_lib (version 20241209) (generator "journey") ${body})\n`;
  fs.writeFileSync(path.join(symdir, 'Base.kicad_sym'), library(symbol('Base', ['1', '2', '3'])));
  fs.writeFileSync(
    path.join(symdir, 'Child.kicad_sym'),
    library('(symbol "Child" (extends "Base") (property "Reference" "U" (at 0 5.08 0) (effects (font (size 1.27 1.27)))) (property "Value" "Child" (at 0 -5.08 0) (effects (font (size 1.27 1.27)))) (property "Description" "Child of Base, a fixture symbol" (at 0 0 0) (effects (font (size 1.27 1.27)) (hide yes))))'),
  );
  // A file KiCad cannot read either: skipped, the rest of the library kept.
  fs.writeFileSync(path.join(symdir, 'Broken.kicad_sym'), '(kicad_symbol_lib (symbol "Broken"');
  const config = tempDir('kicad-settings');
  fs.writeFileSync(
    path.join(config, 'kicad_common.json'),
    JSON.stringify({ environment: { vars: { MY_LIBS: libs } } }),
  );
  fs.writeFileSync(
    path.join(config, 'sym-lib-table'),
    '(sym_lib_table (version 7)\n' +
      '  (lib (name "MySymbols")(type "KiCad")(uri "${MY_LIBS}/Mine.kicad_symdir")(options "")(descr "My own symbols"))\n' +
      '  (lib (name "Device")(type "KiCad")(uri "${KICAD10_SYMBOL_DIR}/Device.kicad_sym")(options "")(descr ""))\n)\n',
  );
  fs.writeFileSync(path.join(config, 'fp-lib-table'), '(fp_lib_table (version 7)\n)\n');
  return config;
}

async function openBrowser(browser) {
  await (await browser.$('#libraries-open')).click();
  await browser.waitUntil(async () => browser.execute((s) => document.querySelector(s)?.open === true, DIALOG), {
    timeout: 10_000,
    timeoutMsg: 'the Libraries browser did not open',
  });
}

async function waitUntilRead(browser) {
  // Reading every KiCad library from nothing takes a few seconds, or about
  // a minute when the disk cache is cold.
  await browser.waitUntil(
    async () =>
      browser.execute(() => {
        const view = document.querySelector('.library-groups');
        return (
          document.querySelector('.libraries-progress')?.dataset.state === 'ready' &&
          view !== null &&
          !view.textContent.includes('…')
        );
      }),
    { timeout: 240_000, interval: 250, timeoutMsg: 'the libraries were never all read' },
  );
}

/** Waits until the list shows the answer to what was last asked. */
async function settled(browser) {
  await browser.waitUntil(
    async () => browser.execute(() => document.getElementById('library-items')?.getAttribute('aria-busy') === 'false'),
    { timeout: 20_000, interval: 25, timeoutMsg: 'the list never showed its answer' },
  );
}

async function chooseLibrary(browser, nickname) {
  const clicked = await browser.execute((name) => {
    const button = [...document.querySelectorAll('.library-button')].find(
      (b) => b.querySelector('.library-name')?.textContent === name,
    );
    button?.click();
    return Boolean(button);
  }, nickname);
  assert.ok(clicked, `no library named ${nickname}`);
  await settled(browser);
}

async function rows(browser) {
  return browser.execute(() =>
    [...document.querySelectorAll('#library-items [role="option"]')].map((row) => ({
      name: row.querySelector('.library-item-name').textContent,
      library: row.querySelector('.library-item-library')?.textContent ?? null,
      selected: row.getAttribute('aria-selected'),
      marks: [...row.querySelectorAll('.mark')].map((mark) => mark.textContent),
    })),
  );
}

async function typeSearch(browser, text) {
  const input = await browser.$('#library-search');
  if (text === '') {
    await input.click();
    await browser.keys(['Control', 'a']);
    await browser.keys('Backspace');
  } else {
    await input.setValue(text);
  }
  await settled(browser);
}

async function selectRow(browser, name) {
  const clicked = await browser.execute((wanted) => {
    const row = [...document.querySelectorAll('#library-items [role="option"]')].find(
      (r) => r.querySelector('.library-item-name').textContent === wanted,
    );
    row?.click();
    return Boolean(row);
  }, name);
  assert.ok(clicked, `no row named ${name}`);
  await waitForText(browser, '#library-detail-title', new RegExp(`:${name.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}$`));
}

async function fact(browser, name) {
  return browser.execute((wanted) => {
    const fact = [...document.querySelectorAll('.library-facts .fact')].find((f) => f.querySelector('dt').textContent === wanted);
    return fact?.querySelector('dd').textContent ?? null;
  }, name);
}

describe('Libraries browser', () => {
  let browser;

  before(async () => {
    ({ browser } = await launchApp({ args: [`--kicad-config-dir=${kicadSettings()}`] }));
  });

  after(async () => {
    await browser?.deleteSession();
  });

  it('opens from the header on every library, grouped', async () => {
    await waitForText(browser, '#result-count', '178 matching parts');
    await openBrowser(browser);
    // The cursor starts in the search box.
    await browser.waitUntil(async () => browser.execute(() => document.activeElement?.id === 'library-search'), {
      timeout: 5_000,
      timeoutMsg: 'the search box did not get focus',
    });
    await waitUntilRead(browser);
    const groups = await browser.execute(() =>
      [...document.querySelectorAll('.library-group')].map((h) => h.textContent),
    );
    assert.equal(groups.length, 3, groups.join(', '));
    assert.match(groups[0], /^Ours \(\d+\)$/);
    assert.match(groups[1], /^KiCad \(\d{3}\)$/);
    // The fixture tables add one library; their Device row is KiCad's own.
    assert.equal(groups[2], 'Installed (1)');
    await waitForText(browser, '#library-count', /^Showing 200 of [\d,]+ symbols\. Search to narrow the list\.$/);
    assert.equal(await textOf(browser, '.libraries-detail h3'), 'Nothing selected');
    await assertAccessible(browser, '66-libraries-open');
    await screenshot(browser, '66-libraries-open');
  });

  it('searches one library, moves with the arrow keys and draws the symbol', async () => {
    await chooseLibrary(browser, 'Device');
    await typeSearch(browser, 'r');
    await waitForText(browser, '#library-count', /^Showing 200 of [\d,]+ symbols/);
    const found = await rows(browser);
    assert.equal(found[0].name, 'R', JSON.stringify(found.slice(0, 3)));
    assert.equal(found[0].library, null, 'one library: rows do not repeat its name');
    // Arrow keys move through the list box.
    await browser.execute(() => document.getElementById('library-items').focus());
    await browser.keys('ArrowDown');
    await waitForText(browser, '#library-detail-title', 'Device:R');
    await browser.keys('ArrowDown');
    await waitForText(browser, '#library-detail-title', `Device:${found[1].name}`);
    await browser.keys('ArrowUp');
    await waitForText(browser, '#library-detail-title', 'Device:R');
    const active = await browser.execute(() => document.getElementById('library-items').getAttribute('aria-activedescendant'));
    assert.equal(active, 'library-item-0');
    await waitForPreviewImages(browser, PREVIEW);
    assert.match(await textOf(browser, `${PREVIEW} .provenance`), /^KiCad library · kicad-cli 10\.\S+ · exported now$/);
    assert.equal(await fact(browser, 'Pins'), '2');
    assert.equal(await fact(browser, 'Footprint filters'), 'R_*');
    assert.equal(await fact(browser, 'Library'), 'Device (KiCad)');
    await assertAccessible(browser, '67-libraries-symbol');
    await screenshot(browser, '67-libraries-symbol', { blankOut: ['.library-facts .fact:last-child dd'] });
  });

  it('filters symbols by pins, reference and power', async () => {
    await typeSearch(browser, '');
    await waitForText(browser, '#library-count', /^Showing 200 of [\d,]+ symbols\. Search to narrow the list\.$/);
    await (await browser.$('#filter-pins-min')).setValue('3');
    await (await browser.$('#filter-pins-max')).setValue('3');
    await (await browser.$('#filter-reference')).setValue('Q');
    await settled(browser);
    await waitForText(browser, '#library-count', /^\d+ symbols$/);
    const transistors = await rows(browser);
    assert.ok(transistors.length > 3 && transistors.every((row) => row.name.startsWith('Q')), JSON.stringify(transistors.slice(0, 5)));
    // Clear them again.
    for (const id of ['#filter-pins-min', '#filter-pins-max', '#filter-reference']) {
      await (await browser.$(id)).click();
      await browser.keys(['Control', 'a']);
      await browser.keys('Backspace');
    }
    await (await browser.$('#filter-hide-power')).click();
    await settled(browser);
    await waitForText(browser, '#library-count', /^Showing 200 of [\d,]+ symbols/);
    assert.ok((await rows(browser)).every((row) => row.name !== 'GND'));
    await (await browser.$('#filter-hide-power')).click();
    await settled(browser);
  });

  it("lists the footprints that match a symbol, with KiCad's two filters", async () => {
    await chooseLibrary(browser, 'Device');
    await typeSearch(browser, 'r');
    await selectRow(browser, 'R');
    await (await browser.$('#library-matching-footprints')).click();
    await settled(browser);
    await waitForText(browser, '.for-symbol-title', /^For the symbol Device:R/);
    await waitForText(browser, 'label[for="filter-match-filters"]', 'Match its footprint filters (R_*)');
    await waitForText(browser, 'label[for="filter-match-pins"]', 'Match its pin count (2)');
    await waitForText(browser, '#library-count', /^[\d,]+ footprints$/);
    const matching = await rows(browser);
    assert.ok(matching.length > 20, `${matching.length}`);
    assert.ok(matching.every((row) => row.name.startsWith('R_')), JSON.stringify(matching.slice(0, 5)));
    assert.ok(matching.some((row) => row.library === 'Resistor_SMD' && row.name === 'R_0603_1608Metric'));
    // Our library's R_0603 is used by Catalog parts.
    const ours = matching.find((row) => row.library === 'g-Resistor_SMD' && row.name === 'R_0603_1608Metric');
    assert.deepEqual(ours?.marks, ['In the Catalog']);
    await (await browser.$('#filter-mount')).selectByAttribute('value', 'tht');
    await settled(browser);
    await waitForText(browser, '#library-count', /^[\d,]+ footprints$/);
    assert.ok((await rows(browser)).every((row) => row.library !== 'Resistor_SMD'));
    await (await browser.$('#filter-mount')).selectByAttribute('value', 'any');
    await settled(browser);
    await typeSearch(browser, 'R_0603_1608Metric');
    await waitForText(browser, '#library-count', /^\d footprints?$/);
    await selectRow(browser, 'R_0603_1608Metric');
    await waitForPreviewImages(browser, PREVIEW);
    assert.equal(await fact(browser, 'Pads'), '2');
    assert.equal(await fact(browser, 'Mounting'), 'SMD');
    await assertAccessible(browser, '68-libraries-footprints');
    await screenshot(browser, '68-libraries-footprints', { blankOut: ['.library-facts .fact:last-child dd'] });
    // Without the symbol, every footprint again.
    await (await browser.$('#for-symbol-clear')).click();
    await settled(browser);
    await browser.waitUntil(async () => (await browser.$$('.for-symbol')).length === 0, { timeout: 5_000 });
  });

  it("draws a symbol from the person's own folder library", async () => {
    await (await browser.$('#library-tab-symbols')).click();
    await chooseLibrary(browser, 'MySymbols');
    await typeSearch(browser, '');
    await waitForText(browser, '#library-count', '2 symbols');
    // The file that could not be read is named, visibly.
    assert.match(await textOf(browser, '.library-unreadable'), /^Some of its files were skipped\. Broken\.kicad_sym could not be read/);
    const said = await browser.execute(() =>
      [...document.querySelectorAll('.library-entry-note')].map((note) => note.textContent),
    );
    assert.ok(said.some((note) => note.startsWith('Broken.kicad_sym could not be read')), JSON.stringify(said));
    await selectRow(browser, 'Child');
    await waitForPreviewImages(browser, PREVIEW);
    assert.match(await textOf(browser, `${PREVIEW} .provenance`), /^installed library · /);
    // A derived symbol has its parent's pins.
    assert.equal(await fact(browser, 'Pins'), '3');
    assert.equal(await fact(browser, 'Library'), 'MySymbols (Installed)');
  });

  it('shows the parts that use one of our symbols', async () => {
    await chooseLibrary(browser, 'g-res');
    await typeSearch(browser, 'R_US');
    await waitForText(browser, '#library-count', /^\d+ symbols?$/);
    await selectRow(browser, 'R_US');
    assert.match(await textOf(browser, '.library-used-title'), /^Used by \d+ Catalog parts$/);
    await (await browser.$('#library-show-parts')).click();
    await browser.waitUntil(async () => browser.execute((s) => !document.querySelector(s), DIALOG), {
      timeout: 5_000,
      timeoutMsg: 'the browser did not close',
    });
    // Only the parts using it exactly, shown as a chip, not as search text.
    assert.equal(await browser.execute(() => document.getElementById('part-search').value), '');
    await waitForText(browser, '#uses-chip span', 'Parts using g-res:R_US');
    // The answer limited to the reference, not the one before it.
    await waitForText(browser, '#result-count', /^(?!178 )\d+ matching parts$/);
    const using = Number((await textOf(browser, '#result-count')).replace(/\D/g, ''));
    assert.ok(using > 10 && using < 178, `${using}`);
    await browser.waitUntil(async () => browser.execute(() => document.activeElement?.id === 'part-search'), {
      timeout: 5_000,
      timeoutMsg: 'focus did not move to the part search',
    });
    await assertAccessible(browser, '69-parts-using-symbol');
    await screenshot(browser, '69-parts-using-symbol');
    await (await browser.$('#uses-chip-remove')).click();
    await waitForText(browser, '#result-count', '178 matching parts');
    assert.equal(await browser.execute(() => document.getElementById('uses-chip')), null);
  });

  it('closes with Escape and with a click outside, giving focus back', async () => {
    await openBrowser(browser);
    await browser.keys('Escape');
    await browser.waitUntil(async () => browser.execute((s) => !document.querySelector(s), DIALOG), {
      timeout: 5_000,
      timeoutMsg: 'Escape did not close the browser',
    });
    await browser.waitUntil(async () => browser.execute(() => document.activeElement?.id === 'libraries-open'), {
      timeout: 5_000,
      timeoutMsg: 'focus did not go back to the Libraries button',
    });
    await openBrowser(browser);
    // A press and release on the dark backdrop, in the margin round it.
    await browser.action('pointer').move({ x: 6, y: 400 }).down().up().perform();
    await browser.waitUntil(async () => browser.execute((s) => !document.querySelector(s), DIALOG), {
      timeout: 5_000,
      timeoutMsg: 'a click outside did not close the browser',
    });
    await openBrowser(browser);
    // A click inside does not.
    await browser.action('pointer').move({ x: 640, y: 30 }).down().up().perform();
    assert.ok(await browser.execute((s) => document.querySelector(s)?.open === true, DIALOG));
    await (await browser.$('#libraries-close')).click();
    await browser.waitUntil(async () => browser.execute((s) => !document.querySelector(s), DIALOG), { timeout: 5_000 });
  });
});
