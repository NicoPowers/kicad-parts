// Team catalog journey: sign in to a disposable local team server, see a
// part's details from the team catalog, edit it, watch someone else's
// change arrive by itself, resolve a conflict, read the history with what
// changed, restore an earlier revision, and view as a read-only member.

import assert from 'node:assert/strict';
import { after, before, describe, it } from 'node:test';
import {
  FOOTPRINT,
  SYMBOL,
  assertAccessible,
  editAsSomeoneElse,
  launchApp,
  record,
  screenshot,
  searchFor,
  selectResult,
  startTeamServer,
  teamSignIn,
  textOf,
  waitForPreview,
  waitForText,
} from '../harness.mjs';

const IPN = 'RES-0000-0470';
const MPN = 'RC0603FR-0747RL';
const BADGE = '.team-settings > summary';
const COLUMN = '.part-fields';
const NOTICE = '.team-notice';

describe('team catalog: history, edits, conflicts and restore', () => {
  let server;
  let browser;
  let editor;
  let other;
  let viewer;
  let otherToken;

  before(async () => {
    server = await startTeamServer();
    [editor, other, viewer] = server.accounts;
    otherToken = await teamSignIn(server.url, other);
    ({ browser } = await launchApp({
      args: [`--team-server=${server.url}`, `--team-email=${editor.email}`],
    }));
  });

  after(async () => {
    await browser?.deleteSession();
    await server?.stop();
  });

  async function openTeamMenu() {
    const open = await browser.execute(() => document.querySelector('.team-settings')?.open === true);
    if (!open) await (await browser.$(BADGE)).click();
    await browser.waitUntil(
      async () => browser.execute(() => document.querySelector('.team-settings')?.open === true),
      { timeout: 5_000, timeoutMsg: 'the team catalog menu did not open' },
    );
  }

  async function signIn(account, password = account.password) {
    await openTeamMenu();
    const email = await browser.$('aria/Email');
    await email.setValue(account.email);
    await (await browser.$('aria/Password')).setValue(password);
    await (await browser.$('form[aria-label="Sign in to the team catalog"] button[type="submit"]')).click();
  }

  async function signOut() {
    await openTeamMenu();
    const buttons = await browser.$$('.team-settings-body button');
    for (const button of buttons) {
      if ((await button.getText()) === 'Sign out') {
        await button.click();
        break;
      }
    }
    await waitForText(browser, BADGE, 'Team catalog: not signed in');
  }

  async function clickButton(scope, text) {
    const buttons = await browser.$$(`${scope} button`);
    for (const button of buttons) {
      if ((await button.getText()).trim() === text && (await button.isDisplayed())) {
        await button.click();
        return;
      }
    }
    assert.fail(`no visible button "${text}" in ${scope}`);
  }

  /** The edit form input for the field labelled `name`. */
  async function fieldInput(name) {
    const id = await browser.execute(
      (label) =>
        [...document.querySelectorAll('.edit-form .edit-row label')].find((l) => l.textContent.trim() === label)
          ?.htmlFor ?? null,
      name,
    );
    assert.ok(id, `the edit form has a ${name} field`);
    return browser.$(`#${id}`);
  }

  async function sourceFact() {
    return browser.execute(() => {
      const fact = [...document.querySelectorAll('.fact')].find((f) => f.querySelector('dt')?.textContent === 'Source');
      return fact?.querySelector('dd')?.textContent ?? null;
    });
  }

  async function waitForRevision(number) {
    await browser.waitUntil(async () => (await sourceFact()) === `Team catalog, revision ${number}`, {
      timeout: 20_000,
      interval: 100,
      timeoutMsg: `the part never showed revision ${number}; last: ${JSON.stringify(await sourceFact())}`,
    });
  }

  async function fieldValue(name) {
    return browser.execute((label) => {
      const row = [...document.querySelectorAll('.part-fields tbody tr')].find(
        (tr) => tr.querySelector('th')?.textContent.trim() === label,
      );
      return row?.querySelector('td')?.textContent.trim() ?? null;
    }, name);
  }

  async function windowFits() {
    return browser.execute(() => {
      const fits = (el) => el.scrollHeight <= el.clientHeight + 1;
      const previews = document.querySelector('.part-previews');
      const fields = document.querySelector('.part-fields');
      return {
        windowFits: fits(document.scrollingElement),
        fieldsOnTheRight: fields.getBoundingClientRect().left >= previews.getBoundingClientRect().right,
      };
    });
  }

  it('starts signed out and explains how to sign in', async () => {
    await waitForText(browser, '#result-count', '178 matching parts');
    await waitForText(browser, BADGE, 'Team catalog: not signed in');
    await openTeamMenu();
    assert.equal(await (await browser.$('aria/Team server address')).getValue(), server.url);
    assert.equal(await (await browser.$('aria/Email')).getValue(), editor.email);
    assert.equal(await (await browser.$('aria/Password')).getAttribute('type'), 'password');
    assert.equal(await browser.execute(() => document.querySelectorAll('[role="alert"]').length), 0);
    await assertAccessible(browser, '40-team-sign-in');
    await screenshot(browser, '40-team-sign-in');
  });

  it('says plainly when the password is wrong', async () => {
    await signIn(editor, 'not-the-password');
    await waitForText(browser, '.team-sign-in [role="alert"]', 'The email or password is not right.');
    assert.equal(await (await browser.$('aria/Password')).getValue(), '', 'the password is cleared');
    assert.equal(await browser.execute(() => document.querySelectorAll('[role="alert"]').length), 1);
    await assertAccessible(browser, '41-team-sign-in-failed');
    await screenshot(browser, '41-team-sign-in-failed');
  });

  it('signs in and shows a part from the team catalog', async () => {
    await signIn(editor);
    await waitForText(browser, BADGE, `Team catalog · ${editor.name}`);
    await browser.waitUntil(
      async () => browser.execute((s) => document.querySelector(s)?.classList.contains('kicad-badge-ready'), BADGE),
      { timeout: 20_000, timeoutMsg: 'live updates never connected' },
    );
    await searchFor(browser, MPN, '1 matching part');
    await selectResult(browser, MPN);
    await waitForRevision(1);
    await waitForPreview(browser, SYMBOL);
    await waitForPreview(browser, FOOTPRINT);
    assert.equal(await textOf(browser, `${COLUMN} [role="tab"][aria-selected="true"]`), 'Details');
    assert.equal(await textOf(browser, `${COLUMN} [role="tab"][aria-selected="false"]`), 'History (1)');
    assert.equal(await textOf(browser, '#fields-heading'), 'Part details (6)');
    assert.equal(await fieldValue('Value'), '47');
    assert.match(await textOf(browser, '.manufacturer-parts'), /Yageo RC0603FR-0747RL \(main\)/);
    assert.match(await textOf(browser, COLUMN), /g-pas:R_US, revision 1/);
    const layout = await windowFits();
    record({ event: 'layout', state: 'team-details', ...layout });
    assert.deepEqual(layout, { windowFits: true, fieldsOnTheRight: true });
    await assertAccessible(browser, '42-team-part-details');
    await screenshot(browser, '42-team-part-details');
  });

  it('edits the part and saves a new revision', async () => {
    await clickButton(COLUMN, 'Edit details');
    await waitForText(browser, '#edit-heading', 'Edit details (starting from revision 1)');
    await (await fieldInput('Value')).setValue('47R');
    await (await browser.$('#edit-reason')).setValue('Write the value the way the BOM does');
    await assertAccessible(browser, '43-team-edit');
    await screenshot(browser, '43-team-edit');
    await clickButton('.edit-form', 'Save');
    await waitForText(browser, NOTICE, 'Saved as revision 2.');
    await waitForRevision(2);
    assert.equal(await fieldValue('Value'), '47R');
    assert.equal(await textOf(browser, `${COLUMN} [role="tab"][aria-selected="false"]`), 'History (2)');
  });

  it("shows someone else's change without being asked", async () => {
    await editAsSomeoneElse(server.url, otherToken, IPN, { Description: 'Thick film resistor, 47 ohm, 1%' }, 'Clearer description');
    await waitForRevision(3);
    await waitForText(browser, '.part-description', 'Thick film resistor, 47 ohm, 1%');
    // The results list shows the team catalog's description too.
    await waitForText(browser, '.results li .result-description', 'Thick film resistor, 47 ohm, 1%');
    // The earlier "saved" message would now be out of date.
    await browser.waitUntil(async () => (await browser.$$(NOTICE)).length === 0, {
      timeout: 5_000,
      timeoutMsg: 'the out-of-date saved message stayed',
    });
    assert.equal(await fieldValue('Description'), 'Thick film resistor, 47 ohm, 1%');
    await assertAccessible(browser, '44-team-live-update');
    await screenshot(browser, '44-team-live-update');
    // Searching uses the team catalog's details too: 47R and "thick" are
    // only in the team catalog, and the CSV-only "1/10W" no longer matches.
    await searchFor(browser, '47R thick', '1 matching part');
    await searchFor(browser, '1/10W 47', /^(No parts match\.|\d+ matching parts?)$/);
    assert.equal(
      await browser.execute(() => [...document.querySelectorAll('.results .result-ipn')].some((e) => e.textContent === 'RES-0000-0470')),
      false,
    );
    await searchFor(browser, MPN, '1 matching part');
  });

  it('shows both versions when someone else saved first, and saves on top', async () => {
    await clickButton(COLUMN, 'Edit details');
    await waitForText(browser, '#edit-heading', 'Edit details (starting from revision 3)');
    await (await fieldInput('Tolerance')).setValue('0.5%');
    await (await browser.$('#edit-reason')).setValue('Use the tighter tolerance part');
    // Meanwhile someone else changes the value.
    await editAsSomeoneElse(server.url, otherToken, IPN, { Value: '47.0' }, 'One decimal place');
    await waitForText(browser, '.edit-form .hint', /Someone else saved revision 4 while you were editing/);
    // What was typed survives their change arriving.
    assert.equal(await (await fieldInput('Tolerance')).getValue(), '0.5%');
    await clickButton('.edit-form', 'Save');
    await waitForText(browser, '#conflict-heading', 'Someone else changed this part first');
    const conflict = await browser.execute(() => {
      const section = document.querySelector('.conflict');
      return {
        role: section.getAttribute('role'),
        summary: section.querySelector('p').textContent,
        headers: [...section.querySelectorAll('thead th')].map((th) => th.textContent.trim()),
        rows: [...section.querySelectorAll('tbody tr')].map((tr) =>
          [...tr.children].map((cell) => cell.textContent.trim()),
        ),
        alerts: document.querySelectorAll('[role="alert"]').length,
        // Laid out at all? (A display rule can beat the hidden attribute.)
        tabsHidden: document.querySelector('.team-tabs').getClientRects().length === 0,
      };
    });
    record({ event: 'conflict', ...conflict });
    assert.equal(conflict.role, 'alert');
    assert.equal(conflict.alerts, 1, 'one alert for the conflict');
    assert.match(conflict.summary, new RegExp(`^${other.name} saved revision 4 on .* after you started editing revision 3\\. Your changes are not saved yet\\.$`));
    assert.deepEqual(conflict.headers, ['Field', 'Revision 3 (you started here)', 'Theirs (revision 4)', 'Yours']);
    assert.deepEqual(conflict.rows, [
      // You did not change the value, so yours says so; saving on top keeps
      // their 47.0.
      ['Value', '47R', '47.0', 'not changed'],
      ['Tolerance', '1%', '1%', '0.5%'],
    ]);
    assert.equal(conflict.tabsHidden, true);
    await assertAccessible(browser, '45-team-conflict');
    await screenshot(browser, '45-team-conflict');
    await clickButton('.conflict', 'Save my changes on top of theirs');
    await waitForText(browser, NOTICE, 'Saved as revision 5.');
    await waitForRevision(5);
    assert.equal(await fieldValue('Value'), '47.0', 'their value is kept');
    assert.equal(await fieldValue('Tolerance'), '0.5%', 'your change is added');
  });

  it('lists the history and what changed in each revision', async () => {
    await clickButton(COLUMN, 'History (5)');
    await browser.waitUntil(async () => (await browser.$$('.history li')).length === 5, {
      timeout: 5_000,
      timeoutMsg: 'the history did not list 5 revisions',
    });
    const titles = await browser.execute(() =>
      [...document.querySelectorAll('.history .history-title')].map((t) => t.textContent.trim()),
    );
    assert.deepEqual(titles.slice(0, 4), [
      `Revision 5 · Edited by ${editor.name} (current)`,
      `Revision 4 · Edited by ${other.name}`,
      `Revision 3 · Edited by ${other.name}`,
      `Revision 2 · Edited by ${editor.name}`,
    ]);
    // The import ran as an administrator account, which is not a team member.
    assert.equal(titles[4], 'Revision 1 · Imported by an administrator');
    assert.match(await textOf(browser, '.history li:nth-child(4) .history-reason'), /^“Write the value the way the BOM does”$/);
    await (await browser.$('.history li:nth-child(4) button')).click();
    await waitForText(browser, '#revision-heading', 'What changed in revision 2');
    assert.equal(
      await browser.execute(() => document.querySelector('.history li:nth-child(4) button').getAttribute('aria-pressed')),
      'true',
    );
    const rows = await browser.execute(() =>
      [...document.querySelectorAll('.revision .changes tbody tr')].map((tr) =>
        [...tr.children].map((cell) => cell.textContent.trim()),
      ),
    );
    assert.deepEqual(rows, [['Value', '47', '47R']]);
    await assertAccessible(browser, '46-team-history');
    await screenshot(browser, '46-team-history');
  });

  it('restores revision 1 as a new revision', async () => {
    const first = await browser.$('.history li:last-child button');
    await first.click();
    await waitForText(browser, '#revision-heading', 'What changed in revision 1');
    await clickButton('.revision', 'Restore revision 1…');
    await waitForText(browser, '#restore-heading', 'Restore revision 1');
    assert.match(
      await textOf(browser, '.restore-form p'),
      /^Restoring adds revision 6 with the details of revision 1\. No revision is deleted or changed\.$/,
    );
    const rows = await browser.execute(() =>
      [...document.querySelectorAll('.restore-form .changes tbody tr')].map((tr) =>
        [...tr.children].map((cell) => cell.textContent.trim()),
      ),
    );
    assert.deepEqual(rows, [
      ['Description', 'Thick film resistor, 47 ohm, 1%', 'RES 47 OHM 1% 1/10W 0603'],
      ['Value', '47.0', '47'],
      ['Tolerance', '0.5%', '1%'],
    ]);
    await (await browser.$('#restore-reason')).setValue('Go back to the imported details');
    await assertAccessible(browser, '47-team-restore');
    await screenshot(browser, '47-team-restore');
    await clickButton('.restore-form', 'Restore revision 1');
    await waitForText(browser, NOTICE, 'Restored revision 1 as revision 6.');
    await waitForRevision(6);
    assert.equal(
      await browser.execute(() => document.querySelector('.part-fields').scrollTop),
      0,
      'the column scrolls back to the message',
    );
    await waitForText(browser, '.history li:first-child .history-title', `Revision 6 · Restored by ${editor.name} (current)`);
    assert.match(await textOf(browser, '.revision'), /It copies the details of revision 1\./);
    await assertAccessible(browser, '48-team-restored');
    await screenshot(browser, '48-team-restored');
  });

  it('lets a read-only member see everything but not edit', async () => {
    await signOut();
    await signIn(viewer);
    await waitForText(browser, BADGE, `Team catalog · ${viewer.name}`);
    await waitForRevision(6);
    await waitForText(browser, '.read-only', 'You can view this part but not edit it.');
    assert.equal((await browser.$$(NOTICE)).length, 0, "no message left from the editor's session");
    const buttons = await browser.execute(() =>
      [...document.querySelectorAll('.part-fields button')].map((b) => b.textContent.trim()),
    );
    assert.ok(!buttons.includes('Edit details'), `no edit button: ${buttons}`);
    await clickButton(COLUMN, 'History (6)');
    await browser.waitUntil(async () => (await browser.$$('.history li')).length === 6, { timeout: 5_000 });
    const second = await browser.$('.history li:nth-child(2) button');
    await second.click();
    await waitForText(browser, '#revision-heading', 'What changed in revision 5');
    assert.equal(await browser.execute(() => document.querySelectorAll('.restore-open').length), 0);
    await clickButton(COLUMN, 'Details');
    await assertAccessible(browser, '49-team-read-only');
    await screenshot(browser, '49-team-read-only');
  });

  it('says when a part is not in the team catalog', async () => {
    await searchFor(browser, 'KF33BD', '1 matching part');
    await selectResult(browser, 'KF33BD-TR');
    await waitForText(browser, '.part-fields .hint', 'This part is not in the team catalog yet, so it has no history.');
    assert.match(await textOf(browser, '#fields-heading'), /^Catalog fields \(\d+\)$/);
  });
});
