// The drawing close-up (kicad-parts-34df): the mouse wheel zooms at the
// pointer, a left or middle drag moves the drawing, Fit shows all of it,
// and the keyboard does the same. A footprint is drawn layer by layer:
// each layer can be hidden, and one is the active layer, drawn on top.
// A symbol keeps kicad-cli's drawing and stays sharp when zoomed.

import assert from 'node:assert/strict';
import { after, before, describe, it } from 'node:test';
import {
  FOOTPRINT,
  SYMBOL,
  assertAccessible,
  launchApp,
  record,
  screenshot,
  searchFor,
  selectResult,
  textOf,
  waitForPreview,
} from '../harness.mjs';

const CLOSE_UP = `${FOOTPRINT} dialog.zoom-dialog`;
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

  before(async () => {
    ({ browser } = await launchApp());
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
    // Tab from the zoom buttons to the drawing.
    await browser.execute((s) => document.querySelector(`${s} .viewer-fit`).focus(), CLOSE_UP);
    await browser.keys(['Tab', 'Tab', 'Tab']);
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
});
