// A footprint's 3D view (kicad-parts-c9c1): the 3D button in a footprint
// close-up shows the footprint on a board with its 3D model, drawn by
// three.js from kicad-cli's GLB export. A left drag turns it, the wheel
// zooms and a middle or right drag pans; the keyboard does the same. A
// panel says which 3D model file each link uses, or why it is not shown.
//
// WebGL works here: under --disable-gpu, WebView2 draws WebGL with
// Microsoft's software renderer. That renderer can differ between
// machines, so screenshots hide the 3D canvas; the journey reads the
// drawn pixels and the GLB's meshes instead.

import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { after, before, describe, it } from 'node:test';
import {
  FOOTPRINT,
  assertAccessible,
  capture,
  launchApp,
  record,
  repoRoot,
  screenshot,
  screenshotsDir,
  searchFor,
  selectResult,
  tempDir,
  waitForPreview,
  waitForText,
} from '../harness.mjs';

const CLOSE_UP = `${FOOTPRINT} dialog.zoom-dialog`;
const LIBRARIES = 'dialog.libraries-dialog';
const LIBRARY_CLOSE_UP = '.libraries-detail .preview dialog.zoom-dialog';
/** Paths differ between machines. */
const PATHS = ['.model-path-text', '.library-facts .fact:last-child dd'];

/** KiCad settings whose footprint table lists the journey's 3D fixtures. */
function kicadSettings() {
  const config = tempDir('kicad-settings-3d');
  const fixtures = path.join(repoRoot, 'apps', 'desktop', 'journeys', 'fixtures', 'Journey3d.pretty').replaceAll('\\', '/');
  fs.writeFileSync(path.join(config, 'sym-lib-table'), '(sym_lib_table (version 7)\n)\n');
  fs.writeFileSync(
    path.join(config, 'fp-lib-table'),
    `(fp_lib_table (version 7)\n  (lib (name "Journey3d")(type "KiCad")(uri "${fixtures}")(options "")(descr "3D view fixtures"))\n)\n`,
  );
  return config;
}

async function isOpen(browser, dialog) {
  return browser.execute((s) => document.querySelector(s)?.open === true, dialog);
}

/** Whether `selector` in `scope` is shown (in the page and not display: none). */
async function shown(browser, scope, selector) {
  return browser.execute(
    (s, q) => {
      const element = document.querySelector(`${s} ${q}`);
      return element !== null && element.getClientRects().length > 0;
    },
    scope,
    selector,
  );
}

/** Waits until the 3D view in `scope` is drawn, or says why not. */
async function waitFor3d(browser, scope) {
  await browser.waitUntil(
    async () => browser.execute((s) => ['ready', 'problem'].includes(document.querySelector(`${s} .viewer-3d`)?.dataset.state), scope),
    { timeout: 90_000, interval: 100, timeoutMsg: `${scope}: the 3D view was never drawn` },
  );
  const state = await browser.execute((s) => {
    const area = document.querySelector(`${s} .viewer-3d`);
    return { state: area.dataset.state, text: area.textContent };
  }, scope);
  assert.equal(state.state, 'ready', `the 3D view says: ${state.text}`);
}

/** What the 3D view draws: its meshes, drawn pixels and camera. */
async function info(browser, scope) {
  return browser.execute((s) => document.querySelector(`${s} .viewer-3d`).view3dInfo(), scope);
}

/** The 3D model panel's links, with what each says. */
async function panel(browser, scope) {
  return browser.execute(
    (s) => ({
      links: [...document.querySelectorAll(`${s} .model-link`)].map((link) => ({
        link: link.querySelector('.model-link-text').textContent,
        found: link.querySelector('.model-found')?.textContent ?? null,
        problem: link.querySelector('.model-problem')?.textContent ?? null,
        path: link.querySelector('.model-path-text')?.textContent ?? null,
      })),
      notes: [...document.querySelectorAll(`${s} .model-note`)].map((note) => note.textContent),
    }),
    scope,
  );
}

