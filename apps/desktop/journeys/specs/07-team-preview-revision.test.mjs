// Team catalog previews come from the exact CAD item revision: the app
// reads a temporary copy of the sample library in which someone has since
// redrawn the R_US symbol and the 0603 footprint, while the team catalog
// stores the original bytes. Signed in, the part shows the stored revision.
// When the stored bytes cannot be fetched, the local library is drawn with
// a note saying it differs. (Screenshot 50 showed a CSV catalog part while
// signed out; without the CSV catalog there is no such part, so it went.)

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
  sampleLibrary,
  screenshot,
  searchFor,
  selectResult,
  startTeamServer,
  tempDir,
  textOf,
  waitForLiveUpdates,
  waitForPreview,
  waitForText,
} from '../harness.mjs';

const MPN = 'RC0603FR-0747RL';
const BADGE = '.team-settings > summary';

/** A copy of the sample library with R_US and R_0603_1608Metric redrawn. */
function changedCatalog() {
  const root = tempDir('changed-catalog');
  for (const folder of ['symbols', 'footprints']) {
    fs.cpSync(path.join(sampleLibrary, folder), path.join(root, folder), { recursive: true });
  }
  const edit = (file, from, to) => {
    const text = fs.readFileSync(file, 'utf8');
    const changed = text.replace(from, to);
    assert.notEqual(changed, text, `${file} was changed`);
    fs.writeFileSync(file, changed);
  };
  edit(
    path.join(root, 'symbols', 'g-res.kicad_sym'),
    '(symbol "R_US_0_1"',
    '(symbol "R_US_0_1"\n\t\t\t(circle (center 0 0) (radius 5) (stroke (width 0.5) (type default)) (fill (type none)))',
  );
  edit(
    path.join(root, 'footprints', 'g-Resistor_SMD.pretty', 'R_0603_1608Metric.kicad_mod'),
    '\t(fp_line',
    '\t(fp_circle (center 0 0) (end 2 0) (stroke (width 0.3) (type solid)) (fill no) (layer "F.SilkS"))\n\t(fp_line',
  );
  return root;
}

/** The SVG in an image's data URI, without the title that holds the export time. */
function drawing(src) {
  const svg = Buffer.from(src.slice(src.indexOf(',') + 1), 'base64').toString('utf8');
  return svg.replace(/<title>[^<]*<\/title>/, '');
}

