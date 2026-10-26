// Critical journey: search a team's parts, select one, and see the exact
// kicad-cli symbol (every unit) and footprint previews. Then the cache, a
// real export failure, and the other preview problem states. The team
// server has the sample library's parts (fixtures/sample-library), which
// the app also reads as our own libraries.

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
  signInToTeam,
  startTeamServer,
  textOf,
  waitForPreview,
  waitForText,
} from '../harness.mjs';

describe('catalog search with exact KiCad previews', () => {
  let server;
  let browser;

  before(async () => {
    server = await startTeamServer();
    ({ browser } = await launchApp({ args: [`--team-server=${server.url}`] }));
    await signInToTeam(browser, server);
  });

  after(async () => {
    await browser?.deleteSession();
    await server?.stop();
  });

  it("opens on the team's parts with KiCad 10 detected", async () => {
    await waitForText(browser, '#result-count', '16 matching parts');
    await waitForText(browser, '.kicad-settings > .kicad-badge-ready', /^KiCad 10\.\d+\.\d+$/);
    // The sample library has no CSV files, so the header says nothing of
    // them.
    assert.equal(await browser.execute(() => document.querySelector('.catalog-summary')), null);
    assert.equal(await textOf(browser, '.empty-state h2'), 'No part selected');
    // The headings people see; a closed header menu shows none of its own.
    const headings = await browser.execute(() =>
      [...document.querySelectorAll('h1, h2, h3')]
        .filter((h) => h.checkVisibility())
        .map((h) => `${h.tagName}:${h.textContent.trim()}`),
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
      ['16 to 13 parts', text('#result-count', '16 matching', '13 matching')],
      // The list opens with every category section closed.
      ['Resistors to Resistars', text('.results', 'Resistors', 'Resistars')],
      ['full stop to comma', text('.empty-state p', '.', ',')],
      ['Capacitors to Capacitars', text('.results', 'Capacitors', 'Capacitars')],
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
    await searchFor(browser, 'sample-mcu', '1 matching part');
    await selectResult(browser, 'SAMPLE-MCU-32');
    await waitForText(browser, '#part-title', 'SAMPLE-MCU-32');
    await waitForPreview(browser, SYMBOL);
    await waitForPreview(browser, FOOTPRINT);

    const units = await images(browser, `${SYMBOL} figure img`);
    assert.equal(units.length, 2, 'the two-unit symbol shows both units');
    units.forEach((unit, n) => {
      const letter = String.fromCharCode(65 + n);
      assert.ok(unit.loaded && unit.svg, `unit ${letter} rendered from a kicad-cli SVG`);
      assert.match(unit.alt, new RegExp(`^Symbol g-mcu:SAMPLE_MCU, unit ${letter} \\(${n + 1} of ${units.length}\\)$`));
    });
    const footprint = await images(browser, `${FOOTPRINT} figure img`);
    assert.equal(footprint.length, 1);
    assert.ok(footprint[0].loaded && footprint[0].svg);
    assert.equal(footprint[0].alt, 'Footprint Package_QFP:LQFP-32_7x7mm_P0.8mm');

    assert.match(
      await textOf(browser, `${SYMBOL} .provenance`),
      /^team catalog, symbol revision 1 · kicad-cli 10\.\S+ · exported now$/,
    );
    assert.match(await textOf(browser, `${FOOTPRINT} .provenance`), /^KiCad library · kicad-cli 10\.\S+ · exported now$/);
    record({ event: 'preview', part: 'SAMPLE-MCU-32', units: units.length });

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
    assert.equal(await textOf(browser, `${dialog} h4`), 'Symbol g-mcu:SAMPLE_MCU, unit A (1 of 2)');
    // The close-up's viewer (kicad-parts-34df) shows kicad-cli's drawing.
    await browser.waitUntil(
      async () =>
        browser.execute((s) => {
          const viewer = document.querySelector(`${s} .viewer`);
          return viewer?.dataset.ready === 'true' && viewer.querySelector('svg image')?.getAttribute('href')?.startsWith('data:image/svg+xml;base64,');
        }, dialog),
      { timeout: 5_000, timeoutMsg: 'the close-up never showed the drawing' },
    );
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

    await searchFor(browser, 'sample-mcu', '1 matching part');
    await selectResult(browser, 'SAMPLE-MCU-32');
    await waitForText(browser, `${SYMBOL} .provenance`, /from cache$/);
    await waitForText(browser, `${FOOTPRINT} .provenance`, /from cache$/);
  });

  it("shows kicad-cli's own message when an export fails", async () => {
    await searchFor(browser, 'TP4333', '1 matching part');
    await selectResult(browser, 'TP4333');
    await waitForPreview(browser, SYMBOL);
    const alert = `${SYMBOL} [role="alert"]`;
    assert.equal(await textOf(browser, `${alert} .problem-title`), 'kicad-cli could not export this symbol');
    assert.equal(await textOf(browser, `${alert} code`), 'Device:TP4333');
    assert.match(await textOf(browser, `${alert} .cli-message`), /There is no symbol selected to save\./);
    assert.match(await textOf(browser, `${alert} .command pre`), /^kicad-cli sym export svg /);
    await waitForPreview(browser, FOOTPRINT);
    await assertAccessible(browser, '05-export-failed');
    await screenshot(browser, '05-export-failed');
  });

  it('explains a footprint whose library is missing', async () => {
    await searchFor(browser, 'SN74LVC1G07DCKR', '1 matching part');
    await selectResult(browser, 'SN74LVC1G07DCKR');
    await waitForPreview(browser, SYMBOL);
    await waitForPreview(browser, FOOTPRINT);
    const alert = `${FOOTPRINT} [role="alert"]`;
    assert.equal(await textOf(browser, `${alert} .problem-title`), 'The footprint library was not found');
    assert.match(await textOf(browser, alert), /SOT-353/);
    // The library part stays whole on one line, and copying still gives the
    // exact reference: the spans and <wbr> add no characters.
    assert.equal(await textOf(browser, `${alert} code`), 'Sample_Missing:SOT-353');
    assert.equal(await textOf(browser, `${alert} code .lib`), 'Sample_Missing:');
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

  it('shows a single-unit part beside every part detail without scrolling', async () => {
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
    assert.equal(layout.fieldsFit, true, 'every part detail fits on the right');
    assert.equal(layout.fieldsOnTheRight, true);
    assert.equal(layout.collapsibles, 0, 'part details are always visible');
    assert.equal(layout.heading, 'Part details (4)');
    await assertAccessible(browser, '09-part-fields');
  });

  it('says when nothing matches', async () => {
    await searchFor(browser, 'no-such-part-xyz', 'No parts match.');
    assert.equal(await browser.execute(() => document.querySelectorAll('.results li').length), 0);
    await assertAccessible(browser, '08-no-results');
    await screenshot(browser, '08-no-results');
  });

  it('keeps each library name whole for every footprint choice', async () => {
    await searchFor(browser, 'sample-mcu', '1 matching part');
    await selectResult(browser, 'SAMPLE-MCU-32');
    await waitForText(browser, '#part-title', 'SAMPLE-MCU-32');
    const rows = await browser.execute(() =>
      [...document.querySelectorAll('.part-fields tbody tr')]
        .filter((tr) => tr.querySelector('th')?.textContent.trim() === 'Footprint')
        .map((tr) => {
          const td = tr.querySelector('td');
          return {
            text: td.textContent,
            libraries: [...td.querySelectorAll('.lib')].map((lib) => lib.textContent),
          };
        }),
    );
    // One row per choice. Copying gives the exact reference: the spans and
    // <wbr> add no characters.
    assert.deepEqual(
      rows.map((row) => row.libraries),
      [['Package_QFP:'], ['Package_DFN_QFN:']],
      JSON.stringify(rows),
    );
    assert.ok(rows[0].text.startsWith('Package_QFP:LQFP-32_7x7mm_P0.8mm'), rows[0].text);
    assert.ok(rows[1].text.startsWith('Package_DFN_QFN:QFN-32-1EP_5x5mm_P0.5mm_EP3.45x3.45mm'), rows[1].text);
  });
});
