// A team part's 3D view (kicad-parts-c9c1) draws the 3D model the Catalog
// stores for the exact footprint revision, after the same checks previews
// use. The app reads a copy of the sample library without that model file,
// so only the Catalog's stored copy can supply it. The view stays steady with
// this model too (kicad-parts-4588).

import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { after, before, describe, it } from 'node:test';
import {
  FOOTPRINT,
  capture,
  launchApp,
  record,
  sampleLibrary,
  screenshotsDir,
  searchFor,
  selectResult,
  startTeamServer,
  tempDir,
  waitForLiveUpdates,
  waitForPreview,
  waitForText,
} from '../harness.mjs';
import { checkSteady3d } from '../view3d-steady.mjs';

const MPN = 'SN74LVC1G07DBVR';
const BADGE = '.team-settings > summary';
const CLOSE_UP = `${FOOTPRINT} dialog.zoom-dialog`;
const MODEL = path.join('3d-models', 'g-Package_TO_SOT_SMD.3dshapes', 'SOT-23-5.wrl');

/** A copy of the sample library without the SOT-23-5 3D model file. */
function catalogWithoutModel() {
  const root = tempDir('catalog-without-model');
  for (const folder of ['symbols', 'footprints', '3d-models']) {
    fs.cpSync(path.join(sampleLibrary, folder), path.join(root, folder), { recursive: true });
  }
  fs.rmSync(path.join(root, MODEL));
  return root;
}

async function isOpen(browser, dialog) {
  return browser.execute((s) => document.querySelector(s)?.open === true, dialog);
}

describe("team part 3D view: the Catalog's stored 3D model", () => {
  let server;
  let browser;
  let dataDir;
  let account;

  before(async () => {
    server = await startTeamServer();
    [account] = server.accounts;
    ({ browser, dataDir } = await launchApp({
      args: [`--catalog-root=${catalogWithoutModel()}`, `--team-server=${server.url}`, `--team-email=${account.email}`],
    }));
  });

  after(async () => {
    await browser?.deleteSession();
    await server?.stop();
  });

  it("draws the model the Catalog stores for the footprint's revision", async () => {
    await browser.execute(() => {
      document.querySelector('.team-settings').open = true;
    });
    await (await browser.$('aria/Password')).setValue(account.password);
    await (await browser.$('form[aria-label="Sign in to the team catalog"] button[type="submit"]')).click();
    await waitForText(browser, BADGE, `Team catalog · ${account.name}`);
    await waitForLiveUpdates(browser);
    await searchFor(browser, MPN, '1 matching part');
    await selectResult(browser, MPN);
    await waitForText(browser, `${FOOTPRINT} .provenance`, /^team catalog, footprint revision 1 · kicad-cli 10\./, 60_000);
    await waitForPreview(browser, FOOTPRINT);

    await (await browser.$(`${FOOTPRINT} figure button`)).click();
    await browser.waitUntil(async () => isOpen(browser, CLOSE_UP), { timeout: 5_000, timeoutMsg: 'the close-up did not open' });
    await browser.waitUntil(async () => browser.execute((s) => document.querySelector(`${s} .viewer`)?.dataset.ready === 'true', CLOSE_UP), {
      timeout: 10_000,
      timeoutMsg: 'the close-up never got its drawing',
    });
    await (await browser.$(`${CLOSE_UP} .viewer-3d-toggle`)).click();
    await browser.waitUntil(
      async () => browser.execute((s) => ['ready', 'problem'].includes(document.querySelector(`${s} .viewer-3d`)?.dataset.state), CLOSE_UP),
      { timeout: 90_000, interval: 100, timeoutMsg: 'the 3D view was never drawn' },
    );
    const shown = await browser.execute((s) => {
      const dialog = document.querySelector(s);
      const area = dialog.querySelector('.viewer-3d');
      return {
        state: area.dataset.state,
        status: area.textContent,
        meshNames: area.dataset.state === 'ready' ? area.view3dInfo().meshNames : [],
        links: [...dialog.querySelectorAll('.model-link')].map((link) => ({
          link: link.querySelector('.model-link-text').textContent,
          found: link.querySelector('.model-found')?.textContent ?? null,
          problem: link.querySelector('.model-problem')?.textContent ?? null,
          path: link.querySelector('.model-path-text')?.textContent ?? null,
          key: link.querySelector('.model-key')?.textContent ?? null,
        })),
      };
    }, CLOSE_UP);
    record({ event: '3d-stored-model', ...shown });
    assert.equal(shown.state, 'ready', shown.status);
    assert.equal(shown.links.length, 1);
    assert.equal(shown.links[0].link, '${GITPLM_PARTS}/3d-models/g-Package_TO_SOT_SMD.3dshapes/SOT-23-5.wrl');
    assert.equal(shown.links[0].found, "From the Catalog's stored model for this revision.");
    assert.equal(shown.links[0].problem, null);
    // Named by where the Catalog keeps it.
    assert.equal(shown.links[0].key, '3d-models/g-Package_TO_SOT_SMD.3dshapes/SOT-23-5.wrl');
    // The checked copy in the app's own cache, with the stored bytes.
    const copy = shown.links[0].path;
    assert.ok(copy.startsWith(dataDir), `${copy} is in the app's cache`);
    assert.deepEqual(fs.readFileSync(copy), fs.readFileSync(path.join(sampleLibrary, MODEL)));
    assert.ok(
      shown.meshNames.some((name) => !name.startsWith('board_')),
      `the model's own mesh is drawn: ${shown.meshNames}`,
    );
    // For people to look at: the model over its silkscreen; not compared.
    await capture(browser, path.join(screenshotsDir, '96-team-footprint-3d-drawn.png'), ['.model-path-text']);
    // The SOT-23-5 turns around its centre, and the view can't be lost.
    await checkSteady3d(browser, `${CLOSE_UP} .viewer-3d`, async () => (await browser.$(`${CLOSE_UP} .viewer-3d-reset`)).click());
    await browser.keys('Escape');
    await browser.waitUntil(async () => !(await isOpen(browser, CLOSE_UP)), { timeout: 5_000, timeoutMsg: 'Escape did not close the close-up' });
  });
});