describe('team catalog previews: drawn from the exact CAD item revision', () => {
  let server;
  let browser;
  let dataDir;
  let account;
  const stored = {};

  before(async () => {
    server = await startTeamServer();
    [account] = server.accounts;
    ({ browser, dataDir } = await launchApp({
      args: [
        `--catalog-root=${changedCatalog()}`,
        `--team-server=${server.url}`,
        `--team-email=${account.email}`,
      ],
    }));
  });

  after(async () => {
    await browser?.deleteSession();
    await server?.stop();
  });

  async function preview(section) {
    return browser.execute(
      (s) => {
        const el = document.querySelector(s);
        return {
          provenance: el.querySelector('.provenance')?.textContent ?? null,
          title: el.querySelector('.provenance')?.getAttribute('title') ?? null,
          src: el.querySelector('img')?.getAttribute('src') ?? null,
          note: el.querySelector('.preview-note')?.textContent ?? null,
        };
      },
      section,
    );
  }

  async function sourceFact() {
    return browser.execute(() => {
      const fact = [...document.querySelectorAll('.fact')].find((f) => f.querySelector('dt')?.textContent === 'Source');
      return fact?.querySelector('dd')?.textContent ?? null;
    });
  }

  it('signed in, draws the stored revision, not the changed local file', async () => {
    // Without a team, the sample library has no parts to list.
    await waitForText(browser, '#result-count', 'No parts match.');
    await waitForText(browser, BADGE, 'Team catalog: not signed in');
    await browser.execute(() => document.querySelector('.team-settings').open = true);
    await (await browser.$('aria/Password')).setValue(account.password);
    await (await browser.$('form[aria-label="Sign in to the team catalog"] button[type="submit"]')).click();
    await waitForText(browser, BADGE, `Team catalog · ${account.name}`);
    await waitForLiveUpdates(browser);
    await browser.execute(() => document.querySelector('.team-settings').open = false);
    await searchFor(browser, MPN, '1 matching part');
    await selectResult(browser, MPN);
    await browser.waitUntil(async () => (await sourceFact()) === 'Team catalog, revision 1 of this part', {
      timeout: 20_000,
      timeoutMsg: 'the part never showed the team catalog revision',
    });
    for (const [name, section] of [['symbol', SYMBOL], ['footprint', FOOTPRINT]]) {
      await waitForText(browser, `${section} .provenance`, /^team catalog, (symbol|footprint) revision 1 · kicad-cli 10\./, 60_000);
      await waitForPreview(browser, section);
      const shown = await preview(section);
      record({ event: 'team-preview', name, provenance: shown.provenance, title: shown.title });
      assert.match(shown.title, /^Stored bytes with SHA-256 [0-9a-f]{64}$/, name);
      assert.equal(shown.note, null, name);
      stored[name] = drawing(shown.src);
    }
    assert.match(await textOf(browser, '.part-fields'), /g-res:R_US, symbol revision 1/);
    assert.equal(await browser.execute(() => document.querySelectorAll('[role="alert"]').length), 0);
    await assertAccessible(browser, '51-team-preview-stored-revision');
    await screenshot(browser, '51-team-preview-stored-revision');
  });

  it('says so when the stored revision cannot be fetched', async () => {
    await server.stop();
    server = null;
    // Forget the drawings, so the next view needs the stored bytes again.
    fs.rmSync(path.join(dataDir, 'cache', 'previews', 'v2'), { recursive: true, force: true });
    await browser.execute(() => document.querySelector('.kicad-settings').open = true);
    const buttons = await browser.$$('.kicad-settings-body button');
    for (const button of buttons) {
      if ((await button.getText()) === 'Search again') {
        await button.click();
        break;
      }
    }
    for (const [section, what, reference] of [
      [SYMBOL, 'Symbol', 'g-res:R_US'],
      [FOOTPRINT, 'Footprint', 'g-Resistor_SMD:R_0603_1608Metric'],
    ]) {
      await waitForText(
        browser,
        `${section} .preview-note`,
        `${what} revision 1 of ${reference} could not be fetched because the team server could not be reached. ` +
          'This drawing is from your local library, which differs from that revision.',
        60_000,
      );
      await waitForPreview(browser, section);
      // The reference in the note keeps its library part whole; its text,
      // checked above, is still exactly the reference.
      assert.equal(await textOf(browser, `${section} .preview-note .lib`), `${reference.split(':')[0]}:`);
    }
    const fits = await browser.execute(() => {
      const previews = document.querySelector('.part-previews');
      return previews.scrollWidth <= previews.clientWidth + 1;
    });
    assert.equal(fits, true, 'the note fits the column without sideways scrolling');
    // The changed local file, not the stored revision.
    for (const [name, section] of [['symbol', SYMBOL], ['footprint', FOOTPRINT]]) {
      const shown = await preview(section);
      assert.match(shown.provenance, /^project library · /, name);
      assert.notEqual(drawing(shown.src), stored[name], `the ${name} is the changed local file`);
    }
    // A note, not an error: the drawing is still useful.
    assert.equal(await browser.execute(() => document.querySelectorAll('.part-previews [role="alert"]').length), 0);
    await browser.execute(() => document.querySelector('.kicad-settings').open = false);
    // The badge says live updates are reconnecting, since the server is gone.
    await browser.waitUntil(
      async () => browser.execute((s) => document.querySelector(s)?.classList.contains('kicad-badge-problem'), BADGE),
      { timeout: 20_000, timeoutMsg: 'the team badge never showed the lost connection' },
    );
    await assertAccessible(browser, '52-team-preview-fallback');
    await screenshot(browser, '52-team-preview-fallback');
  });
});
