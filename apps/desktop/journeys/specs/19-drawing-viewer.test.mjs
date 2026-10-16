// The drawing close-up (kicad-parts-34df): the mouse wheel zooms at the
// pointer, a left or middle drag moves the drawing, Fit shows all of it,
// and the keyboard does the same. A footprint is drawn layer by layer:
// each layer can be hidden, and one is the active layer, drawn on top.
// A symbol keeps kicad-cli's drawing and stays sharp when zoomed.
// The Measure tool (kicad-parts-e95e) measures between points that snap
// to the active layer, and the gap between two pads.

import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { after, before, describe, it } from 'node:test';
import {
  FOOTPRINT,
  SYMBOL,
  assertAccessible,
  launchApp,
  record,
  repoRoot,
  screenshot,
  searchFor,
  selectResult,
  tempDir,
  textOf,
  waitForPreview,
  waitForText,
} from '../harness.mjs';

const CLOSE_UP = `${FOOTPRINT} dialog.zoom-dialog`;
const LIBRARIES = 'dialog.libraries-dialog';
const LIBRARY_PREVIEW = '.libraries-detail .preview';

/**
 * KiCad settings whose footprint table has one installed library: the
 * journey's own fixtures, with a footprint whose paste shrinks.
 */
function kicadSettings() {
  const config = tempDir('kicad-settings');
  const fixtures = path.join(repoRoot, 'apps', 'desktop', 'journeys', 'fixtures', 'Journey.pretty').replaceAll('\\', '/');
  fs.writeFileSync(path.join(config, 'sym-lib-table'), '(sym_lib_table (version 7)\n)\n');
  fs.writeFileSync(
    path.join(config, 'fp-lib-table'),
    `(fp_lib_table (version 7)\n  (lib (name "Journey")(type "KiCad")(uri "${fixtures}")(options "")(descr "Journey fixtures"))\n)\n`,
  );
  return config;
}
const AREA = `${CLOSE_UP} .viewer-area`;

/** The viewer's view in `scope`: its viewBox and pixels per millimetre. */
async function view(browser, scope) {
  return browser.execute((s) => {
    const area = document.querySelector(`${s} .viewer-area`);
    const box = area.querySelector('svg').getAttribute('viewBox').split(' ').map(Number);
    const rect = area.querySelector('svg').getBoundingClientRect();
    return { box, scale: Math.min(rect.width / box[2], rect.height / box[3]) };
  }, scope);
}

/** The drawing point at a screen position in `scope`'s viewer. */
async function drawingPointAt(browser, scope, x, y) {
  return browser.execute(
    (s, x, y) => {
      const area = document.querySelector(`${s} .viewer-area`);
      const [bx, by, bw, bh] = area.querySelector('svg').getAttribute('viewBox').split(' ').map(Number);
      const rect = area.querySelector('svg').getBoundingClientRect();
      const scale = Math.min(rect.width / bw, rect.height / bh);
      const left = rect.left + (rect.width - bw * scale) / 2;
      const top = rect.top + (rect.height - bh * scale) / 2;
      return { x: bx + (x - left) / scale, y: by + (y - top) / scale };
    },
    scope,
    x,
    y,
  );
}

/** The screen position of a drawing point in `scope`'s viewer. */
async function screenAt(browser, scope, x, y) {
  return browser.execute(
    (s, x, y) => {
      const area = document.querySelector(`${s} .viewer-area`);
      const [bx, by, bw, bh] = area.querySelector('svg').getAttribute('viewBox').split(' ').map(Number);
      const rect = area.querySelector('svg').getBoundingClientRect();
      const scale = Math.min(rect.width / bw, rect.height / bh);
      const left = rect.left + (rect.width - bw * scale) / 2;
      const top = rect.top + (rect.height - bh * scale) / 2;
      return { x: Math.round(left + (x - bx) * scale), y: Math.round(top + (y - by) * scale) };
    },
    scope,
    x,
    y,
  );
}

async function clickAt(browser, at) {
  await browser
    .action('pointer', { parameters: { pointerType: 'mouse' } })
    .move({ x: at.x, y: at.y })
    .down({ button: 0 })
    .up({ button: 0 })
    .perform();
}

/** The Measure readout's lines. */
async function readout(browser, scope) {
  return browser.execute(
    (s) => [...document.querySelectorAll(`${s} .viewer-readout p`)].map((line) => line.textContent),
    scope,
  );
}

