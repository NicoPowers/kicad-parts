// Several teams journey (kicad-parts-f10d, kicad-parts-8a71): the person
// works for two clients, each its own team on its own server. They sign in
// to one and add the other without signing out. One part list then shows
// both teams' parts, each row marked with its team, and a Team filter
// chosen from the keyboard shows one team. When one server stops, that
// team's rows stay, marked offline, and its parts can't be edited, while
// the other team keeps working. Signing out and removing touch one team
// only. Without a credential store, the
// Teams menu says once why the app will ask again after a restart. KiCad
// libraries made from another server address wait until the person, asked
// on the team's row, says the team moved (kicad-parts-092b). If both
// servers are in the person's teams, the row says so and asks nothing.
// Sign-ins stay in the app's memory (--sign-ins), never the person's
// credential store.

import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { after, before, describe, it } from 'node:test';
import {
  FOOTPRINT,
  SYMBOL,
  assertAccessible,
  editAsSomeoneElse,
  launchApp,
  screenshot,
  searchFor,
  selectResult,
  startTeamServer,
  teamSignIn,
  tempDir,
  textOf,
  waitForLibraryCheck,
  waitForLiveUpdates,
  waitForPreview,
  waitForText,
} from '../harness.mjs';

const IPN = 'RES-0000-0470';
const MPN = 'RC0603FR-0747RL';
const BADGE = '.team-settings > summary';
/** Server addresses hold ports that change from run to run. */
const ADDRESSES = ['.team-server'];

