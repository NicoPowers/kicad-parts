// Offline start journey (kicad-parts-ed46): the person works for two
// clients. While both team servers answer, the app keeps each team's
// parts on disk. The next day one server is down when the app starts.
// That team's parts still show, marked offline and read-only, and its row
// in the Teams menu and the part's offline note say when they were last
// up to date. Sign-ins stay in a file in the journey's own data folder
// (--sign-ins=test-file, debug builds only), never the person's
// credential store.

import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { after, before, describe, it } from 'node:test';
import {
  FOOTPRINT,
  SYMBOL,
  assertAccessible,
  launchApp,
  screenshot,
  searchFor,
  startTeamServer,
  tempDir,
  waitForLiveUpdates,
  waitForPreview,
  waitForText,
} from '../harness.mjs';

const MPN = 'RC0603FR-0747RL';
const BADGE = '.team-settings > summary';

describe('a team server down when the app starts', () => {
  let acme;
  let beta;
  let browser;
  let dataDir;

  const launch = async () => {
    ({ browser } = await launchApp({
      dataDir,
      args: ['--sign-ins=test-file', `--team-server=${acme.url}`, `--team-email=${acme.accounts[0].email}`],
    }));
  };

  before(async () => {
    [acme, beta] = await Promise.all([
      startTeamServer({ csv: ['g-res'], teamCode: 'AC' }),
      startTeamServer({ csv: ['g-res'], teamCode: 'BE' }),
    ]);
    dataDir = tempDir('data');
    await launch();
  });

  after(async () => {
    await browser?.deleteSession();
    await acme?.stop();
    await beta?.stop();
  });

  async function setMenuOpen(open) {
    await browser.execute((wanted) => {
      document.querySelector('.team-settings').open = wanted;
    }, open);
  }

  async function signInWithForm(server, account) {
    await (await browser.$('#team-server')).setValue(server.url);
    await (await browser.$('#team-email')).setValue(account.email);
    await (await browser.$('#team-password')).setValue(account.password);
    await (await browser.$('form[aria-label="Sign in to the team catalog"] button[type="submit"]')).click();
  }

  /** What the row of the team named `label` in the Teams menu says. */
  async function rowText(label) {
    return browser.execute(
      (name) =>
        [...document.querySelectorAll('.team-row')]
          .find((row) => row.querySelector('.team-name strong')?.textContent === name)
          ?.querySelector('.team-state')?.textContent ?? null,
      label,
    );
  }

  async function fieldValue(name) {
    return browser.execute((label) => {
      const row = [...document.querySelectorAll('.part-fields tbody tr')].find(
        (tr) => tr.querySelector('th')?.textContent.trim() === label,
      );
      return row?.querySelector('td')?.textContent.trim() ?? null;
    }, name);
  }

  /** The kept-parts files the app wrote, one per team. */
  const keptFiles = () => {
    const folder = path.join(dataDir, 'parts-files');
    return fs.existsSync(folder) ? fs.readdirSync(folder).filter((name) => name.endsWith('.json')) : [];
  };

  it('keeps each team\'s parts on disk while its server answers', async () => {
    await waitForText(browser, '#result-count', '178 matching parts');
    await setMenuOpen(true);
    await signInWithForm(acme, acme.accounts[0]);
    await waitForText(browser, BADGE, `Team catalog · ${acme.accounts[0].name}`);
    await setMenuOpen(true);
    await (await browser.$('#team-add')).click();
    await signInWithForm(beta, beta.accounts[0]);
    await waitForText(browser, BADGE, 'Teams · 2 live');
    await waitForLiveUpdates(browser);
    // Written a few seconds after each catch-up, once the parts stay the same.
    await browser.waitUntil(async () => keptFiles().length === 2, {
      timeout: 30_000,
      timeoutMsg: `the app kept ${keptFiles().length} teams' parts, not 2`,
    });
  });

  it('shows the kept parts of a team whose server is down at start, marked offline', async () => {
    await browser.deleteSession();
    await beta.stop();
    await launch();
    await waitForText(browser, BADGE, 'Teams · 1 live, 1 offline', 30_000);
    // Beta's parts show, from the last run, and say the team is offline.
    await searchFor(browser, MPN, '2 matching parts');
    await browser.waitUntil(
      async () =>
        (await browser.execute(
          (team) => document.querySelector(`button.result[data-team="${team}"] .team-chip`)?.textContent,
          beta.url,
        )) === 'Team BE · offline',
      { timeout: 10_000, timeoutMsg: 'beta\'s row never said it is offline' },
    );
    // Beta's row in the Teams menu says when its parts were last up to date.
    await setMenuOpen(true);
    await browser.waitUntil(async () => /^Offline\. Parts as of /.test((await rowText('BE')) ?? ''), {
      timeout: 10_000,
      timeoutMsg: 'beta\'s row never said when its parts are from',
    });
    assert.match(
      await rowText('BE'),
      /^Offline\. Parts as of \d\d:\d\d\. The app keeps trying, and signs you in when it is back\. Last problem: /,
    );
    assert.equal(
      await browser.execute(() =>
        [...document.querySelectorAll('.team-row .team-state time')].every((time) => time.dateTime.endsWith('Z')),
      ),
      true,
      'the time says the full UTC time for machines',
    );
    // No screenshot of the menu: the last problem names the server's
    // address, whose port changes from run to run.
    await assertAccessible(browser, '102-teams-offline-start-menu');
    await setMenuOpen(false);
  });

  it('shows a kept part read-only, with when it was last up to date', async () => {
    await browser.execute((team) => document.querySelector(`button.result[data-team="${team}"]`).click(), beta.url);
    await browser.waitUntil(async () => (await fieldValue('Value')) === '47', {
      timeout: 20_000,
      timeoutMsg: `beta's part never showed its kept value; last: ${await fieldValue('Value')}`,
    });
    await waitForText(
      browser,
      '.team-offline-note',
      /^BE is offline, so this part can't be edited now\. Parts as of \d\d:\d\d\. The app reconnects by itself\.$/,
    );
    assert.equal(
      await browser.execute(() =>
        [...document.querySelectorAll('.team-toolbar button')].some((b) => b.textContent.trim() === 'Edit part'),
      ),
      false,
      'no Edit part while offline',
    );
    await waitForPreview(browser, SYMBOL);
    await waitForPreview(browser, FOOTPRINT);
    await assertAccessible(browser, '102-teams-offline-start');
    await screenshot(browser, '102-teams-offline-start');
  });
});