/** Presses N until the readout highlights `name`. */
async function highlight(browser, scope, name) {
  for (let n = 0; n < 60; n += 1) {
    await browser.keys('n');
    if ((await readout(browser, scope)).includes(`Highlighted: ${name}.`)) return;
  }
  assert.fail(`N never highlighted ${name}`);
}

/** The centre of `selector` on screen. */
async function centreOf(browser, selector) {
  return browser.execute((s) => {
    const rect = document.querySelector(s).getBoundingClientRect();
    return { x: Math.round(rect.left + rect.width / 2), y: Math.round(rect.top + rect.height / 2) };
  }, selector);
}

async function wheel(browser, x, y, deltaY) {
  await browser.action('wheel').scroll({ x, y, deltaX: 0, deltaY, duration: 0 }).perform();
}

async function drag(browser, from, to, button) {
  await browser
    .action('pointer', { parameters: { pointerType: 'mouse' } })
    .move({ x: from.x, y: from.y })
    .down({ button })
    .move({ x: Math.round((from.x + to.x) / 2), y: Math.round((from.y + to.y) / 2), duration: 50 })
    .move({ x: to.x, y: to.y, duration: 50 })
    .up({ button })
    .perform();
}

/** Waits until the viewer in `scope` has its drawing, which comes when shown. */
async function waitForDrawing(browser, scope) {
  await browser.waitUntil(
    async () => browser.execute((s) => document.querySelector(`${s} .viewer`)?.dataset.ready === 'true', scope),
    { timeout: 10_000, timeoutMsg: `${scope} never got its drawing` },
  );
}

async function isOpen(browser, dialog) {
  return browser.execute((s) => document.querySelector(s)?.open === true, dialog);
}

function near(a, b, tolerance, message) {
  assert.ok(Math.abs(a - b) <= tolerance, `${message}: ${a} is not ${b}`);
}

