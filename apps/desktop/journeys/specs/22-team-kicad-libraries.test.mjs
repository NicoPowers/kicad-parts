// KiCad libraries journey (kicad-parts-f397): signed in to a team server,
// the app writes the KiCad libraries into its data folder and keeps them
// up to date. The KiCad settings panel says how they are doing, offers
// Update now, and shows what to paste into KiCad once. An edit by someone
// else that adds a revision updates them by itself, and a file changed
// outside the app is named plainly. Open folder is never clicked: it opens
// a File Explorer window on the person's desktop.

import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { after, before, describe, it } from 'node:test';
import {
  assertAccessible,
  editAsSomeoneElse,
  launchApp,
  screenshot,
  startTeamServer,
  teamSignIn,
  textOf,
  waitForLiveUpdates,
  waitForText,
} from '../harness.mjs';

const IPN = 'RES-0000-0470';
const BADGE = '.team-settings > summary';
const STATUS = '.kicad-libraries-status';
const SECTION = '.kicad-libraries';
/** Text that differs between machines and runs: paths and the kicad-cli location. */
const MACHINE_TEXT = [
  '.kicad-libraries-folder',
  '#kicad-symbol-libraries',
  '#kicad-footprint-libraries',
  '.kicad-settings-body > p:first-child code',
  // Whether the SQLite3 ODBC driver is installed differs between machines.
  '.kicad-libraries-driver',
];

