// Critical journey: search the real catalog, select a part, and see the
// exact kicad-cli symbol (every unit) and footprint previews. Then the
// cache, a real export failure, and the other preview problem states.

import assert from 'node:assert/strict';
import { after, before, describe, it } from 'node:test';
import {
  FOOTPRINT,
  SYMBOL,
  assertAccessible,
  images,
  launchApp,
  record,
  screenshot,
  searchFor,
  selectResult,
  textOf,
  waitForPreview,
  waitForText,
} from '../harness.mjs';

describe('catalog search with exact KiCad previews', () => {
  let browser;

  before(async () => {
    ({ browser } = await launchApp());
  });

  after(async () => {
    await browser?.deleteSession();
  });

  it('opens on the whole catalog with KiCad 10 detected', async () => {
    await waitForText(browser, '#result-count', '178 matching parts');
    await waitForText(browser, '.kicad-badge-ready', /^KiCad 10\.\d+\.\d+$/);
    assert.equal(await textOf(browser, '.catalog-summary'), '178 parts in 21 categories');
    assert.equal(await textOf(browser, '.empty-state h2'), 'No part selected');
    const headings = await browser.execute(() =>
      [...document.querySelectorAll('h1, h2, h3')].map((h) => `${h.tagName}:${h.textContent.trim()}`),
    );
    assert.deepEqual(headings, ['H1:KiCad Parts', 'H2:No part selected']);
    assert.equal(await browser.execute(() => document.querySelectorAll('[role="alert"]').length), 0);
    await assertAccessible(browser, '01-catalog-loaded');
    await screenshot(browser, '01-catalog-loaded');
  });

  it('searches and previews every symbol unit and the footprint', async () => {
    await searchFor(browser, 'stm32h723', '1 matching part');
    await selectResult(browser, 'STM32H723ZGT6');
    await waitForText(browser, '#part-title', 'STM32H723ZGT6');
    await waitForPreview(browser, SYMBOL);
    await waitForPreview(browser, FOOTPRINT);

    const units = await images(browser, `${SYMBOL} figure img`);
    assert.ok(units.length > 1, `multi-unit symbol shows every unit (${units.length})`);
    units.forEach((unit, n) => {
      const letter = String.fromCharCode(65 + n);
      assert.ok(unit.loaded && unit.svg, `unit ${letter} rendered from a kicad-cli SVG`);
      assert.match(unit.alt, new RegExp(`^Symbol g-mcu:STM32H723ZG, unit ${letter} \\(${n + 1} of ${units.length}\\)$`));
    });
    const footprint = await images(browser, `${FOOTPRINT} figure img`);
    assert.equal(footprint.length, 1);
    assert.ok(footprint[0].loaded && footprint[0].svg);
    assert.equal(footprint[0].alt, 'Footprint Package_QFP:LQFP-144_20x20mm_P0.5mm');

    assert.match(await textOf(browser, `${SYMBOL} .provenance`), /^project library · kicad-cli 10\.\S+ · exported now$/);
    assert.match(await textOf(browser, `${FOOTPRINT} .provenance`), /^KiCad library · kicad-cli 10\.\S+ · exported now$/);
    record({ event: 'preview', part: 'STM32H723ZGT6', units: units.length });

    await assertAccessible(browser, '02-part-previews');
    await screenshot(browser, '02-part-previews');
    await browser.execute((s) => document.querySelector(s).scrollIntoView(), FOOTPRINT);
    await screenshot(browser, '03-part-footprint');
  });

  it('enlarges a symbol unit in a modal dialog', async () => {
    const first = await browser.$(`${SYMBOL} figure button`);
    assert.equal(await first.getAttribute('title'), 'Enlarge');
    await first.click();
    const dialog = `${SYMBOL} dialog`;
    await browser.waitUntil(async () => browser.execute((s) => document.querySelector(s)?.open === true, dialog), {
      timeout: 5_000,
      timeoutMsg: 'the enlarge dialog did not open',
    });
    assert.equal(await textOf(browser, `${dialog} h4`), 'Symbol g-mcu:STM32H723ZG, unit A (1 of 5)');
    await browser.waitUntil(async () => (await images(browser, `${dialog} img`))[0]?.loaded === true, {
      timeout: 5_000,
    });
    await assertAccessible(browser, '04-symbol-enlarged');
    await screenshot(browser, '04-symbol-enlarged');
    await browser.keys('Escape');
    await browser.waitUntil(async () => browser.execute((s) => document.querySelector(s).open === false, dialog), {
      timeout: 5_000,
      timeoutMsg: 'Escape did not close the dialog',
    });
  });

  it('serves a repeat view from the cache', async () => {
    await searchFor(browser, 'RC0603FR-0747RL', '1 matching part');
    await selectResult(browser, 'RC0603FR-0747RL');
    await waitForPreview(browser, SYMBOL);
    await waitForPreview(browser, FOOTPRINT);
    assert.equal((await images(browser, `${SYMBOL} figure img`)).length, 1);

    await searchFor(browser, 'stm32h723', '1 matching part');
    await selectResult(browser, 'STM32H723ZGT6');
    await waitForText(browser, `${SYMBOL} .provenance`, /from cache$/);
    await waitForText(browser, `${FOOTPRINT} .provenance`, /from cache$/);
  });

  it("shows kicad-cli's own message when an export fails", async () => {
    await searchFor(browser, 'TP4333', '1 matching part');
    await selectResult(browser, 'TP4333');
    await waitForPreview(browser, SYMBOL);
    const alert = `${SYMBOL} [role="alert"]`;
    assert.equal(await textOf(browser, `${alert} .problem-title`), 'kicad-cli could not export this symbol');
    assert.equal(await textOf(browser, `${alert} code`), 'g-ics:TP4333');
    assert.match(await textOf(browser, `${alert} .cli-message`), /There is no symbol selected to save\./);
    assert.match(await textOf(browser, `${alert} .command pre`), /^kicad-cli sym export svg /);
    await waitForPreview(browser, FOOTPRINT);
    await assertAccessible(browser, '05-export-failed');
    await screenshot(browser, '05-export-failed');
  });

  it('explains a footprint whose library entry is missing', async () => {
    await searchFor(browser, 'jetson nano', '1 matching part');
    await selectResult(browser, '2309413-1');
    await waitForPreview(browser, SYMBOL);
    await waitForPreview(browser, FOOTPRINT);
    assert.equal((await images(browser, `${SYMBOL} figure img`)).length, 3, 'Jetson Nano symbol has three units');
    const alert = `${FOOTPRINT} [role="alert"]`;
    assert.equal(await textOf(browser, `${alert} .problem-title`), 'The footprint library was not found');
    assert.match(await textOf(browser, alert), /Jetson_Nano_260_SODIMM/);
    await browser.execute((s) => document.querySelector(s).scrollIntoView(), FOOTPRINT);
    await assertAccessible(browser, '06-footprint-unresolved');
    await screenshot(browser, '06-footprint-unresolved');
  });

  it('marks a part without CAD assignments quietly', async () => {
    await searchFor(browser, 'PCB-0000-0001', '1 matching part');
    await selectResult(browser, 'PCB-0000-0001');
    await waitForPreview(browser, SYMBOL);
    await waitForPreview(browser, FOOTPRINT);
    assert.equal(await textOf(browser, `${SYMBOL} .problem.quiet .problem-title`), 'No symbol assigned');
    assert.equal(await browser.execute(() => document.querySelectorAll('[role="alert"]').length), 0);
    await assertAccessible(browser, '07-unassigned');
    await screenshot(browser, '07-unassigned');
  });

  it('says when nothing matches', async () => {
    await searchFor(browser, 'no-such-part-xyz', 'No parts match.');
    assert.equal(await browser.execute(() => document.querySelectorAll('.results li').length), 0);
    await assertAccessible(browser, '08-no-results');
    await screenshot(browser, '08-no-results');
  });
});
