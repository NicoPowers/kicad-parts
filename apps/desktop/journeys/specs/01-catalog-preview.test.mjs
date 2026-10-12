// Critical journey: search the real catalog, select a part, and see the
// exact kicad-cli symbol (every unit) and footprint previews. Then the
// cache, a real export failure, and the other preview problem states.

import assert from 'node:assert/strict';
import path from 'node:path';
import { after, before, describe, it } from 'node:test';
import {
  FOOTPRINT,
  SYMBOL,
  VISUAL_LIMITS,
  assertAccessible,
  capture,
  comparePngs,
  images,
  outDir,
  screenshotsDir,
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
    assert.equal(await textOf(browser, '.catalog-summary'), '178 parts from 21 CSV files');
    assert.equal(await textOf(browser, '.empty-state h2'), 'No part selected');
    const headings = await browser.execute(() =>
      [...document.querySelectorAll('h1, h2, h3')].map((h) => `${h.tagName}:${h.textContent.trim()}`),
    );
    assert.deepEqual(headings, ['H1:KiCad Parts', 'H2:No part selected']);
    assert.equal(await browser.execute(() => document.querySelectorAll('[role="alert"]').length), 0);
    await assertAccessible(browser, '01-catalog-loaded');
    await screenshot(browser, '01-catalog-loaded');
  });

  it('fails a screenshot that differs by one character or one colour', async () => {
    // Compared with this run's own 01 screenshot, so it also works while
    // the approved screenshots are being updated.
    const reference = path.join(screenshotsDir, '01-catalog-loaded.png');
    const checksDir = path.join(outDir, 'limit-checks');
    const unchanged = path.join(checksDir, '00-unchanged.png');
    await capture(browser, unchanged);
    assert.equal(comparePngs(reference, unchanged).differentPixels, 0, 'the same page gives the same pixels');

    const text = (selector, from, to) => ({ selector, from, to });
    const token = (name, value) => ({ name, value });
    const changes = [
      ['178 to 176 parts', text('.catalog-summary', '178 parts', '176 parts')],
      ['1.8V to 7.8V', text('.result-description', '1.8V', '7.8V')],
      ['full stop to comma', text('.empty-state p', '.', ',')],
      ['rail to rall', text('.result-description', 'rail', 'rall')],
      ['header colour', token('--brand', '#2a4a75')],
      ['canvas colour', token('--canvas', '#e9edf2')],
      ['corner radius', token('--radius', '6px')],
    ];
    const counts = {};
    for (const [n, [label, change]] of changes.entries()) {
      // Make one change, take a screenshot, then undo the change.
      const made = await browser.execute((c) => {
        if (c.name) {
          const root = document.documentElement.style;
          const before = root.getPropertyValue(c.name);
          root.setProperty(c.name, c.value);
          window.journeyUndo = () => (before ? root.setProperty(c.name, before) : root.removeProperty(c.name));
          return true;
        }
        const walker = document.createTreeWalker(document.querySelector(c.selector), NodeFilter.SHOW_TEXT);
        for (let node = walker.nextNode(); node; node = walker.nextNode()) {
          if (node.data.includes(c.from)) {
            const before = node.data;
            node.data = before.replace(c.from, c.to);
            window.journeyUndo = () => (node.data = before);
            return true;
          }
        }
        return false;
      }, change);
      assert.ok(made, `${label}: the page has the text to change`);
      const file = path.join(checksDir, `${String(n + 1).padStart(2, '0')}-${label.replaceAll(' ', '-')}.png`);
      await capture(browser, file);
      await browser.execute(() => {
        window.journeyUndo();
        delete window.journeyUndo;
      });
      counts[label] = comparePngs(reference, file, file.replace(/\.png$/, '-differences.png')).differentPixels;
    }
    record({ event: 'visual-limit-checks', limit: VISUAL_LIMITS.MAX_DIFFERENT_PIXELS, counts });
    for (const [label, count] of Object.entries(counts)) {
      assert.ok(
        count > VISUAL_LIMITS.MAX_DIFFERENT_PIXELS,
        `${label} changed only ${count} pixels, which the limit of ${VISUAL_LIMITS.MAX_DIFFERENT_PIXELS} would let pass`,
      );
    }
    await capture(browser, unchanged);
    assert.equal(comparePngs(reference, unchanged).differentPixels, 0, 'every change was undone');
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

  it('explains a footprint whose library is missing', async () => {
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

  it('shows a single-unit part beside every catalog field without scrolling', async () => {
    await searchFor(browser, 'KF33BD', '1 matching part');
    await selectResult(browser, 'KF33BD-TR');
    await waitForText(browser, '#part-title', 'KF33BD-TR');
    await waitForPreview(browser, SYMBOL);
    await waitForPreview(browser, FOOTPRINT);
    const layout = await browser.execute(() => {
      const fits = (el) => el.scrollHeight <= el.clientHeight + 1;
      const previews = document.querySelector('.part-previews');
      const fields = document.querySelector('.part-fields');
      return {
        windowFits: fits(document.scrollingElement),
        panelFits: fits(document.querySelector('.part-panel')),
        previewsFit: fits(previews),
        fieldsFit: fits(fields),
        fieldsOnTheRight: fields.getBoundingClientRect().left >= previews.getBoundingClientRect().right,
        collapsibles: document.querySelectorAll('.part-view details').length,
        rows: fields.querySelectorAll('tbody tr').length,
        heading: fields.querySelector('h3').textContent.trim(),
        fieldsHeight: [fields.scrollHeight, fields.clientHeight],
        previewsHeight: [previews.scrollHeight, previews.clientHeight],
      };
    });
    record({ event: 'layout', part: 'KF33BD-TR', ...layout });
    await screenshot(browser, '09-part-fields');
    assert.equal(layout.windowFits, true, 'the window itself never scrolls');
    assert.equal(layout.panelFits, true, 'the part panel needs no scrolling');
    assert.equal(layout.previewsFit, true, 'symbol and footprint fit stacked on the left');
    assert.equal(layout.fieldsFit, true, 'every catalog field fits on the right');
    assert.equal(layout.fieldsOnTheRight, true);
    assert.equal(layout.collapsibles, 0, 'catalog fields are always visible');
    assert.equal(layout.heading, `Catalog fields (${layout.rows})`);
    await assertAccessible(browser, '09-part-fields');
  });

  it('says when nothing matches', async () => {
    await searchFor(browser, 'no-such-part-xyz', 'No parts match.');
    assert.equal(await browser.execute(() => document.querySelectorAll('.results li').length), 0);
    await assertAccessible(browser, '08-no-results');
    await screenshot(browser, '08-no-results');
  });
});