describe('KiCad libraries made from the team catalog', () => {
  let server;
  let browser;
  let dataDir;
  let editor;
  let otherToken;

  before(async () => {
    server = await startTeamServer({ csv: ['g-res'] });
    let other;
    [editor, other] = server.accounts;
    otherToken = await teamSignIn(server.url, other);
    ({ browser, dataDir } = await launchApp({
      args: [`--team-server=${server.url}`, `--team-email=${editor.email}`],
    }));
  });

  after(async () => {
    await browser?.deleteSession();
    await server?.stop();
  });

  const folder = () => path.join(dataDir, 'kicad-libraries');

  async function setOpen(selector, open) {
    await browser.execute((s, o) => {
      document.querySelector(s).open = o;
    }, selector, open);
  }

  async function state() {
    return browser.execute((s) => {
      const status = document.querySelector(s);
      return { state: status?.dataset.state ?? null, at: status?.dataset.at ?? null, text: status?.textContent ?? '' };
    }, STATUS);
  }

  /** Waits until the status is up to date at a time other than `before`. */
  async function waitForUpdate(before, timeout = 60_000) {
    let last;
    await browser.waitUntil(
      async () => {
        last = await state();
        return last.state === 'up-to-date' && last.at !== before;
      },
      { timeout, interval: 100, timeoutMsg: `the libraries were never updated; last: ${JSON.stringify(last)}` },
    );
    return last;
  }

  async function scrollPanel(to) {
    await browser.execute((where) => {
      const body = document.querySelector('.kicad-settings > .kicad-settings-body');
      body.scrollTop = where === 'top' ? 0 : body.scrollHeight;
    }, to);
  }

  it('says to sign in first', async () => {
    await waitForText(browser, '.kicad-badge-ready', /^KiCad 10\.\d+\.\d+$/);
    await setOpen('.kicad-settings', true);
    await waitForText(browser, STATUS, 'Sign in to the team catalog to make the KiCad libraries.');
    assert.equal(await textOf(browser, '#kicad-libraries-heading'), 'KiCad libraries');
    assert.equal(await (await browser.$('#kicad-libraries-update')).isEnabled(), false);
    assert.equal(await textOf(browser, '.kicad-libraries-folder'), folder());
    // Nothing to add to KiCad before the first update.
    assert.equal(await browser.execute(() => document.querySelector('#kicad-symbol-libraries')), null);
    assert.equal(fs.existsSync(folder()), false);
    await assertAccessible(browser, '88-kicad-libraries-signed-out');
    await screenshot(browser, '88-kicad-libraries-signed-out', { blankOut: MACHINE_TEXT });
    await setOpen('.kicad-settings', false);
  });

  it('writes the libraries once signed in, and says how to add them to KiCad', async () => {
    await setOpen('.kicad-settings', false);
    await setOpen('.team-settings', true);
    await (await browser.$('aria/Password')).setValue(editor.password);
    await (await browser.$('form[aria-label="Sign in to the team catalog"] button[type="submit"]')).click();
    await waitForText(browser, BADGE, `Team catalog · ${editor.name}`);
    await waitForLiveUpdates(browser);
    await setOpen('.kicad-settings', true);
    const shown = await waitForUpdate(null);
    assert.match(shown.text, /^Up to date at \d\d:\d\d: \d+ parts, 0 older revisions and 0 archived revisions\.$/);
    // The files are in the app's data folder.
    for (const file of ['team-parts.kicad_dbl', 'team-parts.sqlite', 'symbols/g-res.kicad_sym']) {
      assert.ok(fs.existsSync(path.join(folder(), file)), `${file} was written`);
    }
    const pretty = path.join(folder(), 'footprints', 'g-Resistor_SMD.pretty');
    assert.ok(fs.readdirSync(pretty).includes('R_0603_1608Metric_v1.kicad_mod'));
    // What to paste into KiCad: every library by nickname and full path.
    const slashed = folder().replaceAll('\\', '/');
    const symbols = await textOf(browser, '#kicad-symbol-libraries');
    assert.ok(symbols.startsWith('(sym_lib_table\n  (version 7)\n'), symbols);
    assert.ok(
      symbols.includes(`(lib (name "team-parts")(type "Database")(uri "${slashed}/team-parts.kicad_dbl")`),
      symbols,
    );
    assert.ok(
      symbols.includes(`(lib (name "g-res")(type "KiCad")(uri "${slashed}/symbols/g-res.kicad_sym")(options "")(descr "Team symbols for team-parts")(hidden))`),
      symbols,
    );
    const footprints = await textOf(browser, '#kicad-footprint-libraries');
    assert.ok(
      footprints.includes(`(lib (name "g-Resistor_SMD")(type "KiCad")(uri "${slashed}/footprints/g-Resistor_SMD.pretty")`),
      footprints,
    );
    const steps = await browser.execute(() =>
      [...document.querySelectorAll('.kicad-libraries-steps li')].map((li) => li.textContent.trim()),
    );
    assert.equal(steps.length, 3);
    assert.match(steps[0], /Preferences, then Manage Symbol Libraries\. On the Global Libraries tab, right-click the table, choose Paste, then choose OK\./);
    assert.match(steps[1], /Manage Footprint Libraries/);
    assert.match(
      await textOf(browser, '.kicad-libraries-note'),
      /within about 10 seconds: close and open the Add Symbol window to see them\.$/,
    );
    assert.match(
      await textOf(browser, '.kicad-libraries-driver'),
      /^KiCad reads the parts through the SQLite3 ODBC driver, which is (installed|not installed\. Install the 64-bit SQLite3 ODBC driver, then restart KiCad)\.$/,
    );
    await scrollPanel('top');
    await assertAccessible(browser, '89-kicad-libraries-up-to-date');
    await screenshot(browser, '89-kicad-libraries-up-to-date', { blankOut: MACHINE_TEXT });
    await scrollPanel('bottom');
    await screenshot(browser, '90-kicad-libraries-setup', { blankOut: MACHINE_TEXT });
  });

  it('copies the libraries to paste into KiCad', async () => {
    // The journey never touches the person's clipboard: writing to it is
    // recorded instead.
    await browser.execute(() => {
      window.journeyCopied = [];
      navigator.clipboard.writeText = async (text) => {
        window.journeyCopied.push(text);
      };
    });
    await (await browser.$('#kicad-symbol-libraries-copy')).click();
    await waitForText(browser, '#kicad-symbol-libraries-copy', 'Copied');
    await (await browser.$('#kicad-footprint-libraries-copy')).click();
    await waitForText(browser, '#kicad-footprint-libraries-copy', 'Copied');
    // What was copied is the text shown, which ends with a line break.
    const copied = (await browser.execute(() => window.journeyCopied)).map((text) => text.trim());
    assert.deepEqual(copied, [
      await textOf(browser, '#kicad-symbol-libraries'),
      await textOf(browser, '#kicad-footprint-libraries'),
    ]);
    await waitForText(browser, '#kicad-symbol-libraries-copy', 'Copy', 5_000);
  });

  it('updates by itself after someone else adds a revision', async () => {
    const before = (await state()).at;
    const symbols = fs.readFileSync(path.join(folder(), 'symbols', 'g-res.kicad_sym'));
    // KiCad's own resistor: a new revision whose symbol is linked, not copied.
    await editAsSomeoneElse(server.url, otherToken, IPN, { Symbol: 'Device:R' }, "Use KiCad's resistor");
    const shown = await waitForUpdate(before);
    assert.match(shown.text, /: \d+ parts, 1 older revision and 0 archived revisions\.$/);
    // Revision 1 keeps its files byte for byte.
    assert.deepEqual(fs.readFileSync(path.join(folder(), 'symbols', 'g-res.kicad_sym')), symbols);
  });

  it('updates at once on Update now', async () => {
    const before = (await state()).at;
    await scrollPanel('top');
    await (await browser.$('#kicad-libraries-update')).click();
    await waitForUpdate(before, 20_000);
  });

  it('names a file changed outside the app, and writes it again once deleted', async () => {
    const file = path.join(folder(), 'footprints', 'g-Resistor_SMD.pretty', 'R_0603_1608Metric_v1.kicad_mod');
    const original = fs.readFileSync(file);
    fs.writeFileSync(file, Buffer.concat([original, Buffer.from('\n')]));
    await (await browser.$('#kicad-libraries-update')).click();
    await browser.waitUntil(async () => (await state()).state === 'failed', {
      timeout: 20_000,
      timeoutMsg: 'the update never failed',
    });
    assert.match(
      (await state()).text,
      /^The update at \d\d:\d\d failed\. footprints\/g-Resistor_SMD\.pretty\/R_0603_1608Metric_v1\.kicad_mod was changed outside this app, so it was left as it is\. Delete that file, then update again to write it from the team catalog\.$/,
    );
    // A status, not an alert: KiCad and the rest of the app still work.
    assert.equal(await browser.execute((s) => document.querySelector(s).getAttribute('role'), STATUS), 'status');
    await assertAccessible(browser, '91-kicad-libraries-failed');
    await screenshot(browser, '91-kicad-libraries-failed', { blankOut: MACHINE_TEXT });
    fs.rmSync(file);
    await (await browser.$('#kicad-libraries-update')).click();
    await waitForUpdate(null);
    assert.deepEqual(fs.readFileSync(file), original);
    await setOpen('.kicad-settings', false);
  });
});