/** Turns 3D on in `scope`, noting each status the view area shows. */
async function turn3dOn(browser, scope) {
  await browser.execute((s) => {
    window.journeyStatuses = [];
    const dialog = document.querySelector(s);
    const note = () => {
      const status = dialog.querySelector('.view3d-status')?.textContent;
      if (status && window.journeyStatuses.at(-1) !== status) window.journeyStatuses.push(status);
    };
    window.journeyObserver?.disconnect();
    window.journeyObserver = new MutationObserver(note);
    window.journeyObserver.observe(dialog, { subtree: true, childList: true, characterData: true });
  }, scope);
  await (await browser.$(`${scope} .viewer-3d-toggle`)).click();
  await waitFor3d(browser, scope);
  return browser.execute(() => {
    window.journeyObserver.disconnect();
    return window.journeyStatuses;
  });
}

/** A screenshot with the 3D canvas hidden, whose pixels differ by GPU. */
async function screenshotHiding3d(browser, name) {
  // The drawn view as it is, for people to look at; not compared.
  await capture(browser, path.join(screenshotsDir, `${name}-drawn.png`), PATHS);
  await browser.execute(() => {
    for (const canvas of document.querySelectorAll('.view3d-canvas')) canvas.style.setProperty('visibility', 'hidden');
  });
  await screenshot(browser, name, { blankOut: PATHS });
  await browser.execute(() => {
    for (const canvas of document.querySelectorAll('.view3d-canvas')) canvas.style.removeProperty('visibility');
  });
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

async function centreOf(browser, selector) {
  return browser.execute((s) => {
    const rect = document.querySelector(s).getBoundingClientRect();
    return { x: Math.round(rect.left + rect.width / 2), y: Math.round(rect.top + rect.height / 2) };
  }, selector);
}

function near(a, b, tolerance, message) {
  assert.ok(Math.abs(a - b) <= tolerance, `${message}: ${a} is not ${b}`);
}

/** Opens a footprint of the Journey3d library in its close-up. */
async function openFixture(browser, name, { group = 'installed', library = 'Journey3d' } = {}) {
  if (!(await isOpen(browser, LIBRARIES))) {
    await (await browser.$('#libraries-open')).click();
    await browser.waitUntil(async () => isOpen(browser, LIBRARIES), { timeout: 10_000, timeoutMsg: 'the Libraries browser did not open' });
    await (await browser.$('#library-tab-footprints')).click();
    await browser.waitUntil(
      async () =>
        browser.execute(
          (g, nick) => {
            const list = document.querySelector(`ul[aria-labelledby="library-group-${g}"]`);
            const button = list && [...list.querySelectorAll('.library-button')].find((b) => b.querySelector('.library-name')?.textContent === nick);
            button?.click();
            return Boolean(button);
          },
          group,
          library,
        ),
      { timeout: 60_000, interval: 250, timeoutMsg: `no ${group} library ${library}` },
    );
  }
  // A large library lists its items as you search.
  await (await browser.$('#library-search')).setValue(name);
  await browser.waitUntil(
    async () =>
      browser.execute((wanted) => {
        const row = [...document.querySelectorAll('#library-items [role="option"]')].find(
          (r) => r.querySelector('.library-item-name').textContent === wanted,
        );
        row?.click();
        return Boolean(row);
      }, name),
    { timeout: 60_000, interval: 250, timeoutMsg: `no row ${name}` },
  );
  await waitForText(browser, '#library-detail-title', new RegExp(`:${name}$`));
  const enlarge = await browser.$('.libraries-detail .preview .viewer-enlarge');
  await enlarge.waitForExist({ timeout: 60_000 });
  await enlarge.click();
  await browser.waitUntil(async () => isOpen(browser, LIBRARY_CLOSE_UP), { timeout: 5_000, timeoutMsg: 'Enlarge did not open the close-up' });
  await browser.waitUntil(
    async () => browser.execute((s) => document.querySelector(`${s} .viewer`)?.dataset.ready === 'true', LIBRARY_CLOSE_UP),
    { timeout: 10_000, timeoutMsg: 'the close-up never got its drawing' },
  );
  // The browser behind says how far it has read; wait until it is done.
  await browser.waitUntil(
    async () => browser.execute(() => document.querySelector('.libraries-progress')?.dataset.state === 'ready'),
    { timeout: 120_000, interval: 250, timeoutMsg: 'the Libraries browser never finished reading' },
  );
}

describe('footprint 3D view', () => {
  let browser;

  before(async () => {
    ({ browser } = await launchApp({ args: [`--kicad-config-dir=${kicadSettings()}`] }));
  });

  after(async () => {
    await browser?.deleteSession();
  });

  it("turns a footprint's close-up into a 3D view of its model", async () => {
    // Our R_0603, whose model is KiCad's own.
    await searchFor(browser, 'RC0603FR-0747RL', '1 matching part');
    await selectResult(browser, 'RC0603FR-0747RL');
    await waitForPreview(browser, FOOTPRINT);
    await (await browser.$(`${FOOTPRINT} figure button`)).click();
    await browser.waitUntil(async () => isOpen(browser, CLOSE_UP), { timeout: 5_000, timeoutMsg: 'the close-up did not open' });
    await browser.waitUntil(async () => browser.execute((s) => document.querySelector(`${s} .viewer`)?.dataset.ready === 'true', CLOSE_UP), {
      timeout: 10_000,
      timeoutMsg: 'the close-up never got its drawing',
    });
    const toggle = await browser.$(`${CLOSE_UP} .viewer-3d-toggle`);
    assert.equal(await toggle.getAttribute('aria-pressed'), 'false');
    assert.equal(await shown(browser, CLOSE_UP, '.viewer-3d-reset'), false);

    const statuses = await turn3dOn(browser, CLOSE_UP);
    record({ event: '3d-statuses', statuses });
    assert.equal(statuses[0], 'Making the 3D view…', 'it says while kicad-cli makes the view');
    assert.equal(await toggle.getAttribute('aria-pressed'), 'true');
    const tools = {};
    for (const [name, selector] of Object.entries({
      measure: '.viewer-measure',
      units: '.viewer-units',
      fit: '.viewer-fit',
      layers: '.viewer-layers:not(.viewer-side-3d)',
      drawing2d: '.viewer-area:not(.viewer-3d)',
      reset: '.viewer-3d-reset',
      top: '.viewer-3d-top',
      bottom: '.viewer-3d-bottom',
    })) {
      tools[name] = await shown(browser, CLOSE_UP, selector);
    }
    assert.deepEqual(tools, {
      measure: false,
      units: false,
      fit: false,
      layers: false,
      drawing2d: false,
      reset: true,
      top: true,
      bottom: true,
    });

    const area = await browser.execute((s) => {
      const element = document.querySelector(`${s} .viewer-3d`);
      return {
        role: element.getAttribute('role'),
        kind: element.getAttribute('aria-roledescription'),
        name: element.getAttribute('aria-label'),
        help: document.getElementById(element.getAttribute('aria-describedby'))?.textContent,
        tabIndex: element.tabIndex,
        canvasHidden: element.querySelector('canvas').getAttribute('aria-hidden'),
      };
    }, CLOSE_UP);
    assert.deepEqual(
      { ...area, help: undefined },
      {
        role: 'application',
        kind: '3D view',
        name: 'Footprint g-Resistor_SMD:R_0603_1608Metric in 3D',
        help: undefined,
        tabIndex: 0,
        canvasHidden: 'true',
      },
    );
    assert.match(area.help, /Drag to turn the model/);

    // The model's own mesh is drawn, with the board's pads.
    const drawn = await info(browser, CLOSE_UP);
    record({ event: '3d-drawn', ...drawn });
    assert.ok(drawn.meshNames.includes('R_0603_1608Metric'), `meshes: ${drawn.meshNames}`);
    assert.ok(drawn.meshNames.includes('board_pad'), `meshes: ${drawn.meshNames}`);
    assert.ok(drawn.drawn > 2_000, `${drawn.drawn} pixels drawn`);
    near(drawn.elevation, 45, 0.01, 'it starts looking down at 45 degrees');

    const said = await panel(browser, CLOSE_UP);
    record({ event: '3d-panel', ...said });
    assert.equal(said.links.length, 1);
    assert.equal(said.links[0].link, '${KICAD9_3DMODEL_DIR}/Resistor_SMD.3dshapes/R_0603_1608Metric.step');
    assert.equal(said.links[0].found, "From KiCad's own library.");
    assert.equal(said.links[0].problem, null);
    assert.match(said.links[0].path, /3dmodels[\\/]Resistor_SMD\.3dshapes[\\/]R_0603_1608Metric\.step$/);
    assert.deepEqual(said.notes, []);
    await assertAccessible(browser, '83-footprint-3d');
    await screenshotHiding3d(browser, '83-footprint-3d');
  });

  it('turns, zooms and pans with the mouse, the keyboard and the buttons', async () => {
    const area = `${CLOSE_UP} .viewer-3d`;
    const start = await info(browser, CLOSE_UP);
    // Top and Bottom look straight down and up; Reset view goes back.
    await (await browser.$(`${CLOSE_UP} .viewer-3d-top`)).click();
    let now = await info(browser, CLOSE_UP);
    near(now.elevation, 89.9, 0.01, 'Top looks straight down');
    near(now.azimuth, 0, 0.01, 'Top keeps the 2D view the right way up');
    assert.ok(now.drawn > 1_000, `${now.drawn} pixels drawn from the top`);
    await (await browser.$(`${CLOSE_UP} .viewer-3d-bottom`)).click();
    now = await info(browser, CLOSE_UP);
    near(now.elevation, -89.9, 0.01, 'Bottom looks straight up');
    near(now.azimuth, 180, 0.01, 'Bottom is mirrored, as KiCad shows it');
    await (await browser.$(`${CLOSE_UP} .viewer-3d-reset`)).click();
    assert.deepEqual(await info(browser, CLOSE_UP), start, 'Reset view goes back to the start');

    // A left drag turns the model; the wheel zooms; a right drag pans.
    const centre = await centreOf(browser, area);
    await drag(browser, centre, { x: centre.x + 60, y: centre.y + 20 }, 0);
    now = await info(browser, CLOSE_UP);
    near(now.azimuth, start.azimuth - 30, 0.01, 'a 60 pixel drag turns it 30 degrees');
    near(now.elevation, start.elevation + 10, 0.01, 'a 20 pixel drag tilts it 10 degrees');
    await browser.action('wheel').scroll({ x: centre.x + 50, y: centre.y, deltaX: 0, deltaY: -300, duration: 0 }).perform();
    const zoomed = await info(browser, CLOSE_UP);
    assert.ok(zoomed.distance < now.distance * 0.7, `the wheel zoomed in: ${now.distance} to ${zoomed.distance}`);
    assert.notDeepEqual(zoomed.target, now.target, 'it zoomed at the pointer, not the centre');
    await drag(browser, centre, { x: centre.x - 40, y: centre.y }, 2);
    const panned = await info(browser, CLOSE_UP);
    assert.notDeepEqual(panned.target, zoomed.target, 'a right drag pans');
    near(panned.azimuth, zoomed.azimuth, 0.001, 'a right drag does not turn');
    await drag(browser, centre, { x: centre.x, y: centre.y + 40 }, 1);
    assert.notDeepEqual((await info(browser, CLOSE_UP)).target, panned.target, 'a middle drag pans');
    assert.equal(await isOpen(browser, CLOSE_UP), true, 'a drag does not close the close-up');

    // The keyboard, with the view focused.
    await browser.execute((s) => document.querySelector(s).focus(), area);
    await browser.keys('0');
    assert.deepEqual(await info(browser, CLOSE_UP), start, '0 shows the starting view');
    await browser.keys('ArrowRight');
    near((await info(browser, CLOSE_UP)).azimuth, start.azimuth - 10, 0.01, 'the right arrow turns it');
    await browser.keys('ArrowUp');
    near((await info(browser, CLOSE_UP)).elevation, start.elevation + 10, 0.01, 'the up arrow tilts it');
    await browser.keys('+');
    near((await info(browser, CLOSE_UP)).distance, start.distance / 1.5, start.distance * 1e-6, 'plus zooms in');
    await browser.keys('-');
    near((await info(browser, CLOSE_UP)).distance, start.distance, start.distance * 1e-6, 'minus zooms out');
    const before = await info(browser, CLOSE_UP);
    await browser.keys(['Shift', 'ArrowLeft', 'Shift']);
    const shifted = await info(browser, CLOSE_UP);
    assert.notDeepEqual(shifted.target, before.target, 'Shift and an arrow pan');
    near(shifted.azimuth, before.azimuth, 0.001, 'Shift and an arrow do not turn');
    await browser.keys('0');
    assert.deepEqual(await info(browser, CLOSE_UP), start);
  });

  it('turns 3D off, frees the view, and makes it again from the cache', async () => {
    const toggle = await browser.$(`${CLOSE_UP} .viewer-3d-toggle`);
    await toggle.click();
    assert.equal(await toggle.getAttribute('aria-pressed'), 'false');
    assert.equal(await shown(browser, CLOSE_UP, '.viewer-area:not(.viewer-3d) svg'), true, 'the 2D drawing is back');
    assert.equal(await shown(browser, CLOSE_UP, '.viewer-layers'), true, 'the layer list is back');
    assert.equal(await shown(browser, CLOSE_UP, '.viewer-measure'), true, 'Measure is back');
    assert.equal(await browser.execute(() => document.querySelectorAll('canvas').length), 0, 'the 3D canvas is gone');

    // A measurement stays while the 3D view is shown.
    await (await browser.$(`${CLOSE_UP} .viewer-measure`)).click();
    const pads = await browser.execute((s) =>
      [...document.querySelectorAll(`${s} g.layer[data-layer="F.Cu"] path`)].slice(0, 2).map((pad) => {
        const rect = pad.getBoundingClientRect();
        return { x: Math.round(rect.left + rect.width / 2), y: Math.round(rect.top + rect.height / 2) };
      }), CLOSE_UP);
    for (const pad of pads) {
      await browser.action('pointer', { parameters: { pointerType: 'mouse' } }).move(pad).down({ button: 0 }).up({ button: 0 }).perform();
    }
    const readout = () => browser.execute((s) => [...document.querySelectorAll(`${s} .viewer-readout p`)].map((line) => line.textContent), CLOSE_UP);
    const measured = await readout();
    assert.equal(measured[0], 'Distance 1.650 mm, \u0394X 1.650 mm, \u0394Y 0.000 mm.');

    const again = await turn3dOn(browser, CLOSE_UP);
    record({ event: '3d-again', statuses: again });
    assert.ok((await info(browser, CLOSE_UP)).meshNames.includes('R_0603_1608Metric'));
    // If the browser takes WebGL away, the view says so.
    await browser.execute((s) => {
      const canvas = document.querySelector(`${s} .view3d-canvas`);
      (canvas.getContext('webgl2') ?? canvas.getContext('webgl')).getExtension('WEBGL_lose_context').loseContext();
    }, CLOSE_UP);
    await waitForText(browser, `${CLOSE_UP} .view3d-status`, 'The 3D view stopped. Turn 3D off and on again.');
    await toggle.click();
    assert.deepEqual(await readout(), measured, 'the measurement is still there');
    assert.equal(await (await browser.$(`${CLOSE_UP} .viewer-measure`)).getAttribute('aria-pressed'), 'true');
    await turn3dOn(browser, CLOSE_UP);
    // Escape closes the close-up, and the 3D view with it.
    await browser.keys('Escape');
    await browser.waitUntil(async () => !(await isOpen(browser, CLOSE_UP)), { timeout: 5_000, timeoutMsg: 'Escape did not close the close-up' });
    assert.equal(await browser.execute(() => document.querySelectorAll('canvas').length), 0, 'closing frees the 3D canvas');
  });

  it('frees a 3D view whose drawing fails partway, and says so', async () => {
    await (await browser.$(`${FOOTPRINT} figure button`)).click();
    await browser.waitUntil(async () => isOpen(browser, CLOSE_UP), { timeout: 5_000, timeoutMsg: 'the close-up did not open' });
    // The view's size watcher fails, after its canvas and WebGL context
    // were made.
    await browser.execute(() => {
      window.journeyResizeObserver = window.ResizeObserver;
      window.ResizeObserver = function failing() {
        throw new Error('no size watcher');
      };
    });
    try {
      await (await browser.$(`${CLOSE_UP} .viewer-3d-toggle`)).click();
      await browser.waitUntil(
        async () => browser.execute((s) => document.querySelector(`${s} .viewer-3d`)?.dataset.state === 'problem', CLOSE_UP),
        { timeout: 60_000, interval: 100, timeoutMsg: 'the 3D view never said it failed' },
      );
      const left = await browser.execute((s) => ({
        status: document.querySelector(`${s} .view3d-status`)?.textContent,
        canvases: document.querySelectorAll('canvas').length,
        info: typeof document.querySelector(`${s} .viewer-3d`).view3dInfo,
      }), CLOSE_UP);
      assert.deepEqual(left, {
        status: 'The 3D view could not be made: no size watcher.',
        canvases: 0,
        info: 'undefined',
      });
    } finally {
      await browser.execute(() => {
        window.ResizeObserver = window.journeyResizeObserver;
      });
    }
    await browser.keys('Escape');
    await browser.waitUntil(async () => !(await isOpen(browser, CLOSE_UP)), { timeout: 5_000, timeoutMsg: 'Escape did not close the close-up' });
  });

  it('says when this computer cannot show 3D views, and keeps the 2D view', async () => {
    await (await browser.$(`${FOOTPRINT} figure button`)).click();
    await browser.waitUntil(async () => isOpen(browser, CLOSE_UP), { timeout: 5_000, timeoutMsg: 'the close-up did not open' });
    // WebGL off, as on a computer without it.
    await browser.execute(() => {
      window.journeyGetContext = HTMLCanvasElement.prototype.getContext;
      HTMLCanvasElement.prototype.getContext = function getContext(kind, ...rest) {
        return kind.startsWith('webgl') ? null : window.journeyGetContext.call(this, kind, ...rest);
      };
    });
    try {
      const toggle = await browser.$(`${CLOSE_UP} .viewer-3d-toggle`);
      await toggle.click();
      const said = await browser.execute((s) => {
        const dialog = document.querySelector(s);
        return {
          message: dialog.querySelector('.viewer-problem')?.textContent ?? null,
          drawing: dialog.querySelector('.viewer-area:not(.viewer-3d) svg')?.getClientRects().length > 0,
          view3d: dialog.querySelector('.viewer-3d') !== null,
        };
      }, CLOSE_UP);
      assert.deepEqual(said, { message: "This computer can't show 3D views (WebGL is off).", drawing: true, view3d: false });
      assert.equal(await toggle.getAttribute('aria-pressed'), 'false');
      await assertAccessible(browser, '86-footprint-3d-no-webgl');
      await screenshot(browser, '86-footprint-3d-no-webgl');
    } finally {
      await browser.execute(() => {
        HTMLCanvasElement.prototype.getContext = window.journeyGetContext;
      });
    }
    await browser.keys('Escape');
    await browser.waitUntil(async () => !(await isOpen(browser, CLOSE_UP)), { timeout: 5_000, timeoutMsg: 'Escape did not close the close-up' });
  });

  it("says where it looked for a part's missing 3D model", async () => {
    // Our Abracon_ABS07L links ${GITPLM_3DMODELS}/ABS07L-32.768KHZ-T.step,
    // which our library does not have.
    await searchFor(browser, 'ABS07L-32.768KHZ-T', '1 matching part');
    await selectResult(browser, 'ABS07L-32.768KHZ-T');
    await waitForPreview(browser, FOOTPRINT);
    await (await browser.$(`${FOOTPRINT} figure button`)).click();
    await browser.waitUntil(async () => isOpen(browser, CLOSE_UP), { timeout: 5_000, timeoutMsg: 'the close-up did not open' });
    await browser.waitUntil(async () => browser.execute((s) => document.querySelector(`${s} .viewer`)?.dataset.ready === 'true', CLOSE_UP), {
      timeout: 10_000,
      timeoutMsg: 'the close-up never got its drawing',
    });
    await turn3dOn(browser, CLOSE_UP);
    const said = await panel(browser, CLOSE_UP);
    record({ event: '3d-part-missing-model', ...said });
    assert.deepEqual(
      said.links.map(({ link, found, problem }) => ({ link, found, problem })),
      [{ link: '${GITPLM_3DMODELS}/ABS07L-32.768KHZ-T.step', found: null, problem: 'The file is missing.' }],
    );
    // Outside a footprint library's folder, saving would not keep it.
    assert.match(
      await browser.execute((s) => document.querySelector(`${s} .model-link-note`)?.textContent ?? '', CLOSE_UP),
      /^Edit part would save this link as missing/,
    );
    assert.match(said.links[0].path, /[\\/]3d-models[\\/]ABS07L-32\.768KHZ-T\.step$/);
    assert.deepEqual(said.notes, ['No 3D model can be shown, so only the board with its pads, silkscreen and mask is shown.']);
    await assertAccessible(browser, '85-footprint-3d-missing-model');
    await screenshotHiding3d(browser, '85-footprint-3d-missing-model');
    await browser.keys('Escape');
    await browser.waitUntil(async () => !(await isOpen(browser, CLOSE_UP)), { timeout: 5_000, timeoutMsg: 'Escape did not close the close-up' });
  });

  it('shows the board alone for a footprint with no 3D model', async () => {
    await openFixture(browser, 'No_Model_Test');
    await turn3dOn(browser, LIBRARY_CLOSE_UP);
    const said = await panel(browser, LIBRARY_CLOSE_UP);
    record({ event: '3d-no-model', ...said });
    assert.deepEqual(said, {
      links: [],
      notes: ['The footprint has no 3D model, so only the board with its pads, silkscreen and mask is shown.'],
    });
    const drawn = await info(browser, LIBRARY_CLOSE_UP);
    assert.ok(drawn.meshNames.length > 0 && drawn.meshNames.every((name) => name.startsWith('board_')), `meshes: ${drawn.meshNames}`);
    assert.ok(drawn.drawn > 2_000, `${drawn.drawn} pixels drawn`);
    // The silkscreen (REF** and a line) shows in front of the mask, white
    // as KiCad draws it, from the top and at the starting angle.
    assert.ok(drawn.silkscreen > 300, `${drawn.silkscreen} silkscreen pixels at the start`);
    assert.ok(drawn.silkscreenLightness > 185, `the silkscreen is white, not ${drawn.silkscreenLightness} of 255`);
    await (await browser.$(`${LIBRARY_CLOSE_UP} .viewer-3d-top`)).click();
    const top = await info(browser, LIBRARY_CLOSE_UP);
    assert.ok(top.silkscreen > 300, `${top.silkscreen} silkscreen pixels from the top`);
    // The back's silkscreen, a filled bar, through the back's mask.
    await (await browser.$(`${LIBRARY_CLOSE_UP} .viewer-3d-bottom`)).click();
    const bottom = await info(browser, LIBRARY_CLOSE_UP);
    record({
      event: '3d-silkscreen',
      start: drawn.silkscreen,
      top: top.silkscreen,
      bottom: bottom.silkscreen,
      lightness: [drawn.silkscreenLightness, top.silkscreenLightness, bottom.silkscreenLightness],
    });
    assert.ok(bottom.silkscreenLightness > 185, `the back's silkscreen is white, not ${bottom.silkscreenLightness} of 255`);
    assert.ok(bottom.silkscreen > 300, `${bottom.silkscreen} silkscreen pixels from the bottom`);
    await (await browser.$(`${LIBRARY_CLOSE_UP} .viewer-3d-reset`)).click();
    await assertAccessible(browser, '84-footprint-3d-no-model');
    await screenshotHiding3d(browser, '84-footprint-3d-no-model');
    await browser.keys('Escape');
    await browser.waitUntil(async () => !(await isOpen(browser, LIBRARY_CLOSE_UP)), { timeout: 5_000, timeoutMsg: 'Escape did not close the close-up' });
  });

  it('says why each 3D model of a footprint cannot be shown', async () => {
    await openFixture(browser, 'Missing_Model_Test');
    await turn3dOn(browser, LIBRARY_CLOSE_UP);
    const said = await panel(browser, LIBRARY_CLOSE_UP);
    record({ event: '3d-missing-model', ...said });
    assert.deepEqual(
      said.links.map(({ link, found, problem }) => ({ link, found, problem })),
      [
        {
          link: '${KICAD10_3DMODEL_DIR}/Journey.3dshapes/Missing_Model_Test.step',
          found: null,
          problem: 'The file is missing.',
        },
        {
          link: '${JOURNEY_3D_MODELS}/Missing_Model_Test.step',
          found: null,
          problem: "The path variable ${JOURNEY_3D_MODELS} is not set in KiCad's path settings.",
        },
      ],
    );
    // Where the missing file was looked for.
    assert.match(said.links[0].path, /3dmodels[\\/]Journey\.3dshapes[\\/]Missing_Model_Test\.step$/);
    assert.equal(said.links[1].path, null);
    assert.deepEqual(said.notes, ['No 3D model can be shown, so only the board with its pads, silkscreen and mask is shown.']);
    const drawn = await info(browser, LIBRARY_CLOSE_UP);
    assert.ok(drawn.meshNames.every((name) => name.startsWith('board_')), `meshes: ${drawn.meshNames}`);
    // axe cannot tell the colours of a note whose lines lie over the
    // Libraries browser, another modal dialog under this close-up
    // (mx-d2fb83). Only that rule is off; the same panel's contrast is
    // checked over the part details above.
    await assertAccessible(browser, '87-footprint-3d-models-not-found', { disabledRules: ['color-contrast'] });
    await screenshotHiding3d(browser, '87-footprint-3d-models-not-found');
    await browser.keys('Escape');
    await browser.waitUntil(async () => !(await isOpen(browser, LIBRARY_CLOSE_UP)), { timeout: 5_000, timeoutMsg: 'Escape did not close the close-up' });
    await browser.keys('Escape');
    await browser.waitUntil(async () => !(await isOpen(browser, LIBRARIES)), { timeout: 5_000, timeoutMsg: 'Escape did not close the Libraries browser' });
  });

  it("shows the silkscreen from low down, next to a tall part", async () => {
    // KiCad's own 2-pin header: its pins reach far above the board, so the
    // middle of the scene is too.
    await openFixture(browser, 'PinHeader_1x02_P2.54mm_Vertical', {
      group: 'kicad',
      library: 'Connector_PinHeader_2.54mm',
    });
    await turn3dOn(browser, LIBRARY_CLOSE_UP);
    // Lower the view until the camera is below the scene's middle, still
    // above the board.
    await browser.execute((s) => document.querySelector(`${s} .viewer-3d`).focus(), LIBRARY_CLOSE_UP);
    // Four steps down to 5 degrees, then a 15 pixel drag up to about -2.5.
    await browser.keys(['ArrowDown', 'ArrowDown', 'ArrowDown', 'ArrowDown']);
    const centre = await centreOf(browser, `${LIBRARY_CLOSE_UP} .viewer-3d`);
    await drag(browser, centre, { x: centre.x, y: centre.y - 15 }, 0);
    const low = await info(browser, LIBRARY_CLOSE_UP);
    record({ event: '3d-low-silkscreen', ...low, meshNames: undefined });
    assert.ok(low.sceneMiddle - low.boardTop > 0.002, 'the pins lift the scene well above the board');
    assert.ok(
      low.cameraHeight < low.sceneMiddle && low.cameraHeight > low.boardTop,
      `the camera at ${low.cameraHeight} is between the board top ${low.boardTop} and the scene middle ${low.sceneMiddle}`,
    );
    assert.ok(low.silkscreen > 100, `${low.silkscreen} silkscreen pixels from low down`);
    await capture(browser, path.join(screenshotsDir, '93-footprint-3d-low-drawn.png'), PATHS);
    await browser.keys('Escape');
    await browser.waitUntil(async () => !(await isOpen(browser, LIBRARY_CLOSE_UP)), { timeout: 5_000, timeoutMsg: 'Escape did not close the close-up' });
    await browser.keys('Escape');
    await browser.waitUntil(async () => !(await isOpen(browser, LIBRARIES)), { timeout: 5_000, timeoutMsg: 'Escape did not close the Libraries browser' });
  });
});
