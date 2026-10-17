// KiCad libraries journey (kicad-parts-f397): signed in to a team server,
// the app writes the team's KiCad libraries into its data folder and keeps
// them up to date. No libraries are made until the team has a code, which
// someone who can edit chooses once, after a confirmation. The KiCad
// settings panel then shows the code, says how the libraries are doing,
// offers Update now, shows what to paste into KiCad once, and lists other
// teams' libraries on this computer. An edit by someone else that adds a
// revision updates them by itself, a file changed outside the app is
// named, and an update that must wait its turn says so. Open folder is never
// clicked: it opens a File Explorer window on the person's desktop.

import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { after, before, describe, it } from 'node:test';
import {
  assertAccessible,
  capture,
  comparePngs,
  editAsSomeoneElse,
  launchApp,
  outDir,
  restoreTimes,
  screenshot,
  screenshotsDir,
  showTimeDigitsAs,
  startTeamServer,
  teamSignIn,
  tempDir,
  textOf,
  waitForLiveUpdates,
  waitForText,
} from '../harness.mjs';

const IPN = 'RES-0000-0470';
const BADGE = '.team-settings > summary';
const STATUS = '.kicad-libraries-status';
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
    dataDir = tempDir('data');
    // Another team's libraries are already on this computer, with code SL.
    const sl = path.join(dataDir, 'kicad-libraries', 'SL');
    fs.mkdirSync(sl, { recursive: true });
    fs.writeFileSync(
      path.join(sl, '.kicad-parts.json'),
      JSON.stringify({
        team_id: 'another-team',
        server: 'http://127.0.0.1:9999',
        code: 'SL',
        updated_at_ms: Date.UTC(2026, 9, 16, 9, 30),
      }),
    );
    ({ browser } = await launchApp({
      dataDir,
      args: [`--team-server=${server.url}`, `--team-email=${editor.email}`],
    }));
  });

  after(async () => {
    await browser?.deleteSession();
    await server?.stop();
  });

  const folder = () => path.join(dataDir, 'kicad-libraries', 'MA');

  /**
   * Proves screenshot `name`, just taken, does not depend on the clock. It
   * takes it again with every digit of every time shown as 1 (narrow) and
   * then as 8 (wide), and expects exactly the same pixels.
   */
  async function assertScreenshotIgnoresTheClock(name) {
    const taken = path.join(screenshotsDir, `${name}.png`);
    for (const digit of ['1', '8']) {
      const file = path.join(outDir, 'clock-checks', `${name}-digits-${digit}.png`);
      await showTimeDigitsAs(browser, digit, 'journeyClockCheck');
      try {
        await capture(browser, file, MACHINE_TEXT);
      } finally {
        await restoreTimes(browser, 'journeyClockCheck');
      }
      const differences = path.join(outDir, 'clock-checks', `${name}-digits-${digit}-differences.png`);
      const { differentPixels, sizeProblem } = comparePngs(taken, file, differences);
      assert.equal(sizeProblem, undefined, `${name} with every time digit ${digit}: ${sizeProblem}`);
      assert.equal(differentPixels, 0, `${name} changes when every time digit is ${digit}: see ${differences}`);
    }
  }

  async function setOpen(selector, open) {
    await browser.execute(
      (s, o) => {
        document.querySelector(s).open = o;
      },
      selector,
      open,
    );
  }

  async function state() {
    return browser.execute((s) => {
      const status = document.querySelector(s);
      return { state: status?.dataset.state ?? null, at: status?.dataset.at ?? null, text: status?.textContent ?? '' };
    }, STATUS);
  }

  /** Waits until the status is `wanted`, at a time other than `before` if given. */
  async function waitForState(wanted, before = undefined, timeout = 60_000) {
    let last;
    await browser.waitUntil(
      async () => {
        last = await state();
        return last.state === wanted && (before === undefined || last.at !== before);
      },
      { timeout, interval: 100 },
    ).catch(async (error) => {
      const panel = await browser.execute(() => document.querySelector('.kicad-libraries')?.innerText ?? '(no section)');
      throw new Error(`the libraries never became ${wanted}; last: ${JSON.stringify(last)}; the section says: ${panel}; ${error.message}`);
    });
    return last;
  }

  async function scrollPanel(to) {
    await browser.execute((where) => {
      const body = document.querySelector('.kicad-settings > .kicad-settings-body');
      body.scrollTop = where === 'top' ? 0 : body.scrollHeight;
    }, to);
  }

  async function typeCode(code) {
    const input = await browser.$('#team-code-input');
    await input.setValue(code);
    await (await browser.$('form[aria-label="Team code"] button[type="submit"]')).click();
    await browser.waitUntil(async () => browser.execute(() => document.querySelector('.team-code-confirm') !== null), {
      timeout: 10_000,
      timeoutMsg: 'the confirmation never showed',
    });
  }

  it('says to sign in first', async () => {
    await waitForText(browser, '.kicad-badge-ready', /^KiCad 10\.\d+\.\d+$/);
    await setOpen('.kicad-settings', true);
    await waitForText(browser, STATUS, 'Sign in to the team catalog to make the KiCad libraries.');
    assert.equal(await textOf(browser, '#kicad-libraries-heading'), 'KiCad libraries');
    assert.equal(await (await browser.$('#kicad-libraries-update')).isEnabled(), false);
    // Nothing to add to KiCad before the first update.
    assert.equal(await browser.execute(() => document.querySelector('#kicad-symbol-libraries')), null);
    assert.equal(fs.existsSync(folder()), false);
    await assertAccessible(browser, '88-kicad-libraries-signed-out');
    await screenshot(browser, '88-kicad-libraries-signed-out', { blankOut: MACHINE_TEXT });
    await setOpen('.kicad-settings', false);
  });

  it('makes no libraries until the team has a code, chosen once with a confirmation', async () => {
    await setOpen('.kicad-settings', false);
    await setOpen('.team-settings', true);
    await (await browser.$('aria/Password')).setValue(editor.password);
    await (await browser.$('form[aria-label="Sign in to the team catalog"] button[type="submit"]')).click();
    await waitForText(browser, BADGE, `Team catalog · ${editor.name}`);
    await setOpen('.team-settings', false);
    await waitForLiveUpdates(browser);
    await setOpen('.kicad-settings', true);
    await waitForState('needs-code');
    assert.equal((await state()).text, 'No KiCad libraries are made until the team has a code.');
    assert.equal(await (await browser.$('#kicad-libraries-update')).isEnabled(), false);
    assert.match(await textOf(browser, '#team-code-hint'), /boards save those names, so it can never change\.$/);
    // A code another team's libraries on this computer use is warned about.
    await typeCode('sl');
    assert.match(await textOf(browser, '#team-code-confirm-text'), /^Use SL as the team code\? Every member's KiCad libraries will be named SL-Parts, SL-Resistor_SMD and so on/);
    assert.match(
      await textOf(browser, '.team-code-warning'),
      /^The SL libraries of another team, from the team server http:\/\/127\.0\.0\.1:9999, are already on this computer\./,
    );
    await assertAccessible(browser, '89-kicad-libraries-team-code');
    await screenshot(browser, '89-kicad-libraries-team-code', { blankOut: MACHINE_TEXT });
    const cancel = await browser.$$('.team-code-confirm button');
    await cancel[1].click();
    assert.equal(await browser.execute(() => document.querySelector('.team-code-confirm')), null);
    // The code this team takes.
    await typeCode('MA');
    assert.equal(await browser.execute(() => document.querySelector('.team-code-warning')), null);
    await (await browser.$('#team-code-confirm')).click();
    await waitForText(browser, '#team-code', 'Team code: MA, chosen by Local Editor. It names the team\'s libraries and never changes.');
    assert.equal(await browser.execute(() => document.querySelector('#team-code-input')), null);
  });

  it('writes the libraries, named with the code, and says how to add them to KiCad', async () => {
    const shown = await waitForState('up-to-date');
    assert.match(shown.text, /^Up to date: \d+ parts, 0 older revisions and 0 archived revisions\. Updated at \d\d:\d\d\.$/);
    // The files are in the team's folder in the app's data folder.
    for (const file of ['MA-Parts.kicad_dbl', 'MA-Parts.sqlite', 'symbols/MA-res-20241209.kicad_sym']) {
      assert.ok(fs.existsSync(path.join(folder(), file)), `${file} was written`);
    }
    const pretty = path.join(folder(), 'footprints', 'MA-Resistor_SMD.pretty');
    assert.ok(fs.readdirSync(pretty).includes('R_0603_1608Metric_v1.kicad_mod'));
    assert.equal(await textOf(browser, '.kicad-libraries-folder'), folder());
    // What to paste into KiCad: every library by nickname and full path.
    const slashed = folder().replaceAll('\\', '/');
    const symbols = await textOf(browser, '#kicad-symbol-libraries');
    assert.ok(symbols.startsWith('(sym_lib_table\n  (version 7)\n'), symbols);
    assert.ok(
      symbols.includes(`(lib (name "MA-Parts")(type "Database")(uri "${slashed}/MA-Parts.kicad_dbl")`),
      symbols,
    );
    assert.ok(
      symbols.includes(
        `(lib (name "MA-res-20241209")(type "KiCad")(uri "${slashed}/symbols/MA-res-20241209.kicad_sym")(options "")(descr "Team symbols that MA-Parts uses")(hidden))`,
      ),
      symbols,
    );
    const footprints = await textOf(browser, '#kicad-footprint-libraries');
    assert.ok(
      footprints.includes(`(lib (name "MA-Resistor_SMD")(type "KiCad")(uri "${slashed}/footprints/MA-Resistor_SMD.pretty")`),
      footprints,
    );
    const steps = await browser.execute(() =>
      [...document.querySelectorAll('.kicad-libraries-steps li')].map((li) => li.textContent.trim()),
    );
    assert.equal(steps.length, 2);
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
    // The other team's libraries are listed, and stay as they are.
    assert.match(await textOf(browser, '.kicad-libraries-other-teams'), /^SL, from http:\/\/127\.0\.0\.1:9999, updated at \d\d:\d\d$/);
    assert.deepEqual(fs.readdirSync(path.join(dataDir, 'kicad-libraries', 'SL')), ['.kicad-parts.json']);
    await scrollPanel('top');
    await assertAccessible(browser, '90-kicad-libraries-up-to-date');
    await screenshot(browser, '90-kicad-libraries-up-to-date', { blankOut: MACHINE_TEXT });
    await assertScreenshotIgnoresTheClock('90-kicad-libraries-up-to-date');
    await scrollPanel('bottom');
    await screenshot(browser, '91-kicad-libraries-setup', { blankOut: MACHINE_TEXT });
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
    const copy = await browser.$('#kicad-symbol-libraries-copy');
    assert.equal(await copy.getAttribute('class'), 'kicad-library-copy');
    assert.equal(await textOf(browser, '#kicad-symbol-libraries-copy'), 'Copy the symbol libraries');
    await copy.click();
    await waitForText(browser, '#kicad-symbol-libraries-copy', 'Copied the symbol libraries');
    await waitForText(browser, '#kicad-libraries-copied', 'Copied the symbol libraries.');
    await (await browser.$('#kicad-footprint-libraries-copy')).click();
    await waitForText(browser, '#kicad-footprint-libraries-copy', 'Copied the footprint libraries');
    // What was copied is the text shown, which ends with a line break.
    const copied = (await browser.execute(() => window.journeyCopied)).map((text) => text.trim());
    assert.deepEqual(copied, [
      await textOf(browser, '#kicad-symbol-libraries'),
      await textOf(browser, '#kicad-footprint-libraries'),
    ]);
    await waitForText(browser, '#kicad-symbol-libraries-copy', 'Copy the symbol libraries', 5_000);
  });

  it('updates by itself after someone else adds a revision', async () => {
    const before = (await state()).at;
    const symbols = fs.readFileSync(path.join(folder(), 'symbols', 'MA-res-20241209.kicad_sym'));
    // KiCad's own resistor: a new revision whose symbol is linked, not copied.
    await editAsSomeoneElse(server.url, otherToken, IPN, { Symbol: 'Device:R' }, "Use KiCad's resistor");
    const shown = await waitForState('up-to-date', before);
    assert.match(shown.text, /: \d+ parts, 1 older revision and 0 archived revisions\. Updated at \d\d:\d\d\.$/);
    // Revision 1 keeps its files byte for byte.
    assert.deepEqual(fs.readFileSync(path.join(folder(), 'symbols', 'MA-res-20241209.kicad_sym')), symbols);
  });

  it('updates at once on Update now, and names a file changed outside the app', async () => {
    const before = (await state()).at;
    const file = path.join(folder(), 'footprints', 'MA-Resistor_SMD.pretty', 'R_0603_1608Metric_v1.kicad_mod');
    const original = fs.readFileSync(file);
    fs.chmodSync(file, 0o666);
    fs.writeFileSync(file, Buffer.concat([original, Buffer.from('\n')]));
    await scrollPanel('top');
    await (await browser.$('#kicad-libraries-update')).click();
    await waitForState('up-to-date', before, 20_000);
    await waitForText(browser, '.kicad-libraries-problems > summary', '1 item to look at');
    assert.equal(
      await browser.execute(() => document.querySelector('.kicad-libraries-problems li').textContent),
      'footprints/MA-Resistor_SMD.pretty/R_0603_1608Metric_v1.kicad_mod was changed outside this app, so it was kept as it is. Delete that file, then update again to write it from the team catalog.',
    );
    fs.rmSync(file);
    const again = (await state()).at;
    await (await browser.$('#kicad-libraries-update')).click();
    await waitForState('up-to-date', again, 20_000);
    assert.deepEqual(fs.readFileSync(file), original);
    assert.equal(await browser.execute(() => document.querySelector('.kicad-libraries-problems')), null);
  });

  it('says plainly when an update must wait its turn', async () => {
    // Another program holds the folder's lock, as a second app would.
    const lock = fs.openSync(path.join(folder(), '.update.lock'), 'r+');
    try {
      await (await browser.$('#kicad-libraries-update')).click();
      const busy = await waitForState('busy', undefined, 20_000);
      assert.match(
        busy.text,
        /^Another update of these libraries is running, from this app or another one, so this one waits\. It tries again in 30 seconds\. Tried at \d\d:\d\d\.$/,
      );
      // A plain status, not a failure or an alert.
      assert.equal(await browser.execute((s) => document.querySelector(s).getAttribute('role'), STATUS), 'status');
      assert.equal(
        await browser.execute((s) => document.querySelector(s).classList.contains('kicad-libraries-failed'), STATUS),
        false,
      );
      await assertAccessible(browser, '92-kicad-libraries-busy');
      await screenshot(browser, '92-kicad-libraries-busy', { blankOut: MACHINE_TEXT });
      await assertScreenshotIgnoresTheClock('92-kicad-libraries-busy');
    } finally {
      fs.closeSync(lock);
    }
    await (await browser.$('#kicad-libraries-update')).click();
    await waitForState('up-to-date', undefined, 20_000);
    await setOpen('.kicad-settings', false);
  });
});
