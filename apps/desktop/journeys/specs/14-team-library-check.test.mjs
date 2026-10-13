// The library check (kicad-parts-da65): the app reads a copy of the CSV
// catalog in which someone drew a footprint by hand in KiCad (no part uses
// it yet) and changed a footprint the Catalog has to link a 3D model on
// their own drive. Signed in as someone who can edit, a short line under
// the search box says so; it opens the Library check view, grouped as To
// fix, Not in the Catalog yet, Worth a look and Clean-up. Choosing a part
// closes it; someone who can only view sees no line.

import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { after, before, describe, it } from 'node:test';
import {
  assertAccessible,
  launchApp,
  record,
  repoRoot,
  screenshot,
  searchFor,
  selectResult,
  startTeamServer,
  tempDir,
  textOf,
  waitForLibraryCheck,
  waitForLiveUpdates,
  waitForText,
} from '../harness.mjs';

const BADGE = '.team-settings > summary';
const LINE = '#library-check-open';
const R0603 = 'footprints/g-Resistor_SMD.pretty/R_0603_1608Metric.kicad_mod';
const KICAD_MODEL = '${KICAD9_3DMODEL_DIR}/Resistor_SMD.3dshapes/R_0603_1608Metric.step';
const OWN_DRIVE = 'C:/Users/me/models/R_0603.step';

/** A copy of the CSV catalog's folders, with the two changes made in KiCad. */
function catalogCopy() {
  const copy = tempDir('catalog');
  for (const folder of ['database', 'symbols', 'footprints', '3d-models']) {
    fs.cpSync(path.join(repoRoot, folder), path.join(copy, folder), { recursive: true });
  }
  const r0603 = path.join(copy, R0603);
  const text = fs.readFileSync(r0603, 'utf8');
  assert.ok(text.includes(KICAD_MODEL));
  fs.writeFileSync(r0603, text.replace(KICAD_MODEL, OWN_DRIVE));
  const hand = fs
    .readFileSync(path.join(copy, 'footprints/g-Capacitor_SMD.pretty/C_0603_1608Metric.kicad_mod'), 'utf8')
    .replace('(footprint "C_0603_1608Metric"', '(footprint "C_0603_Hand"')
    .replace(
      '${KICAD9_3DMODEL_DIR}/Capacitor_SMD.3dshapes/C_0603_1608Metric.step',
      '${GITPLM_PARTS}/3d-models/g-Capacitor_SMD.3dshapes/C_0603_Hand.step',
    );
  fs.writeFileSync(path.join(copy, 'footprints/g-Capacitor_SMD.pretty/C_0603_Hand.kicad_mod'), hand);
  fs.mkdirSync(path.join(copy, '3d-models/g-Capacitor_SMD.3dshapes'), { recursive: true });
  fs.writeFileSync(
    path.join(copy, '3d-models/g-Capacitor_SMD.3dshapes/C_0603_Hand.step'),
    'ISO-10303-21;\nEND-ISO-10303-21;\n',
  );
  return copy;
}

