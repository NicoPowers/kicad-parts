// Checks that a drawn 3D view stays steady and can't be lost
// (kicad-parts-4588). Journeys 20 and 21 run them on different models.
//
// The checks read the view through view3dInfo(), which draws and reads
// in one call: its camera numbers, where the model's centre shows, and
// how many pixels differ from the background. They never compare exact
// GPU pixels, which differ between machines (mx-1c7a9b).

import assert from 'node:assert/strict';
import { record } from './harness.mjs';

/** What the 3D view `area` draws: its meshes, drawn pixels and camera. */
export async function view3dInfo(browser, area) {
  return browser.execute((s) => document.querySelector(s).view3dInfo(), area);
}

/** A drag with mouse `button` (0 left, 1 middle, 2 right) in two steps. */
export async function drag(browser, from, to, button) {
  await browser
    .action('pointer', { parameters: { pointerType: 'mouse' } })
    .move({ x: from.x, y: from.y })
    .down({ button })
    .move({ x: Math.round((from.x + to.x) / 2), y: Math.round((from.y + to.y) / 2), duration: 50 })
    .move({ x: to.x, y: to.y, duration: 50 })
    .up({ button })
    .perform();
}

/** `times` wheel events of `deltaY` at `at`, as fast as they can come. */
export async function wheelBurst(browser, at, deltaY, times) {
  let burst = browser.action('wheel');
  for (let i = 0; i < times; i += 1) burst = burst.scroll({ x: at.x, y: at.y, deltaX: 0, deltaY, duration: 0 });
  await burst.perform();
}

export function near(a, b, tolerance, message) {
  assert.ok(Math.abs(a - b) <= tolerance, `${message}: ${a} is not ${b}`);
}

/** The page pixel where `onScreen` ([x, y], -1 to 1, y up) shows in `box`. */
function pixelOf(box, [x, y]) {
  return { x: Math.round(box.left + ((x + 1) / 2) * box.width), y: Math.round(box.top + ((1 - y) / 2) * box.height) };
}

/** Whether every camera number of `info` is finite. */
function cameraValid(info) {
  return [info.azimuth, info.elevation, info.zoom, ...info.shift, ...info.centreOnScreen].every(Number.isFinite);
}

/**
 * Checks, on the focused 3D view `area`, that:
 * - after zooming at a corner and panning, a turn goes around the model's
 *   centre, which stays put on screen, and the model stays in view;
 * - fast bursts of wheel turns stop at the zoom limits, with the model
 *   still in view at a sensible size;
 * - long pans stop with the middle of the view over the model;
 * - 0 and Reset view bring back the starting view exactly.
 * `reset` clicks the Reset view button.
 */