describe('several teams at once', () => {
  let acme;
  let beta;
  let browser;
  let editor;

  before(async () => {
    [acme, beta] = await Promise.all([
      startTeamServer({ csv: ['g-res'], teamCode: 'AC' }),
      startTeamServer({ csv: ['g-res'], teamCode: 'BE' }),
    ]);
    editor = acme.accounts[0];
    // Acme's value differs, so each team's part can be told apart.
    const other = acme.accounts[1];
    await editAsSomeoneElse(acme.url, await teamSignIn(acme.url, other), IPN, { Value: '47R' }, 'BOM style');
    ({ browser } = await launchApp({
      args: [`--team-server=${acme.url}`, `--team-email=${editor.email}`],
    }));
  });

  after(async () => {
    await browser?.deleteSession();
    await acme?.stop();
    await beta?.stop();
  });

  /** AC's KiCad library folder state file, once a step makes it. */
  let stateFile;
  const recorded = () => JSON.parse(fs.readFileSync(stateFile, 'utf8'));

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

  /** The row of the team named `label` in the Teams menu. */
  async function rowText(label) {
    return browser.execute(
      (name) =>
        [...document.querySelectorAll('.team-row')]
          .find((row) => row.querySelector('.team-name strong')?.textContent === name)
          ?.querySelector('.team-state')?.textContent ?? null,
      label,
    );
  }

  async function clickInRow(label, text) {
    const clicked = await browser.execute(
      (name, button) => {
        const row = [...document.querySelectorAll('.team-row')].find(
          (r) => r.querySelector('.team-name strong')?.textContent === name,
        );
        const found = [...(row?.querySelectorAll('button') ?? [])].find((b) => b.textContent.trim() === button);
        found?.click();
        return Boolean(found);
      },
      label,
      text,
    );
    assert.ok(clicked, `no ${text} button for ${label}`);
  }

  async function fieldValue(name) {
    return browser.execute((label) => {
      const row = [...document.querySelectorAll('.part-fields tbody tr')].find(
        (tr) => tr.querySelector('th')?.textContent.trim() === label,
      );
      return row?.querySelector('td')?.textContent.trim() ?? null;
    }, name);
  }

  async function waitForValue(value) {
    await browser.waitUntil(async () => (await fieldValue('Value')) === value, {
      timeout: 20_000,
      timeoutMsg: `the part never showed the value ${value}; last: ${await fieldValue('Value')}`,
    });
  }

  it('adds a second team without signing out of the first', async () => {
    await waitForText(browser, '#result-count', '178 matching parts');
    await setMenuOpen(true);
    await signInWithForm(acme, editor);
    await waitForText(browser, BADGE, `Team catalog · ${editor.name}`);
    await setMenuOpen(true);
    // The team's code names it once the app has read it.
    await browser.waitUntil(async () => (await rowText('AC')) !== null, {
      timeout: 20_000,
      timeoutMsg: 'the team was never named by its code',
    });
    // With one team signed in, the list shows no team marks and no filter.
    assert.equal(await browser.execute(() => document.querySelector('#team-filter')), null);
    assert.equal(await browser.execute(() => document.querySelector('.results .team-chip')), null);
    await (await browser.$('#team-add')).click();
    assert.equal(
      await browser.execute(() => document.activeElement?.id),
      'team-server',
      'Add a team puts the cursor in the address',
    );
    await signInWithForm(beta, beta.accounts[0]);
    // Both teams' parts are shown; the badge counts each team once.
    await waitForText(browser, BADGE, 'Teams · 2 live');
    await waitForLiveUpdates(browser);
    await setMenuOpen(true);
    await browser.waitUntil(async () => (await rowText('BE')) !== null, { timeout: 20_000 });
    assert.match(await rowText('AC'), /^Signed in as .+ You can edit parts\. Changes others make appear here by themselves\.$/);
    assert.match(await rowText('BE'), /^Signed in as /);
    assert.equal(await textOf(browser, '#teams-heading'), 'Your teams');
    // Each team's name is in its own colour, as its rows are marked.
    assert.deepEqual(
      await browser.execute(() => [...document.querySelectorAll('.team-row .team-name strong')].map((s) => s.className)),
      ['team-chip team-colour-blue', 'team-chip team-colour-green'],
    );
    // The filter shows every team at first.
    assert.equal(await (await browser.$('#team-filter')).getValue(), '');
    await assertAccessible(browser, '31-teams-two-signed-in');
    await screenshot(browser, '31-teams-two-signed-in', { blankOut: ADDRESSES });
    await setMenuOpen(false);
  });

  /** Each row shown, as `team code:IPN`, in order. */
  async function rows() {
    return browser.execute(() =>
      [...document.querySelectorAll('.results button.result')].map((row) => {
        const chip = row.querySelector('.team-chip');
        const code = chip ? chip.childNodes[1].textContent : '';
        return `${code}:${row.dataset.ipn}`;
      }),
    );
  }

  it('shows both teams\' parts in one list, each marked with its team', async () => {
    await searchFor(browser, MPN, '2 matching parts');
    // The same IPN once per team, in the order of the Teams menu.
    assert.deepEqual(await rows(), [`AC:${IPN}`, `BE:${IPN}`]);
    // Each team's part shows that team's values.
    await browser.execute((team) => document.querySelector(`button.result[data-team="${team}"]`).click(), beta.url);
    await waitForValue('47');
    await browser.execute((team) => document.querySelector(`button.result[data-team="${team}"]`).click(), acme.url);
    await waitForValue('47R');
    await waitForPreview(browser, SYMBOL);
    await waitForPreview(browser, FOOTPRINT);
    await assertAccessible(browser, '38-teams-one-list');
    await screenshot(browser, '38-teams-one-list');
  });

  it('shows one team with the Team filter, from the keyboard', async () => {
    const filter = await browser.$('#team-filter');
    assert.equal(await (await browser.$('label[for="team-filter"]')).getText(), 'Team');
    // Move to the first team with the arrow key, as a person would.
    await browser.execute(() => document.querySelector('#team-filter').focus());
    await browser.keys('ArrowDown');
    await browser.waitUntil(async () => (await filter.getValue()) === acme.url, {
      timeout: 5_000,
      timeoutMsg: 'the arrow key did not choose the first team',
    });
    await waitForText(browser, '#result-count', '1 matching part');
    assert.deepEqual(await rows(), [`AC:${IPN}`]);
    // The filter keeps the focus while the list changes under it.
    assert.equal(await browser.execute(() => document.activeElement?.id), 'team-filter');
    await waitForValue('47R');
    await assertAccessible(browser, '39-teams-filter');
    await screenshot(browser, '39-teams-filter');
    // Back to every team. (Taking a screenshot moves the focus away.)
    await browser.execute(() => document.querySelector('#team-filter').focus());
    await browser.keys('ArrowUp');
    await waitForText(browser, '#result-count', '2 matching parts');
  });

  it('marks a team offline when its server stops, and keeps the other working', async () => {
    await beta.stop();
    await waitForText(browser, BADGE, 'Teams · 1 live, 1 offline', 30_000);
    // Beta's rows stay, from the parts last seen, and say it is offline.
    await browser.waitUntil(
      async () =>
        (await browser.execute(
          (team) => document.querySelector(`button.result[data-team="${team}"] .team-chip`)?.textContent,
          beta.url,
        )) === 'Team BE · offline',
      { timeout: 10_000, timeoutMsg: 'beta\'s row never said it is offline' },
    );
    // Acme keeps working.
    await waitForValue('47R');
    await waitForPreview(browser, SYMBOL);
    await waitForPreview(browser, FOOTPRINT);
    await setMenuOpen(true);
    assert.match(await rowText('BE'), /^Offline: showing the parts last seen\. The app reconnects by itself\./);
    await setMenuOpen(false);
    // Beta's part shows as last seen, and says why it can't be edited.
    await browser.execute((team) => document.querySelector(`button.result[data-team="${team}"]`).click(), beta.url);
    await waitForValue('47');
    await waitForText(
      browser,
      '.team-offline-note',
      "BE is offline, so this part can't be edited now. You see it as it was last seen. The app reconnects by itself.",
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
    await assertAccessible(browser, '33-teams-offline');
    await screenshot(browser, '33-teams-offline', { blankOut: ADDRESSES });
  });

  it('signs out of one team and removes another, each alone', async () => {
    await setMenuOpen(true);
    await clickInRow('AC', 'Sign out');
    await browser.waitUntil(async () => (await rowText('AC')) === `Signed out (${editor.email}).`, {
      timeout: 10_000,
      timeoutMsg: 'acme never showed as signed out',
    });
    // Beta, offline, keeps its session.
    assert.match(await rowText('BE'), /^Offline/);
    // Remove asks once, in plain words, with the keyboard on Cancel.
    await clickInRow('BE', 'Remove');
    await waitForText(
      browser,
      '#team-remove-question',
      "Remove BE from your teams? You'll need the address, email and password to add it again.",
    );
    assert.equal(await browser.execute(() => document.activeElement?.id), 'team-remove-cancel');
    // Behind the menu, the part panel still shows beta's part: let it settle.
    await waitForLibraryCheck(browser);
    await waitForPreview(browser, SYMBOL);
    await waitForPreview(browser, FOOTPRINT);
    await assertAccessible(browser, '35-teams-remove-question');
    await screenshot(browser, '35-teams-remove-question', {
      blankOut: [...ADDRESSES, '.team-row:nth-child(2) .team-state'],
    });
    // Cancel keeps it; asking again and choosing Remove, from the keyboard,
    // removes it. (Taking a screenshot moves the focus away.)
    await browser.execute(() => document.getElementById('team-remove-cancel').focus());
    await browser.keys('Enter');
    assert.match(await rowText('BE'), /^Offline/);
    await clickInRow('BE', 'Remove');
    await browser.waitUntil(async () => (await browser.execute(() => document.activeElement?.id)) === 'team-remove-cancel', {
      timeout: 5_000,
      timeoutMsg: `the question never took the focus: ${await browser.execute(() => `${document.activeElement?.outerHTML?.slice(0, 120)} question=${Boolean(document.querySelector('#team-remove-question'))}`)}`,
    });
    await browser.keys(['Shift', 'Tab']);
    assert.equal(await browser.execute(() => document.activeElement?.id), 'team-remove-confirm');
    await browser.keys('Enter');
    await browser.waitUntil(async () => (await rowText('BE')) === null, { timeout: 10_000 });
    await waitForText(browser, BADGE, 'Team catalog: not signed in');
    // Sign in again needs only the password: the address and email are filled in.
    await clickInRow('AC', 'Sign in');
    assert.equal(await (await browser.$('#team-server')).getValue(), acme.url);
    assert.equal(await (await browser.$('#team-email')).getValue(), editor.email);
    assert.equal(await browser.execute(() => document.activeElement?.id), 'team-password');
    await browser.keys([...editor.password, 'Enter']);
    await waitForText(browser, BADGE, `Team catalog · ${editor.name}`);
  });

  it('says once why sign-ins last only until the app closes, without a credential store', async () => {
    await browser.deleteSession();
    ({ browser } = await launchApp({
      args: ['--sign-ins=until-closed', `--team-server=${acme.url}`, `--team-email=${editor.email}`],
    }));
    await waitForText(browser, '#result-count', '178 matching parts');
    await setMenuOpen(true);
    await waitForText(
      browser,
      '#sign-ins-until-closed',
      'You stay signed in only until the app closes, because the app was started with --sign-ins=until-closed. After a restart, the app asks for each password again.',
    );
    assert.equal(await textOf(browser, '.team-sign-in .hint'), 'The app never saves your password.');
    await assertAccessible(browser, '34-teams-until-closed');
    await screenshot(browser, '34-teams-until-closed', { blankOut: ['#team-server'] });
  });

  it('asks on the team row before its KiCad libraries take a new server address', async () => {
    // AC's libraries are on this computer already, made when the team
    // was at another address.
    const token = await teamSignIn(acme.url, editor);
    const settings = await (await fetch(`${acme.url}/api/team`, { headers: { Authorization: token } })).json();
    const dataDir = tempDir('data');
    const folder = path.join(dataDir, 'kicad-libraries', 'AC');
    stateFile = path.join(folder, '.kicad-parts.json');
    fs.mkdirSync(folder, { recursive: true });
    fs.writeFileSync(
      stateFile,
      JSON.stringify({ team_id: settings.id, server: 'https://old.example.com', code: 'AC', updated_at_ms: 1 }),
    );
    await browser.deleteSession();
    ({ browser } = await launchApp({
      dataDir,
      args: [`--team-server=${acme.url}`, `--team-email=${editor.email}`],
    }));
    await waitForText(browser, '#result-count', '178 matching parts');
    await setMenuOpen(true);
    await signInWithForm(acme, editor);
    // The badge says a team needs a look, with the problem look.
    await waitForText(browser, BADGE, `Team catalog · ${editor.name} · needs a look`, 30_000);
    assert.equal(
      await browser.execute(() => document.querySelector('.team-settings > summary').classList.contains('kicad-badge-problem')),
      true,
    );
    await setMenuOpen(true);
    const question = '.team-question p';
    await waitForText(
      browser,
      question,
      `The KiCad libraries for AC come from https://old.example.com. Update them from ${acme.url} instead? Only do this if the team moved to this new address.`,
    );
    assert.equal(
      await browser.execute(() => document.querySelector('.team-question').getAttribute('aria-labelledby')),
      await browser.execute(() => document.querySelector('.team-question p').id),
      'the question names its group',
    );
    // Nothing was written from the new address.
    assert.equal(recorded().server, 'https://old.example.com');
    assert.deepEqual(fs.readdirSync(folder), ['.kicad-parts.json']);
    await waitForLibraryCheck(browser);
    await assertAccessible(browser, '36-teams-libraries-new-address');
    await screenshot(browser, '36-teams-libraries-new-address', {
      blankOut: [...ADDRESSES, '.team-question-server'],
    });
    // Not now, from the keyboard: the libraries stay as they are, and the
    // answer stays one button away.
    await browser.execute(() => document.querySelector('.team-use-new-server').focus());
    await browser.keys('Tab');
    assert.equal(await browser.execute(() => document.activeElement?.textContent.trim()), 'Not now');
    await browser.keys('Enter');
    await waitForText(
      browser,
      '.team-question .hint',
      'The app does not update the KiCad libraries for AC until you choose Use the new address.',
    );
    assert.equal(await browser.execute(() => document.activeElement?.textContent.trim()), 'Sign out');
    assert.equal(recorded().server, 'https://old.example.com');
    // Use the new address, from the keyboard: the folder records it, and
    // the libraries are made from it.
    await browser.execute(() => document.querySelector('.team-use-new-server').focus());
    await browser.keys('Enter');
    await browser.waitUntil(async () => (await browser.execute(() => document.querySelector('.team-question'))) === null, {
      timeout: 10_000,
      timeoutMsg: 'the question stayed after it was answered',
    });
    assert.equal(recorded().server, acme.url);
    await waitForText(browser, BADGE, `Team catalog · ${editor.name}`);
    await browser.waitUntil(() => fs.existsSync(path.join(folder, 'AC-Parts.kicad_dbl')), {
      timeout: 60_000,
      timeoutMsg: 'the libraries were never made from the new address',
    });
    await setMenuOpen(false);
  });

  it('says plainly when two team servers in your teams claim one team', async () => {
    // A second server in the person's teams says it is AC, and AC's
    // libraries came from it last.
    const gamma = await startTeamServer({ csv: ['g-res'], teamCode: 'GA' });
    try {
      await setMenuOpen(true);
      await (await browser.$('#team-add')).click();
      await signInWithForm(gamma, gamma.accounts[0]);
      await setMenuOpen(true);
      await browser.waitUntil(async () => (await rowText('GA')) !== null, {
        timeout: 20_000,
        timeoutMsg: 'the second team was never named by its code',
      });
      // The app keeps the file read-only.
      fs.chmodSync(stateFile, 0o644);
      fs.writeFileSync(stateFile, JSON.stringify({ ...recorded(), server: gamma.url }));
      // Signing in to AC again updates its libraries.
      await clickInRow('AC', 'Sign out');
      await browser.waitUntil(async () => (await rowText('AC')) === `Signed out (${editor.email}).`, {
        timeout: 10_000,
        timeoutMsg: 'AC never showed as signed out',
      });
      await clickInRow('AC', 'Sign in');
      await setMenuOpen(true);
      await signInWithForm(acme, editor);
      await setMenuOpen(true);
      await waitForText(
        browser,
        '.team-two-servers',
        `Two team servers say they are team AC: ${gamma.url} and ${acme.url}. The app updates AC's libraries only from ${gamma.url}. If the team moved, remove ${gamma.url} from your teams. If it did not, remove ${acme.url}.`,
        30_000,
      );
      // There is no answer to give on the row, and nothing was written.
      assert.equal(await browser.execute(() => document.querySelectorAll('.team-question button').length), 0);
      assert.equal(recorded().server, gamma.url);
      assert.match(await textOf(browser, BADGE), / · 1 team needs a look$/);
      await waitForLibraryCheck(browser);
      // A sign-in that works closes the menu; open it to show the row.
      await setMenuOpen(true);
      await assertAccessible(browser, '37-teams-two-servers');
      await screenshot(browser, '37-teams-two-servers', {
        blankOut: [...ADDRESSES, '.team-question-server'],
      });
      await setMenuOpen(false);
    } finally {
      await gamma.stop();
    }
  });
});
