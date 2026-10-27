// Team catalog journey: sign in to a disposable local team server, see a
// part's details and current revision from the team catalog, edit its
// details (which adds no revision), watch someone else's change arrive by
// itself, resolve a conflict, read the history with what changed, and view
// as a read-only member. Changing the symbol adds a revision, and archiving
// it makes the earlier revision current again. There is no Restore: an
// earlier revision is never copied back (kicad-parts-9296).

import assert from 'node:assert/strict';
import { after, before, describe, it } from 'node:test';
import {
  FOOTPRINT,
  SYMBOL,
  assertAccessible,
  chooseInLibraries,
  editAsSomeoneElse,
  launchApp,
  record,
  screenshot,
  searchFor,
  selectResult,
  startTeamServer,
  teamSignIn,
  textOf,
  waitForLiveUpdates,
  waitForPreview,
  waitForPreviewImages,
  waitForLibraries,
  waitForText,
} from '../harness.mjs';

const IPN = 'RES-0000-0470';
const MPN = 'RC0603FR-0747RL';
const BADGE = '.team-settings > summary';
const COLUMN = '.part-fields';
const NOTICE = '.team-notice';
/** The server address holds a port that changes from run to run. */
const SERVER_ADDRESS = '#team-server';

describe('team catalog: history, edits and conflicts', () => {
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

  /** Waits until the History tab counts `count` entries. */
  async function waitForHistory(count) {
    await browser.waitUntil(
      async () =>
        (await browser.execute(() => document.querySelector('#team-tab-history')?.textContent.trim())) ===
        `History (${count})`,
      { timeout: 20_000, interval: 100, timeoutMsg: `the history never counted ${count}` },
    );
  }

  /** Asserts that the right-hand column is scrolled to its top, where the notice is. */
  async function columnAtTop() {
    assert.equal(
      await browser.execute(() => document.querySelector('.part-fields').scrollTop),
      0,
      'the column scrolls back to the message',
    );
  }

  async function waitForRevision(number) {
    await browser.waitUntil(async () => (await sourceFact()) === `Team catalog, revision ${number} of this part`, {
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
    // Without a team there are no parts, and the list says how to get some.
    await waitForText(browser, '#result-count', 'Sign in to a team to see its parts.');
    await waitForText(browser, BADGE, 'Team catalog: not signed in');
    await openTeamMenu();
    assert.equal(await (await browser.$('aria/Team server address')).getValue(), server.url);
    assert.equal(await (await browser.$('aria/Email')).getValue(), editor.email);
    assert.equal(await (await browser.$('aria/Password')).getAttribute('type'), 'password');
    assert.equal(await browser.execute(() => document.querySelectorAll('[role="alert"]').length), 0);
    await assertAccessible(browser, '40-team-sign-in');
    await screenshot(browser, '40-team-sign-in', { blankOut: [SERVER_ADDRESS] });
  });

  it('says plainly when the password is wrong', async () => {
    await signIn(editor, 'not-the-password');
    await waitForText(browser, '.team-sign-in [role="alert"]', 'The email or password is not right.');
    assert.equal(await (await browser.$('aria/Password')).getValue(), '', 'the password is cleared');
    assert.equal(await browser.execute(() => document.querySelectorAll('[role="alert"]').length), 1);
    await assertAccessible(browser, '41-team-sign-in-failed');
    await screenshot(browser, '41-team-sign-in-failed', { blankOut: [SERVER_ADDRESS] });
  });

  it('refuses a plain http:// address for another computer, before sending the password', async () => {
    const address = await browser.$(SERVER_ADDRESS);
    await address.setValue('http://team.example.com:8090');
    await signIn(editor);
    await waitForText(
      browser,
      '.team-sign-in [role="alert"]',
      'Addresses starting with http:// send your password unencrypted, so they only work for a server on this ' +
        'computer. Use an https:// address, such as https://team.example.com.',
    );
    assert.equal(await (await browser.$('aria/Password')).getValue(), '', 'the password is cleared');
    await assertAccessible(browser, '99-team-sign-in-plain-http');
    // The address has no port that changes, so nothing is blanked out.
    await screenshot(browser, '99-team-sign-in-plain-http');
    await address.setValue(server.url);
  });

  it('signs in and shows a part from the team catalog', async () => {
    await signIn(editor);
    await waitForText(browser, BADGE, `Team catalog · ${editor.name}`);
    await waitForLiveUpdates(browser);
    await searchFor(browser, MPN, '1 matching part');
    await selectResult(browser, MPN);
    await waitForRevision(1);
    await waitForPreview(browser, SYMBOL);
    await waitForPreview(browser, FOOTPRINT);
    assert.equal(await textOf(browser, `${COLUMN} [role="tab"][aria-selected="true"]`), 'Details');
    assert.equal(await textOf(browser, `${COLUMN} [role="tab"][aria-selected="false"]`), 'History (1)');
    // Symbol and Footprint belong to the revision, so the details list the
    // other four fields.
    assert.equal(await textOf(browser, '#fields-heading'), 'Part details (4)');
    assert.equal(await fieldValue('Value'), '47');
    assert.match(await textOf(browser, '.manufacturer-parts'), /Yageo RC0603FR-0747RL \(main\)/);
    assert.equal(await textOf(browser, '#cad-items-heading'), 'Symbol and footprint (revision 1 of this part)');
    // The symbol's and footprint's own revisions say which they are.
    assert.match(await textOf(browser, COLUMN), /g-res:R_US, symbol revision 1/);
    assert.match(await textOf(browser, COLUMN), /g-Resistor_SMD:R_0603_1608Metric, footprint revision 1/);
    await waitForText(browser, `${SYMBOL} .provenance`, /^team catalog, symbol revision 1 · /);
    const layout = await windowFits();
    record({ event: 'layout', state: 'team-details', ...layout });
    assert.deepEqual(layout, { windowFits: true, fieldsOnTheRight: true });
    await assertAccessible(browser, '42-team-part-details');
    await screenshot(browser, '42-team-part-details');
  });

  it('edits the part details, which adds no revision', async () => {
    await clickButton(COLUMN, 'Edit part');
    await waitForText(browser, '#edit-heading', 'Edit part');
    assert.match(
      await textOf(browser, '.edit-form'),
      /Change details only to correct them\. A part with a different value, package or pinout needs a new IPN\./,
    );
    const legends = await browser.execute(() =>
      [...document.querySelectorAll('.edit-form legend')].map((legend) => legend.textContent.trim()),
    );
    assert.deepEqual(legends, ['Part details', 'Manufacturer parts (the first is the main one)', 'Symbol and footprint']);
    assert.match(
      await textOf(browser, '.edit-form fieldset:last-of-type .hint'),
      /^Changing these adds a new revision of this part\. Designs that use revision 1 keep it\.$/,
    );
    await (await fieldInput('Value')).setValue('47R');
    await (await browser.$('#edit-reason')).setValue('Write the value the way the BOM does');
    // Show the top of the form, with the new-IPN hint, in the screenshot.
    await browser.execute(() => {
      document.querySelector('.part-fields').scrollTop = 0;
    });
    await assertAccessible(browser, '43-team-edit');
    await screenshot(browser, '43-team-edit');
    await clickButton('.edit-form', 'Save');
    await waitForText(browser, NOTICE, 'Saved. The part details changed; no revision was added.');
    await columnAtTop();
    await waitForHistory(2);
    assert.equal(await sourceFact(), 'Team catalog, revision 1 of this part');
    assert.equal(await fieldValue('Value'), '47R');
  });

  it("shows someone else's change without being asked", async () => {
    await editAsSomeoneElse(server.url, otherToken, IPN, { Description: 'Thick film resistor, 47 ohm, 1%' }, 'Clearer description');
    await waitForHistory(3);
    await waitForText(browser, '.part-description', 'Thick film resistor, 47 ohm, 1%');
    assert.equal(await sourceFact(), 'Team catalog, revision 1 of this part');
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
    // Searching uses the team catalog's details: 47R and "thick" are new,
    // and "1/10W", only in the old description, no longer matches.
    await searchFor(browser, '47R thick', '1 matching part');
    await searchFor(browser, '1/10W 47', /^(No parts match\.|\d+ matching parts?)$/);
    assert.equal(
      await browser.execute(() => [...document.querySelectorAll('.results .result-ipn')].some((e) => e.textContent === 'RES-0000-0470')),
      false,
    );
    await searchFor(browser, MPN, '1 matching part');
  });

  it('shows both sets of changes when someone else saved first, and saves on top', async () => {
    await clickButton(COLUMN, 'Edit part');
    await waitForText(browser, '#edit-heading', 'Edit part');
    await (await fieldInput('Tolerance')).setValue('0.5%');
    await (await browser.$('#edit-reason')).setValue('Use the tighter tolerance part');
    // Meanwhile someone else changes the value.
    await editAsSomeoneElse(server.url, otherToken, IPN, { Value: '47.0' }, 'One decimal place');
    await browser.waitUntil(
      async () =>
        browser.execute(() =>
          [...document.querySelectorAll('.edit-form .hint')].some((hint) =>
            hint.textContent.startsWith('Someone else saved a change to this part while you were editing.'),
          ),
        ),
      { timeout: 20_000, timeoutMsg: 'the edit form never said someone else saved a change' },
    );
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
    assert.match(conflict.summary, new RegExp(`^${other.name} saved a change on .* after you started editing\\. Your changes are not saved yet\\.$`));
    assert.deepEqual(conflict.headers, ['Field', 'You started from', 'Theirs', 'Yours']);
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
    await waitForText(browser, NOTICE, 'Saved. The part details changed; no revision was added.');
    await columnAtTop();
    await waitForHistory(5);
    assert.equal(await fieldValue('Value'), '47.0', 'their value is kept');
    assert.equal(await fieldValue('Tolerance'), '0.5%', 'your change is added');
  });

  it('lists the history and what changed in each entry', async () => {
    await clickButton(COLUMN, 'History (5)');
    await browser.waitUntil(async () => (await browser.$$('.history li')).length === 5, {
      timeout: 5_000,
      timeoutMsg: 'the history did not list 5 entries',
    });
    const titles = await browser.execute(() =>
      [...document.querySelectorAll('.history .history-title')].map((t) => t.textContent.trim()),
    );
    // Details changes add no revision; the import added revision 1, still
    // the current one. The import ran as an administrator account, which is
    // not a team member.
    assert.deepEqual(titles, [
      `Part details · Edited by ${editor.name}`,
      `Part details · Edited by ${other.name}`,
      `Part details · Edited by ${other.name}`,
      `Part details · Edited by ${editor.name}`,
      'Revision 1 (current) · Added by an administrator',
    ]);
    assert.match(await textOf(browser, '.history li:nth-child(4) .history-reason'), /^“Write the value the way the BOM does”$/);
    await (await browser.$('.history li:nth-child(4) button')).click();
    await waitForText(browser, '#revision-heading', 'What changed in the part details');
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
    const revisionButtons = await browser.execute(() =>
      [...document.querySelectorAll('.revision button')].map((b) => b.textContent.trim()),
    );
    assert.deepEqual(revisionButtons, [], 'an earlier revision offers no restore');
    await assertAccessible(browser, '46-team-history');
    await screenshot(browser, '46-team-history');
  });

  it('changes the symbol, which adds a revision', async () => {
    await clickButton(COLUMN, 'Details');
    await clickButton(COLUMN, 'Edit part');
    await waitForText(browser, '#edit-heading', 'Edit part');
    // KiCad's own resistor symbol draws the same two-pin resistor, so the
    // part can still replace the old one: a new revision, not a new IPN.
    // No CAD name is typed (kicad-parts-bc6f): the Symbol is read only, and
    // Choose... opens the Libraries browser on the current symbol.
    assert.equal(await browser.execute(() => document.getElementById('edit-cad-0').readOnly), true);
    await browser.execute(() => document.getElementById('edit-cad-0-choose').scrollIntoView({ block: 'center' }));
    await (await browser.$('#edit-cad-0-choose')).click();
    await waitForLibraries(browser);
    await waitForText(browser, '#library-detail-title', 'g-res:R_US');
    await chooseInLibraries(browser, { group: 'kicad', library: 'Device', name: 'R' });
    await browser.waitUntil(async () => (await browser.execute(() => document.getElementById('edit-cad-0').value)) === 'Device:R', {
      timeout: 20_000,
      timeoutMsg: `the Symbol did not become Device:R: ${await textOf(browser, '.edit-form')}`,
    });
    await waitForText(browser, '.edit-form .file-chosen .file-name', /^KiCad's own: the part refers to it in KiCad's library, which every engineer has\./);
    await browser.waitUntil(async () => (await browser.execute(() => document.activeElement?.id)) === 'edit-cad-0-choose', {
      timeout: 5_000,
      timeoutMsg: 'focus did not come back to Choose…',
    });
    await (await browser.$('#edit-reason')).setValue("Use KiCad's resistor symbol");
    await clickButton('.edit-form', 'Save');
    await waitForText(browser, NOTICE, 'Saved as revision 2.');
    await columnAtTop();
    await waitForRevision(2);
    await waitForHistory(6);
    assert.equal(await textOf(browser, '#cad-items-heading'), 'Symbol and footprint (revision 2 of this part)');
    assert.match(await textOf(browser, COLUMN), /Device:R, KiCad library/);
    // KiCad's own library is not read yet, so the app does not claim to know.
    assert.equal(
      await fieldValue('Excluded from BOM'),
      "Not known: the symbol comes from KiCad's own library, which this app does not read yet",
    );
    await waitForPreview(browser, SYMBOL);
    await clickButton(COLUMN, 'History (6)');
    await browser.waitUntil(async () => (await browser.$$('.history li')).length === 6, { timeout: 5_000 });
    const titles = await browser.execute(() =>
      [...document.querySelectorAll('.history .history-title')].map((t) => t.textContent.trim()),
    );
    assert.deepEqual(titles, [
      `Revision 2 (current) · Edited by ${editor.name}`,
      `Part details · Edited by ${editor.name}`,
      `Part details · Edited by ${other.name}`,
      `Part details · Edited by ${other.name}`,
      `Part details · Edited by ${editor.name}`,
      'Revision 1 · Added by an administrator',
    ]);
    await waitForText(browser, '#revision-heading', 'What changed in revision 2');
    const rows = await browser.execute(() =>
      [...document.querySelectorAll('.revision .changes tbody tr')].map((tr) =>
        [...tr.children].map((cell) => cell.textContent.trim()),
      ),
    );
    assert.deepEqual(rows, [['Symbol', 'g-res:R_US', 'Device:R']]);
    // The notice that a revision was added stays above the history.
    assert.equal(await textOf(browser, NOTICE), 'Saved as revision 2.');
    await assertAccessible(browser, '47-team-new-revision');
    await screenshot(browser, '47-team-new-revision');
  });

  it('archives the new revision, which makes revision 1 current again', async () => {
    // The newest entry, revision 2, is selected.
    await clickButton('.revision', 'Archive revision 2…');
    await waitForText(browser, '#archive-heading', 'Archive revision 2');
    // Keyboard focus moves to the reason box.
    await browser.waitUntil(async () => (await browser.execute(() => document.activeElement?.id)) === 'archive-reason', {
      timeout: 5_000,
      timeoutMsg: 'focus did not move to the reason box',
    });
    assert.equal(
      await textOf(browser, '.archive-form p'),
      'This app stops offering revision 2, and KiCad lists it under Archived revisions, so nobody new picks it by mistake. Designs that already use it keep it. Nothing is deleted. The current revision becomes revision 1.',
    );
    await (await browser.$('#archive-reason')).setValue("KiCad's symbol confuses the assembly house");
    await assertAccessible(browser, '48-team-archive');
    await screenshot(browser, '48-team-archive');
    await clickButton('.archive-form', 'Archive revision 2');
    await waitForText(browser, NOTICE, 'Revision 2 is archived. The current revision is now revision 1.');
    await columnAtTop();
    await waitForRevision(1);
    await waitForHistory(7);
    const titles = await browser.execute(() =>
      [...document.querySelectorAll('.history .history-title')].map((t) => t.textContent.trim()),
    );
    assert.deepEqual(titles, [
      `Revision 2 · Archived by ${editor.name}`,
      `Revision 2 (archived) · Edited by ${editor.name}`,
      `Part details · Edited by ${editor.name}`,
      `Part details · Edited by ${other.name}`,
      `Part details · Edited by ${other.name}`,
      `Part details · Edited by ${editor.name}`,
      'Revision 1 (current) · Added by an administrator',
    ]);
    await waitForText(browser, '#revision-heading', 'Revision 2 archived');
    assert.equal(
      await textOf(browser, '.revision .hint'),
      'Revision 2 was archived. The current revision became revision 1.',
    );
    // Undo is right there, on the entry just made.
    assert.equal(
      await browser.execute(() => [...document.querySelectorAll('.revision button')].map((b) => b.textContent.trim()).join()),
      'Unarchive revision 2…',
    );
    await assertAccessible(browser, '53-team-archived');
    await screenshot(browser, '53-team-archived');
    // The only revision that is not archived cannot be archived: the app
    // says so before anyone tries.
    await (await browser.$('.history li:last-child button')).click();
    await waitForText(browser, '#revision-heading', 'What changed in revision 1');
    assert.equal(
      await browser.execute(() => [...document.querySelectorAll('.revision button')].map((b) => b.textContent.trim()).join()),
      '',
    );
    assert.match(
      await textOf(browser, '.revision'),
      /Revision 1 is the only revision that is not archived, so it cannot be archived\. Add a corrected revision first\./,
    );
    // The archived revision offers to unarchive it.
    await (await browser.$('.history li:nth-child(2) button')).click();
    await waitForText(browser, '#revision-heading', 'What changed in revision 2');
    await clickButton('.revision', 'Unarchive revision 2…');
    assert.equal(
      await textOf(browser, '.archive-form p'),
      'This app offers revision 2 again, and KiCad no longer lists it under Archived revisions. The current revision becomes revision 2.',
    );
    await clickButton('.archive-form', 'Cancel');
    // Focus goes back to the button that opened the form.
    await browser.waitUntil(async () => (await browser.execute(() => document.activeElement?.id)) === 'archive-open-2', {
      timeout: 5_000,
      timeoutMsg: 'focus did not return to the button',
    });
    // The Details tab shows revision 1 again and names the archived one.
    await clickButton(COLUMN, 'Details');
    assert.equal(await textOf(browser, '#cad-items-heading'), 'Symbol and footprint (revision 1 of this part)');
    assert.equal(await textOf(browser, '.other-revisions'), 'Other revisions: revision 2 (archived).');
  });

  it('lets a read-only member see everything but not edit', async () => {
    await signOut();
    await signIn(viewer);
    await waitForText(browser, BADGE, `Team catalog · ${viewer.name}`);
    await waitForLiveUpdates(browser);
    await waitForRevision(1);
    await waitForHistory(7);
    await waitForText(browser, '.read-only', 'You can view this part but not edit it.');
    assert.equal((await browser.$$(NOTICE)).length, 0, "no message left from the editor's session");
    const buttons = await browser.execute(() =>
      [...document.querySelectorAll('.part-fields button')].map((b) => b.textContent.trim()),
    );
    assert.ok(!buttons.includes('Edit part'), `no edit button: ${buttons}`);
    await clickButton(COLUMN, 'History (7)');
    await browser.waitUntil(async () => (await browser.$$('.history li')).length === 7, { timeout: 5_000 });
    const second = await browser.$('.history li:nth-child(3) button');
    await second.click();
    await waitForText(browser, '#revision-heading', 'What changed in the part details');
    assert.equal(await browser.execute(() => document.querySelectorAll('.revision button').length), 0);
    const last = await browser.$('.history li:last-child button');
    await last.click();
    await waitForText(browser, '#revision-heading', 'What changed in revision 1');
    // The sample part was added from the sample library's files.
    assert.equal(await textOf(browser, '.revision .hint'), 'The first revision of this part, made from these files:');
    await clickButton(COLUMN, 'Details');
    // The previews redraw after signing in again. Until they do, the cards
    // from the signed-out moment ("Sign in to see this symbol") are still
    // shown, so wait for the drawn images themselves.
    await waitForPreviewImages(browser, SYMBOL);
    await waitForPreviewImages(browser, FOOTPRINT);
    await assertAccessible(browser, '49-team-read-only');
    await screenshot(browser, '49-team-read-only');
  });
});