describe('team catalog: the library check', () => {
  let server;
  let browser;
  let editor;
  let viewer;

  before(async () => {
    server = await startTeamServer({ csv: ['g-res'] });
    [editor, , viewer] = server.accounts;
    ({ browser } = await launchApp({
      args: [`--team-server=${server.url}`, `--team-email=${editor.email}`, `--catalog-root=${catalogCopy()}`],
    }));
  });

  after(async () => {
    await browser?.deleteSession();
    await server?.stop();
  });

  async function signIn(account) {
    const open = await browser.execute(() => document.querySelector('.team-settings')?.open === true);
    if (!open) await (await browser.$(BADGE)).click();
    await (await browser.$('aria/Email')).setValue(account.email);
    await (await browser.$('aria/Password')).setValue(account.password);
    await (await browser.$('form[aria-label="Sign in to the team catalog"] button[type="submit"]')).click();
    await waitForText(browser, BADGE, `Team catalog · ${account.name}`);
    await waitForLiveUpdates(browser);
    await browser.execute(() => {
      document.querySelector('.team-settings').open = false;
    });
  }

  async function headings() {
    return browser.execute(() =>
      [...document.querySelectorAll('.library-check-group h3')].map((h) => h.textContent.trim()),
    );
  }

  async function focused() {
    return browser.execute(() => document.activeElement?.id ?? null);
  }

  it('shows nothing until someone who can edit signs in', async () => {
    await waitForText(browser, '#result-count', '178 matching parts');
    assert.equal((await browser.$$(LINE)).length, 0);
    assert.equal(await browser.execute(() => document.querySelector('.search-pane').dataset.libraryCheck), 'off');
  });

  it('says under the search box what to fix and what is new', async () => {
    await signIn(editor);
    // R_0603_1608Metric changed since the resistors were imported; the
    // hand-drawn footprint joins the symbols and footprints no part uses.
    await waitForText(browser, LINE, /^Library check: 1 to fix, \d+ new items$/);
    record({ event: 'library-check-line', text: await textOf(browser, LINE) });
  });

  it('opens the Library check view, grouped, with each item plainly', async () => {
    await (await browser.$(LINE)).click();
    await waitForText(browser, '#library-check-title', 'Library check');
    await browser.waitUntil(async () => (await focused()) === 'library-check-title', {
      timeout: 5_000,
      timeoutMsg: 'focus did not move to the view',
    });
    const groups = await headings();
    assert.equal(groups.length, 4, `${groups}`);
    assert.equal(groups[0], 'To fix (1)');
    assert.match(groups[1], /^Not in the Catalog yet \(\d+\)$/);
    assert.match(groups[2], /^Worth a look \(\d+\)$/);
    assert.match(groups[3], /^Clean-up \(\d+\)$/);
    const toFix = await textOf(browser, '.library-check-group.to-fix .library-check-item');
    assert.match(toFix, /g-Resistor_SMD:R_0603_1608Metric/);
    assert.match(toFix, /footprints\/g-Resistor_SMD\.pretty\/R_0603_1608Metric\.kicad_mod/);
    assert.match(toFix, /C:\/Users\/me\/models\/R_0603\.step, which is outside our library/);
    assert.match(toFix, /The footprint changed since the last import\.$/);
    const newItems = await browser.execute(() =>
      [...document.querySelectorAll('.library-check-group:nth-of-type(2) .library-check-name')].map((p) =>
        p.textContent.trim(),
      ),
    );
    assert.ok(newItems.includes('g-Capacitor_SMD:C_0603_Hand'), `${newItems}`);
    // Today's g-ics.csv names g-ics:TP4333, while the library has IC_TP4333.
    assert.match(await textOf(browser, '.library-check-body'), /Did you mean g-ics:IC_TP4333\?/);
    // Clean-up starts folded: those old problems stop nothing.
    assert.equal(await browser.execute(() => document.querySelector('details.library-check-group').open), false);
    // Only symbols and footprints not in the Catalog yet can start Add part
    // (kicad-parts-37a7); journey 15 does.
    const adds = await browser.execute(() =>
      [...document.querySelectorAll('.library-check-body button')].map((button) => [
        button.firstChild.textContent.trim(),
        button.closest('.library-check-group').querySelector('h3').textContent.trim(),
      ]),
    );
    assert.ok(adds.length > 0);
    assert.ok(
      adds.every(([text, group]) => text === 'Add part from it' && group.startsWith('Not in the Catalog yet')),
      JSON.stringify(adds),
    );
    assert.equal(
      await browser.execute(() => document.querySelector('[data-reference="g-Capacitor_SMD:C_0603_Hand"]') !== null),
      true,
    );
    await assertAccessible(browser, '62-team-library-check');
    await screenshot(browser, '62-team-library-check');
  });

  it('closes, giving the focus back to the line, and a chosen part closes it', async () => {
    await (await browser.$('#library-check-close')).click();
    await browser.waitUntil(async () => (await browser.$$('#library-check-title')).length === 0, {
      timeout: 5_000,
      timeoutMsg: 'the view did not close',
    });
    await browser.waitUntil(async () => (await focused()) === 'library-check-open', {
      timeout: 5_000,
      timeoutMsg: 'focus did not return to the line',
    });
    await (await browser.$(LINE)).click();
    await waitForText(browser, '#library-check-title', 'Library check');
    await searchFor(browser, 'RC0603FR-0747RL', '1 matching part');
    await selectResult(browser, 'RC0603FR-0747RL');
    await waitForText(browser, '#part-title', 'RC0603FR-0747RL');
    assert.equal((await browser.$$('#library-check-title')).length, 0);
  });

  it('shows no library check to someone who can only view', async () => {
    const open = await browser.execute(() => document.querySelector('.team-settings')?.open === true);
    if (!open) await (await browser.$(BADGE)).click();
    for (const button of await browser.$$('.team-settings-body button')) {
      if ((await button.getText()) === 'Sign out') {
        await button.click();
        break;
      }
    }
    await waitForText(browser, BADGE, 'Team catalog: not signed in');
    await signIn(viewer);
    await waitForLibraryCheck(browser);
    assert.equal(await browser.execute(() => document.querySelector('.search-pane').dataset.libraryCheck), 'off');
    assert.equal((await browser.$$(LINE)).length, 0);
  });
});