export async function checkSteady3d(browser, area, reset) {
  await browser.execute((s) => document.querySelector(s).focus(), area);
  await browser.keys('0');
  const start = await view3dInfo(browser, area);
  record({ event: '3d-steady-start', ...start, meshNames: undefined });
  near(start.centreOnScreen[0], 0, 1e-6, 'the model starts in the middle across');
  near(start.centreOnScreen[1], 0, 1e-6, 'the model starts in the middle up and down');
  const box = await browser.execute((s) => {
    const rect = document.querySelector(s).getBoundingClientRect();
    return { left: rect.left, top: rect.top, width: rect.width, height: rect.height };
  }, area);
  const middle = pixelOf(box, [0, 0]);

  // Zoom in at a corner of the model, then pan.
  const corner = pixelOf(box, [-0.25, 0.2]);
  await wheelBurst(browser, corner, -150, 4);
  await drag(browser, middle, { x: middle.x + 30, y: middle.y - 20 }, 2);
  const before = await view3dInfo(browser, area);
  assert.ok(before.zoom > start.zoom * 2, `the wheel zoomed in: ${start.zoom} to ${before.zoom}`);
  const [bx, by] = before.centreOnScreen;
  assert.ok(Math.hypot(bx, by) > 0.1, `zooming at the corner and panning moved the centre: ${bx}, ${by}`);

  // Turn: the centre stays where it is on screen, and the model shows.
  await drag(browser, middle, { x: middle.x + 80, y: middle.y + 30 }, 0);
  const turned = await view3dInfo(browser, area);
  near(turned.azimuth, before.azimuth - 40, 0.01, 'an 80 pixel drag turns it 40 degrees');
  near(turned.centreOnScreen[0], bx, 1e-4, 'the centre stays put across while it turns');
  near(turned.centreOnScreen[1], by, 1e-4, 'the centre stays put up and down while it turns');
  assert.deepEqual(turned.shift, before.shift, 'turning keeps the pan');
  assert.ok(turned.drawn > 2_000, `the turned model is in view: ${turned.drawn} pixels`);
  await browser.keys(['ArrowLeft', 'ArrowDown']);
  const keyed = await view3dInfo(browser, area);
  near(keyed.centreOnScreen[0], bx, 1e-4, 'the arrow keys turn around the centre too');
  near(keyed.centreOnScreen[1], by, 1e-4, 'the arrow keys turn around the centre too');

  // Fast bursts of big wheel turns, over the model, as a fine wheel or a
  // trackpad sends them.
  const overModel = pixelOf(box, keyed.centreOnScreen);
  await wheelBurst(browser, overModel, -2_000, 40);
  const closest = await view3dInfo(browser, area);
  record({ event: '3d-steady-closest', zoom: closest.zoom, drawn: closest.drawn, shift: closest.shift });
  assert.ok(cameraValid(closest), JSON.stringify(closest));
  assert.equal(closest.zoom, closest.zoomLimits[1], 'zooming in stops at its limit');
  assert.ok(closest.drawn > closest.width * closest.height * 0.5, `zoomed right in, the model fills the view: ${closest.drawn} pixels`);
  await wheelBurst(browser, corner, 2_000, 40);
  const farthest = await view3dInfo(browser, area);
  record({ event: '3d-steady-farthest', zoom: farthest.zoom, drawn: farthest.drawn, shift: farthest.shift });
  assert.ok(cameraValid(farthest), JSON.stringify(farthest));
  assert.equal(farthest.zoom, farthest.zoomLimits[0], 'zooming out stops at its limit');
  assert.ok(
    farthest.drawn > start.drawn / 40 && farthest.drawn < start.drawn,
    `zoomed right out, the model is small but seen: ${farthest.drawn} pixels, ${start.drawn} at the start`,
  );

  // Long pans, by drag and by key, stop with the model in view. Shift and
  // the left arrow pan the way a drag to the right does.
  await browser.keys('0');
  for (let i = 0; i < 6; i += 1) await drag(browser, middle, { x: middle.x + 150, y: middle.y + 100 }, 2);
  const keys = [];
  for (let i = 0; i < 30; i += 1) keys.push('Shift', 'ArrowLeft', 'Shift');
  await browser.keys(keys);
  const panned = await view3dInfo(browser, area);
  record({ event: '3d-steady-panned', shift: panned.shift, centreOnScreen: panned.centreOnScreen, drawn: panned.drawn });
  assert.ok(cameraValid(panned), JSON.stringify(panned));
  assert.ok(panned.shift.every((value) => Math.abs(value) <= panned.radius), `the pan stops near the model: ${panned.shift}`);
  assert.ok(panned.centreOnScreen.every((value) => Math.abs(value) < 1), `the model's centre is in view: ${panned.centreOnScreen}`);
  assert.ok(panned.drawn > start.drawn / 4, `the panned model is in view: ${panned.drawn} pixels`);

  // 0 and Reset view bring back the starting view, exactly.
  await browser.keys('0');
  assert.deepEqual(await view3dInfo(browser, area), start, '0 shows the starting view');
  await drag(browser, middle, { x: middle.x - 50, y: middle.y }, 0);
  await wheelBurst(browser, corner, -300, 3);
  await reset();
  assert.deepEqual(await view3dInfo(browser, area), start, 'Reset view shows the starting view');
}