describe('drawing close-up', () => {
  let browser;
  let dataDir;

  before(async () => {
    ({ browser, dataDir } = await launchApp({ args: [`--kicad-config-dir=${kicadSettings()}`] }));
  });

  after(async () => {
    await browser?.deleteSession();
  });

  it('opens a footprint drawn layer by layer, front copper active', async () => {
    await searchFor(browser, 'LTC4364', '1 matching part');
    await selectResult(browser, 'LTC4364HDE-2#TRPBF');
    await waitForPreview(browser, FOOTPRINT);
    await (await browser.$(`${FOOTPRINT} figure button`)).click();
    await browser.waitUntil(async () => isOpen(browser, CLOSE_UP), { timeout: 5_000, timeoutMsg: 'the close-up did not open' });
    await waitForDrawing(browser, CLOSE_UP);
    assert.equal(await textOf(browser, `${CLOSE_UP} h4`), 'Footprint Package_DFN_QFN:DFN-14-1EP_3x4mm_P0.5mm_EP1.7x3.3mm');
    const shown = await browser.execute((s) => {
      const dialog = document.querySelector(s);
      return {
        active: dialog.querySelector('.viewer').dataset.activeLayer,
        listed: [...dialog.querySelectorAll('.viewer-layers .layer-name')].map((name) => name.textContent),
        drawn: [...dialog.querySelectorAll('g.layer')].map((group) => group.dataset.layer),
        pads: dialog.querySelector('g.layer[data-layer="F.Cu"]').querySelectorAll('path, circle').length,
        pasteShapes: dialog.querySelector('g.layer[data-layer="F.Paste"]').querySelectorAll('path, circle').length,
        name: dialog.querySelector('.viewer-area').getAttribute('aria-label'),
        role: dialog.querySelector('.viewer-area').getAttribute('role'),
        help: document.getElementById(dialog.querySelector('.viewer-area').getAttribute('aria-describedby'))?.textContent,
        note: [...dialog.querySelectorAll('.viewer-note')].map((note) => note.textContent),
      };
    }, CLOSE_UP);
    record({ event: 'footprint-close-up', ...shown });
    assert.equal(shown.active, 'F.Cu');
    assert.deepEqual(shown.listed, ['F.Cu', 'F.Paste', 'F.SilkS', 'F.Mask', 'F.CrtYd', 'F.Fab']);
    // Front copper, the active layer, is drawn last, on top.
    assert.equal(shown.drawn.at(-1), 'F.Cu');
    assert.equal(shown.pads, 15, '14 pads and the exposed pad');
    // Paste for the 14 pads, and the exposed pad's paste in openings.
    assert.ok(shown.pasteShapes > 15, `${shown.pasteShapes} paste shapes`);
    assert.equal(shown.name, 'Footprint Package_DFN_QFN:DFN-14-1EP_3x4mm_P0.5mm_EP1.7x3.3mm');
    assert.equal(shown.role, 'application');
    assert.match(shown.help, /zoom/);
    assert.deepEqual(shown.note, ["A board's own settings can change paste and mask openings."]);
    await assertAccessible(browser, '72-footprint-close-up');
    await screenshot(browser, '72-footprint-close-up');
  });

  it('zooms at the pointer with the wheel, and moves with a left or middle drag', async () => {
    const fitted = await view(browser, CLOSE_UP);
    const centre = await centreOf(browser, AREA);
    const pointer = { x: centre.x + 120, y: centre.y - 60 };
    const before = await drawingPointAt(browser, CLOSE_UP, pointer.x, pointer.y);
    await wheel(browser, pointer.x, pointer.y, -300);
    const zoomed = await view(browser, CLOSE_UP);
    assert.ok(zoomed.scale > fitted.scale * 1.5, `zoomed from ${fitted.scale} to ${zoomed.scale} px/mm`);
    const after = await drawingPointAt(browser, CLOSE_UP, pointer.x, pointer.y);
    near(after.x, before.x, 0.01, 'the point under the pointer stays (x)');
    near(after.y, before.y, 0.01, 'the point under the pointer stays (y)');

    // A left drag moves the drawing with the pointer.
    await drag(browser, centre, { x: centre.x + 100, y: centre.y + 40 }, 0);
    const moved = await view(browser, CLOSE_UP);
    near(moved.box[0], zoomed.box[0] - 100 / zoomed.scale, 0.01, 'moved left by the drag');
    near(moved.box[1], zoomed.box[1] - 40 / zoomed.scale, 0.01, 'moved up by the drag');
    // A middle drag too, ending far outside the close-up, which stays open.
    await drag(browser, centre, { x: 5, y: 5 }, 1);
    const middle = await view(browser, CLOSE_UP);
    assert.ok(middle.box[0] > moved.box[0], 'the middle button moved the drawing');
    assert.equal(await isOpen(browser, CLOSE_UP), true, 'a drag that ends outside does not close the close-up');
    // A click on the drawing does not close it either.
    await (await browser.$(AREA)).click();
    assert.equal(await isOpen(browser, CLOSE_UP), true, 'a click on the drawing does not close the close-up');

    await (await browser.$(`${CLOSE_UP} .viewer-fit`)).click();
    assert.deepEqual((await view(browser, CLOSE_UP)).box, fitted.box, 'Fit shows the whole drawing again');
  });

  it('turns the paste layer off and on, and changes the active layer', async () => {
    const pasteShown = () =>
      browser.execute((s) => document.querySelector(`${s} g.layer[data-layer="F.Paste"]`).getAttribute('display'), CLOSE_UP);
    await (await browser.$(`${CLOSE_UP} input[aria-label="Show F.Paste"]`)).click();
    assert.equal(await pasteShown(), 'none');
    await screenshot(browser, '73-footprint-paste-off');
    await (await browser.$(`${CLOSE_UP} input[aria-label="Show F.Paste"]`)).click();
    assert.equal(await pasteShown(), 'inline');

    // Choosing a layer's name makes it the active layer, drawn on top.
    const label = await browser.$(`${CLOSE_UP} .viewer-layers li:nth-child(2) label`);
    assert.equal(await label.getText(), 'F.Paste\nFront paste (stencil)');
    await label.click();
    const active = await browser.execute((s) => {
      const dialog = document.querySelector(s);
      return {
        active: dialog.querySelector('.viewer').dataset.activeLayer,
        top: [...dialog.querySelectorAll('g.layer')].at(-1).dataset.layer,
        checked: dialog.querySelector('.viewer-layers li:nth-child(2) input[type="radio"]').checked,
      };
    }, CLOSE_UP);
    assert.deepEqual(active, { active: 'F.Paste', top: 'F.Paste', checked: true });
    // Dimming the others shows the paste layer alone.
    await (await browser.$(`${CLOSE_UP} .viewer-dim input`)).click();
    await screenshot(browser, '74-footprint-paste-active');
    await (await browser.$(`${CLOSE_UP} .viewer-dim input`)).click();
    await (await browser.$(`${CLOSE_UP} .viewer-layers li:nth-child(1) label`)).click();
  });

  it('zooms, moves and fits with the keyboard, with visible focus', async () => {
    const fitted = await view(browser, CLOSE_UP);
    // Tab from the zoom, 3D, Measure and Units buttons to the drawing.
    await browser.execute((s) => document.querySelector(`${s} .viewer-fit`).focus(), CLOSE_UP);
    await browser.keys(['Tab', 'Tab', 'Tab', 'Tab', 'Tab', 'Tab']);
    const focus = await browser.execute((s) => {
      const area = document.querySelector(`${s} .viewer-area`);
      return {
        focused: document.activeElement === area,
        visible: area.matches(':focus-visible'),
        outline: getComputedStyle(area).outlineStyle,
      };
    }, CLOSE_UP);
    assert.deepEqual(focus, { focused: true, visible: true, outline: 'solid' });
    await browser.keys('+');
    const zoomed = await view(browser, CLOSE_UP);
    near(zoomed.scale, fitted.scale * 1.5, 0.01, 'plus zooms in');
    await browser.keys('ArrowLeft');
    const moved = await view(browser, CLOSE_UP);
    assert.ok(moved.box[0] < zoomed.box[0], 'the left arrow moves the view left');
    await browser.keys('-');
    near((await view(browser, CLOSE_UP)).scale, fitted.scale, 0.01, 'minus zooms out');
    await browser.keys('0');
    assert.deepEqual((await view(browser, CLOSE_UP)).box, fitted.box, '0 fits the drawing');
  });

  it('zooms in far enough to see one pad corner smoothly', async () => {
    await browser.keys('Escape');
    await browser.waitUntil(async () => !(await isOpen(browser, CLOSE_UP)), { timeout: 5_000, timeoutMsg: 'Escape did not close the close-up' });
    // A 0603 resistor, whose pads have rounded corners.
    await searchFor(browser, 'RC0603FR-0747RL', '1 matching part');
    await selectResult(browser, 'RC0603FR-0747RL');
    await waitForPreview(browser, FOOTPRINT);
    await (await browser.$(`${FOOTPRINT} figure button`)).click();
    await browser.waitUntil(async () => isOpen(browser, CLOSE_UP), { timeout: 5_000, timeoutMsg: 'the close-up did not open' });
    await waitForDrawing(browser, CLOSE_UP);
    // Halfway along the first pad's top left corner, on screen.
    const corner = await browser.execute((s) => {
      const pad = document.querySelector(`${s} g.layer[data-layer="F.Cu"] path`);
      const box = pad.getBBox();
      const radius = Math.min(box.width, box.height) * 0.25;
      const inset = radius * (1 - Math.SQRT1_2);
      const matrix = pad.getScreenCTM();
      return {
        x: Math.round((box.x + inset) * matrix.a + matrix.e),
        y: Math.round((box.y + inset) * matrix.d + matrix.f),
      };
    }, CLOSE_UP);
    for (let n = 0; n < 6; n += 1) await wheel(browser, corner.x, corner.y, -500);
    const close = await view(browser, CLOSE_UP);
    record({ event: 'closest-zoom', pixelsPerMm: close.scale });
    assert.ok(close.scale >= 1000, `${close.scale} pixels per millimetre`);
    // The pointer's position, in footprint millimetres.
    await browser.action('pointer').move({ x: corner.x, y: corner.y }).perform();
    const position = await textOf(browser, `${CLOSE_UP} .viewer-position`);
    const expected = await drawingPointAt(browser, CLOSE_UP, corner.x, corner.y);
    const shown = position.match(/^X (−?\d+\.\d{3}) mm, Y (−?\d+\.\d{3}) mm$/);
    assert.ok(shown, position);
    const value = (text) => Number(text.replace('−', '-'));
    near(value(shown[1]), expected.x, 0.001, 'the X shown');
    near(value(shown[2]), expected.y, 0.001, 'the Y shown');
    await screenshot(browser, '75-footprint-pad-corner');
    await browser.keys('Escape');
    await browser.waitUntil(async () => !(await isOpen(browser, CLOSE_UP)), { timeout: 5_000, timeoutMsg: 'Escape did not close the close-up' });
  });

  it('measures between pads that snap to the active layer, and the gap between them', async () => {
    await (await browser.$(`${FOOTPRINT} figure button`)).click();
    await browser.waitUntil(async () => isOpen(browser, CLOSE_UP), { timeout: 5_000, timeoutMsg: 'the close-up did not open' });
    await waitForDrawing(browser, CLOSE_UP);
    const toggle = await browser.$(`${CLOSE_UP} .viewer-measure`);
    assert.equal(await toggle.getAttribute('aria-pressed'), 'false');
    await toggle.click();
    const on = await browser.execute((s) => {
      const area = document.querySelector(`${s} .viewer-area`);
      return {
        pressed: document.querySelector(`${s} .viewer-measure`).getAttribute('aria-pressed'),
        cursor: getComputedStyle(area).cursor,
        live: document.querySelector(`${s} .viewer-readout`).getAttribute('aria-live'),
        // Clicks on the drawing never select text, but the readout can
        // still be selected and copied.
        areaSelect: getComputedStyle(area).userSelect,
        readoutSelectable: getComputedStyle(document.querySelector(`${s} .viewer-readout`)).userSelect !== 'none',
      };
    }, CLOSE_UP);
    assert.deepEqual(on, { pressed: 'true', cursor: 'crosshair', live: 'polite', areaSelect: 'none', readoutSelectable: true });
    assert.match((await readout(browser, CLOSE_UP))[0], /^Click a point to start\./);

    // The pads of our g-Resistor_SMD:R_0603_1608Metric are at x = -0.825
    // and 0.825 mm.
    // A click a few pixels off a pad's centre snaps to it.
    const one = await screenAt(browser, CLOSE_UP, -0.825, 0);
    const two = await screenAt(browser, CLOSE_UP, 0.825, 0);
    await clickAt(browser, { x: one.x + 3, y: one.y - 2 });
    assert.deepEqual(await readout(browser, CLOSE_UP), ['From Pad 1 centre. Click the end point.']);
    await clickAt(browser, { x: two.x - 2, y: two.y + 3 });
    const copper = await readout(browser, CLOSE_UP);
    record({ event: 'measure-copper', copper });
    assert.deepEqual(copper, [
      'Distance 1.650 mm, \u0394X 1.650 mm, \u0394Y 0.000 mm.',
      'From Pad 1 centre to Pad 2 centre.',
      'Gap between pad 1 and pad 2 on F.Cu: 0.850 mm.',
    ]);
    const drawn = await browser.execute((s) => {
      const measure = document.querySelector(`${s} g.measure`);
      const gap = measure.querySelector('.measure-gap');
      return {
        line: measure.querySelectorAll('.measure-line').length,
        ends: measure.querySelectorAll('.measure-end').length,
        label: measure.querySelector('.measure-label')?.textContent,
        gap: gap ? gap.getBBox().width : null,
      };
    }, CLOSE_UP);
    assert.equal(drawn.line, 1);
    assert.equal(drawn.ends, 2);
    assert.equal(drawn.label, '1.650 mm');
    near(drawn.gap, 0.85, 0.001, 'the gap line runs between the copper edges');
    await assertAccessible(browser, '78-footprint-measure');
    await screenshot(browser, '78-footprint-measure');

    // Paste: choosing F.Paste makes it active, and a new measurement snaps
    // to its openings. The third click starts again.
    const pasteRow = await browser.execute(
      (s) => [...document.querySelectorAll(`${s} .viewer-layers li`)].findIndex((li) => li.querySelector('.layer-name').textContent === 'F.Paste') + 1,
      CLOSE_UP,
    );
    await (await browser.$(`${CLOSE_UP} .viewer-layers li:nth-child(${pasteRow}) label`)).click();
    assert.equal(await browser.execute((s) => document.querySelector(`${s} .viewer`).dataset.activeLayer, CLOSE_UP), 'F.Paste');
    await clickAt(browser, one);
    await clickAt(browser, two);
    const paste = await readout(browser, CLOSE_UP);
    record({ event: 'measure-paste', paste });
    assert.equal(paste[2], 'Gap between pad 1 and pad 2 on F.Paste: 0.850 mm.');
    await screenshot(browser, '79-footprint-measure-paste');

    // Hiding the active layer stops snapping, and the readout says so.
    await (await browser.$(`${CLOSE_UP} input[aria-label="Show F.Paste"]`)).click();
    assert.equal((await readout(browser, CLOSE_UP)).at(-1), 'F.Paste is hidden. Show it to snap to its points.');
    await (await browser.$(`${CLOSE_UP} input[aria-label="Show F.Paste"]`)).click();

    // Mils, for the readout and the pointer's position.
    await (await browser.$(`${CLOSE_UP} .viewer-units`)).click();
    assert.equal(await textOf(browser, `${CLOSE_UP} .viewer-units`), 'Units: mils');
    const mils = await readout(browser, CLOSE_UP);
    assert.deepEqual(mils, [
      'Distance 65.0 mils, \u0394X 65.0 mils, \u0394Y 0.0 mils.',
      'From Pad 1 centre to Pad 2 centre.',
      'Gap between pad 1 and pad 2 on F.Paste: 33.5 mils.',
    ]);
    await browser.action('pointer').move({ x: two.x, y: two.y }).perform();
    assert.match(await textOf(browser, `${CLOSE_UP} .viewer-position`), /^X 32\.5 mils, Y 0\.0 mils/);
    await screenshot(browser, '80-footprint-measure-mils');
    await (await browser.$(`${CLOSE_UP} .viewer-units`)).click();

    // The keyboard: Escape clears the measurement and keeps the close-up
    // open; N steps through the snap points and Enter picks one.
    await browser.execute((s) => document.querySelector(`${s} .viewer-area`).focus(), CLOSE_UP);
    await browser.keys('Escape');
    assert.equal(await isOpen(browser, CLOSE_UP), true, 'Escape cleared the measurement only');
    assert.match((await readout(browser, CLOSE_UP))[0], /^Click a point to start\./);
    assert.equal(await browser.execute((s) => document.querySelector(`${s} g.measure .measure-line`), CLOSE_UP), null);
    await browser.keys('m');
    assert.equal(await toggle.getAttribute('aria-pressed'), 'false', 'M turns the tool off');
    await browser.keys('m');
    assert.equal(await toggle.getAttribute('aria-pressed'), 'true', 'M turns it on again');
    // Move the pointer away, so only the keys pick points. Zoomed in, the
    // view moves to keep the highlighted point in sight.
    await browser.action('pointer').move({ x: 5, y: 5 }).perform();
    await browser.keys(['+', '+', '+', '+']);
    await highlight(browser, CLOSE_UP, 'Pad 1 centre');
    const inView = await browser.execute((s) => {
      const marker = document.querySelector(`${s} g.measure .measure-snap`).getBoundingClientRect();
      const area = document.querySelector(`${s} .viewer-area`).getBoundingClientRect();
      return marker.left >= area.left && marker.right <= area.right && marker.top >= area.top && marker.bottom <= area.bottom;
    }, CLOSE_UP);
    assert.equal(inView, true, 'the highlighted point is in view');
    await browser.keys('Enter');
    await browser.keys('0');
    await highlight(browser, CLOSE_UP, 'Pad 2 centre');
    // Shift+N steps back.
    await browser.keys('n');
    await browser.keys(['Shift', 'N', 'Shift']);
    assert.equal((await readout(browser, CLOSE_UP)).at(-1), 'Highlighted: Pad 2 centre.');
    await browser.keys('Enter');
    const keyed = await readout(browser, CLOSE_UP);
    assert.deepEqual(keyed.slice(0, 3), [
      'Distance 1.650 mm, \u0394X 1.650 mm, \u0394Y 0.000 mm.',
      'From Pad 1 centre to Pad 2 centre.',
      'Gap between pad 1 and pad 2 on F.Paste: 0.850 mm.',
    ]);
    await assertAccessible(browser, '81-footprint-measure-keys');
    await screenshot(browser, '81-footprint-measure-keys');
    await browser.execute((s) => document.querySelector(`${s} .viewer-area`).focus(), CLOSE_UP);
    // A held Escape repeats; a repeat neither clears nor closes.
    await browser.execute(
      (s) =>
        document
          .querySelector(`${s} .viewer-area`)
          .dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', repeat: true, bubbles: true, cancelable: true })),
      CLOSE_UP,
    );
    assert.match((await readout(browser, CLOSE_UP))[0], /^Distance /, 'a repeated Escape keeps the measurement');
    await browser.keys('Escape');
    assert.equal(await isOpen(browser, CLOSE_UP), true, 'the first Escape clears');
    await browser.keys('Escape');
    await browser.waitUntil(async () => !(await isOpen(browser, CLOSE_UP)), { timeout: 5_000, timeoutMsg: 'the second Escape did not close the close-up' });
  });

  it('zooms a symbol, which stays sharp', async () => {
    const dialog = `${SYMBOL} dialog.zoom-dialog`;
    await waitForPreview(browser, SYMBOL);
    await (await browser.$(`${SYMBOL} figure button`)).click();
    await browser.waitUntil(async () => isOpen(browser, dialog), { timeout: 5_000, timeoutMsg: 'the symbol close-up did not open' });
    await browser.waitUntil(async () => browser.execute((s) => document.querySelector(`${s} .viewer`).dataset.ready === 'true', dialog), {
      timeout: 5_000,
      timeoutMsg: 'the symbol never loaded',
    });
    const symbol = await browser.execute((s) => {
      const viewer = document.querySelector(`${s} .viewer`);
      return {
        image: viewer.querySelector('svg image').getAttribute('href').startsWith('data:image/svg+xml;base64,'),
        layers: viewer.querySelectorAll('.viewer-layers').length,
        position: viewer.querySelectorAll('.viewer-position').length,
      };
    }, dialog);
    assert.deepEqual(symbol, { image: true, layers: 0, position: 0 });
    await assertAccessible(browser, '76-symbol-zoomed');
    const fitted = await view(browser, dialog);
    // The resistor's middle, where its zigzag is.
    const middle = await centreOf(browser, `${dialog} svg image`);
    for (let n = 0; n < 2; n += 1) await wheel(browser, middle.x, middle.y, -500);
    const zoomed = await view(browser, dialog);
    assert.ok(zoomed.scale > fitted.scale * 5, `${zoomed.scale} against ${fitted.scale}`);
    await screenshot(browser, '76-symbol-zoomed');
    await (await browser.$(`${dialog} .zoom-header button`)).click();
    await browser.waitUntil(async () => !(await isOpen(browser, dialog)), { timeout: 5_000, timeoutMsg: 'Close did not close the close-up' });
  });

  it("says when the layers cannot be loaded, and shows kicad-cli's drawing", async () => {
    // The footprint file the drawing is read from is gone from the cache.
    fs.rmSync(path.join(dataDir, 'cache', 'previews', 'v2'), { recursive: true, force: true });
    await (await browser.$(`${FOOTPRINT} figure button`)).click();
    await browser.waitUntil(async () => isOpen(browser, CLOSE_UP), { timeout: 5_000, timeoutMsg: 'the close-up did not open' });
    await waitForDrawing(browser, CLOSE_UP);
    const shown = await browser.execute((s) => {
      const viewer = document.querySelector(`${s} .viewer`);
      return {
        problem: viewer.querySelector('.viewer-problem')?.textContent ?? null,
        image: viewer.querySelector('svg image') !== null,
        layers: viewer.querySelectorAll('.viewer-layers').length,
      };
    }, CLOSE_UP);
    assert.match(shown.problem ?? '', /^The layers could not be loaded: /);
    assert.equal(shown.image, true);
    assert.equal(shown.layers, 0);
    await browser.keys('Escape');
    await browser.waitUntil(async () => !(await isOpen(browser, CLOSE_UP)), { timeout: 5_000, timeoutMsg: 'Escape did not close the close-up' });
  });

  it('shows how paste gaps differ from copper gaps, and follows the active layer', async () => {
    // Paste_Margin_Test, in the journey's own installed library: two 1 mm
    // pads 2 mm apart, whose paste shrinks by 0.1 mm on each side.
    await (await browser.$('#libraries-open')).click();
    await browser.waitUntil(async () => isOpen(browser, LIBRARIES), { timeout: 10_000, timeoutMsg: 'the Libraries browser did not open' });
    await (await browser.$('#library-tab-footprints')).click();
    await browser.waitUntil(
      async () =>
        browser.execute(() => {
          const list = document.querySelector('ul[aria-labelledby="library-group-installed"]');
          const button = list && [...list.querySelectorAll('.library-button')].find((b) => b.querySelector('.library-name')?.textContent === 'Journey');
          button?.click();
          return Boolean(button);
        }),
      { timeout: 60_000, interval: 250, timeoutMsg: 'no installed library Journey' },
    );
    await browser.waitUntil(
      async () =>
        browser.execute(() => {
          const row = [...document.querySelectorAll('#library-items [role="option"]')].find(
            (r) => r.querySelector('.library-item-name').textContent === 'Paste_Margin_Test',
          );
          row?.click();
          return Boolean(row);
        }),
      { timeout: 20_000, interval: 250, timeoutMsg: 'no row Paste_Margin_Test' },
    );
    await waitForText(browser, '#library-detail-title', /:Paste_Margin_Test$/);
    const enlarge = await browser.$(`${LIBRARY_PREVIEW} .viewer-enlarge`);
    await enlarge.waitForExist({ timeout: 60_000 });
    await enlarge.click();
    const closeUp = `${LIBRARY_PREVIEW} dialog.zoom-dialog`;
    await browser.waitUntil(async () => isOpen(browser, closeUp), { timeout: 5_000, timeoutMsg: 'Enlarge did not open the close-up' });
    await waitForDrawing(browser, closeUp);
    await (await browser.$(`${closeUp} .viewer-measure`)).click();
    const chooseLayer = async (layer) => {
      const row = await browser.execute(
        (s, wanted) => [...document.querySelectorAll(`${s} .viewer-layers li`)].findIndex((li) => li.querySelector('.layer-name').textContent === wanted) + 1,
        closeUp,
        layer,
      );
      await (await browser.$(`${closeUp} .viewer-layers li:nth-child(${row}) label`)).click();
    };

    // Centre to centre: the distance stays, and the gap grows on paste.
    await clickAt(browser, await screenAt(browser, closeUp, -1, 0));
    await clickAt(browser, await screenAt(browser, closeUp, 1, 0));
    const copper = await readout(browser, closeUp);
    record({ event: 'measure-paste-margin-copper', copper });
    assert.deepEqual(copper, [
      'Distance 2.000 mm, \u0394X 2.000 mm, \u0394Y 0.000 mm.',
      'From Pad 1 centre to Pad 2 centre.',
      'Gap between pad 1 and pad 2 on F.Cu: 1.000 mm.',
    ]);
    await chooseLayer('F.Paste');
    const paste = await readout(browser, closeUp);
    record({ event: 'measure-paste-margin-paste', paste });
    assert.deepEqual(paste, [
      'Distance 2.000 mm, \u0394X 2.000 mm, \u0394Y 0.000 mm.',
      'From Pad 1 centre to Pad 2 centre.',
      'Gap between pad 1 and pad 2 on F.Paste: 1.200 mm.',
    ]);

    // Corner to corner on copper; on paste the corners move to the
    // openings' corners.
    await chooseLayer('F.Cu');
    await clickAt(browser, await screenAt(browser, closeUp, -0.5, -0.5));
    await clickAt(browser, await screenAt(browser, closeUp, 0.5, -0.5));
    assert.equal((await readout(browser, closeUp))[1], 'From Pad 1 corner to Pad 2 corner.');
    await chooseLayer('F.Paste');
    const corners = await readout(browser, closeUp);
    record({ event: 'measure-paste-margin-corners', corners });
    assert.deepEqual(corners, [
      'Distance 1.200 mm, \u0394X 1.200 mm, \u0394Y 0.000 mm.',
      'From Pad 1 corner to Pad 2 corner.',
      'Gap between pad 1 and pad 2 on F.Paste: 1.200 mm.',
    ]);
    await browser.action('pointer').move({ x: 5, y: 5 }).perform();
    // The browser behind says how far it has read; wait until it is done.
    await browser.waitUntil(
      async () => browser.execute(() => document.querySelector('.libraries-progress')?.dataset.state === 'ready'),
      { timeout: 120_000, interval: 250, timeoutMsg: 'the Libraries browser never finished reading' },
    );
    await screenshot(browser, '82-footprint-measure-paste-margin', { blankOut: ['.library-facts .fact:last-child dd'] });
    // The courtyard has no pads: the points stay, and say their layer.
    await chooseLayer('F.CrtYd');
    const elsewhere = await readout(browser, closeUp);
    assert.equal(elsewhere[1], 'From Pad 1 corner on F.Paste to Pad 2 corner on F.Paste.');
    assert.equal(elsewhere.length, 2, `no gap on the courtyard: ${elsewhere}`);

    await browser.execute((s) => document.querySelector(`${s} .viewer-area`).focus(), closeUp);
    await browser.keys('Escape');
    assert.equal(await isOpen(browser, closeUp), true, 'the first Escape clears');
    await browser.keys('Escape');
    await browser.waitUntil(async () => !(await isOpen(browser, closeUp)), { timeout: 5_000, timeoutMsg: 'the second Escape did not close the close-up' });
    await browser.keys('Escape');
    await browser.waitUntil(async () => !(await isOpen(browser, LIBRARIES)), { timeout: 5_000, timeoutMsg: 'Escape did not close the Libraries browser' });
  });
});
