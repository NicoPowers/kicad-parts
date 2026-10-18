// Several teams journey (kicad-parts-f10d): the person works for two
// clients, each its own team on its own server. They sign in to one, add
// the other without signing out, and switch between them from the
// keyboard with no sign-in. When one server stops, that team shows its
// last-known parts marked offline while the other keeps working. Signing
// out and removing touch one team only. Without a credential store, the
// Teams menu says once why the app will ask again after a restart.
// Sign-ins stay in the app's memory (--sign-ins), never the person's
// credential store.

import assert from 'node:assert/strict';
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
  textOf,
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
    // With one team signed in, there is nothing to switch to.
    assert.equal(await browser.execute(() => document.querySelector('#team-shown')), null);
    await (await browser.$('#team-add')).click();
    assert.equal(
      await browser.execute(() => document.activeElement?.id),
      'team-server',
      'Add a team puts the cursor in the address',
    );
    await signInWithForm(beta, beta.accounts[0]);
    await waitForText(browser, BADGE, 'Teams · 2 signed in');
    await waitForLiveUpdates(browser);
    await setMenuOpen(true);
    await browser.waitUntil(async () => (await rowText('BE')) !== null, { timeout: 20_000 });
    assert.match(await rowText('AC'), /^Signed in as .+ You can edit parts\. Changes others make appear here by themselves\.$/);
    assert.match(await rowText('BE'), /^Signed in as /);
    assert.equal(await textOf(browser, '#teams-heading'), 'Your teams');
    // The switcher shows the team signed in last.
    assert.equal(await (await browser.$('#team-shown')).getValue(), beta.url);
    await assertAccessible(browser, '31-teams-two-signed-in');
    await screenshot(browser, '31-teams-two-signed-in', { blankOut: ADDRESSES });
    await setMenuOpen(false);
  });

  it('switches teams from the keyboard, with no sign-in', async () => {
    await searchFor(browser, MPN, '1 matching part');
    await selectResult(browser, MPN);
    await waitForValue('47');
    const switcher = await browser.$('#team-shown');
    assert.equal(await (await browser.$('label[for="team-shown"]')).getText(), 'Team');
    // Move to the first team with the arrow key, as a person would.
    await browser.execute(() => document.querySelector('#team-shown').focus());
    await browser.keys('ArrowUp');
    await browser.waitUntil(async () => (await switcher.getValue()) === acme.url, {
      timeout: 5_000,
      timeoutMsg: 'the arrow key did not choose the first team',
    });
    // The part list reloads for acme; the switcher keeps the focus.
    await waitForValue('47R');
    assert.equal(await browser.execute(() => document.activeElement?.id), 'team-shown');
    assert.equal(await textOf(browser, BADGE), 'Teams · 2 signed in');
    await waitForPreview(browser, SYMBOL);
    await waitForPreview(browser, FOOTPRINT);
    await assertAccessible(browser, '32-teams-switched');
    await screenshot(browser, '32-teams-switched');
  });

  it('marks a team offline when its server stops, and keeps the other working', async () => {
    await beta.stop();
    await waitForText(browser, BADGE, 'Teams · 2 signed in · 1 offline', 30_000);
    // Acme, shown, keeps working.
    await selectResult(browser, MPN);
    await waitForValue('47R');
    await waitForPreview(browser, SYMBOL);
    await waitForPreview(browser, FOOTPRINT);
    await setMenuOpen(true);
    assert.match(await rowText('BE'), /^Offline: showing the parts last seen\. The app reconnects by itself\./);
    await assertAccessible(browser, '33-teams-offline');
    // Beta's last problem holds the system's own words, which differ
    // between machines.
    const betaProblem = '.team-row:nth-child(2) .team-state';
    assert.match(await textOf(browser, betaProblem), /^Offline/);
    await screenshot(browser, '33-teams-offline', { blankOut: [...ADDRESSES, betaProblem] });
    // Beta's last-known parts are still there to show.
    await browser.execute((server) => {
      const select = document.querySelector('#team-shown');
      select.value = server;
      select.dispatchEvent(new Event('change'));
    }, beta.url);
    await setMenuOpen(false);
    await selectResult(browser, MPN);
    await waitForValue('47');
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
    await clickInRow('BE', 'Remove');
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
});
