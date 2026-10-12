// Team catalog edge cases a team hits in daily use: unusual and very long
// values (units, right-to-left text, emoji, an unbroken token) shown exactly
// and kept inside the window at its narrowest width; a save that only adds
// spaces; archiving a revision again after someone else unarchived it; a
// change someone else already saved; and a save while the team server is
// stopped.

import assert from 'node:assert/strict';
import { after, before, describe, it } from 'node:test';
import {
  archiveAsSomeoneElse,
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
  waitForLiveUpdates,
  waitForText,
} from '../harness.mjs';

const IPN = 'RES-0000-0470';
const MPN = 'RC0603FR-0747RL';
const BADGE = '.team-settings > summary';
const COLUMN = '.part-fields';
const NOTICE = '.team-notice';
const PROBLEM = '.team-problem';

// An unbroken token as long as a pasted distributor URL path or a part
// number list, plus units, right-to-left text, an accent and an emoji.
const TOKEN = `CRCW0603-${'47R0FKEA'.repeat(18)}`;
const DESCRIPTION = `Résistance 47 Ω ±1 % — נגד 47 אוהם 🔌 ${TOKEN}`;
const VALUE = '4.7 µΩ';
const MANUFACTURER = 'Ünïcödé Électronique';
const LONG_MPN = `LONG-${'0123456789'.repeat(8)}`;

describe('team catalog: edge cases', () => {
  let server;
  let browser;
  let editor;
  let other;
  let otherToken;

  before(async () => {
    server = await startTeamServer();
    [editor, other] = server.accounts;
    otherToken = await teamSignIn(server.url, other);
    ({ browser } = await launchApp({
      args: [`--team-server=${server.url}`, `--team-email=${editor.email}`],
    }));
  });

  after(async () => {
    await browser?.deleteSession();
    await server?.stop();
  });

  async function signIn(account) {
    await (await browser.$(BADGE)).click();
    await (await browser.$('aria/Password')).setValue(account.password);
    await (await browser.$('form[aria-label="Sign in to the team catalog"] button[type="submit"]')).click();
    await waitForText(browser, BADGE, `Team catalog · ${account.name}`);
    await waitForLiveUpdates(browser);
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

  async function fieldValue(name) {
    return browser.execute((label) => {
      const row = [...document.querySelectorAll('.part-fields tbody tr')].find(
        (tr) => tr.querySelector('th')?.textContent.trim() === label,
      );
      return row?.querySelector('td')?.textContent.trim() ?? null;
    }, name);
  }

  async function sourceFact() {
    return browser.execute(() => {
      const fact = [...document.querySelectorAll('.fact')].find((f) => f.querySelector('dt')?.textContent === 'Source');
      return fact?.querySelector('dd')?.textContent ?? null;
    });
  }

  async function waitForRevision(number) {
    await browser.waitUntil(async () => (await sourceFact()) === `Team catalog, revision ${number} of this part`, {
      timeout: 20_000,
      interval: 100,
      timeoutMsg: `the part never showed revision ${number}; last: ${JSON.stringify(await sourceFact())}`,
    });
  }

  async function historyCount() {
    const text = await textOf(browser, '#team-tab-history');
    return Number(/\((\d+)\)/.exec(text ?? '')?.[1] ?? NaN);
  }

  async function waitForHistory(count) {
    await browser.waitUntil(async () => (await historyCount()) === count, {
      timeout: 20_000,
      interval: 100,
      timeoutMsg: `the history never counted ${count}`,
    });
  }

  /** What sticks out to the right of the window or of the part panel. */
  async function overflow() {
    return browser.execute(() => {
      const panel = document.querySelector('.part-panel');
      const scrolls = (el) => el.scrollWidth > el.clientWidth + 1;
      return {
        windowScrolls: scrolls(document.scrollingElement),
        scrollsSideways: [panel, ...panel.querySelectorAll('.part-previews, .part-fields')]
          .filter(scrolls)
          .map((el) => el.className),
        stickingOut: [...panel.querySelectorAll('.part-header, .part-header *, .part-fields *, .part-previews *')]
          .filter((el) => {
            if (el.getClientRects().length === 0) return false;
            // Its box, or text running out of its box (a block's box keeps
            // its width while a long word runs past it).
            const box = el.closest('.part-fields, .part-previews') ?? panel;
            const outside = el.getBoundingClientRect().right > box.getBoundingClientRect().right + 1;
            // A text box scrolls its own text, and a clipped box hides it.
            const clipped = getComputedStyle(el).overflowX === 'hidden' || el.matches('input, textarea');
            return outside || (!clipped && el.clientWidth > 0 && scrolls(el));
          })
          .map((el) => `${el.tagName.toLowerCase()}.${el.className}`)
          .slice(0, 5),
      };
    });
  }

  const FITS = { windowScrolls: false, scrollsSideways: [], stickingOut: [] };

  it('saves unusual and very long values exactly', async () => {
    await waitForText(browser, '#result-count', '178 matching parts');
    await signIn(editor);
    await searchFor(browser, MPN, '1 matching part');
    await selectResult(browser, MPN);
    await waitForRevision(1);
    await clickButton(COLUMN, 'Edit part');
    await waitForText(browser, '#edit-heading', 'Edit part');
    await (await fieldInput('Description')).setValue(DESCRIPTION);
    await (await fieldInput('Value')).setValue(VALUE);
    await clickButton('.edit-form', 'Add a manufacturer part');
    await (await browser.$('aria/Manufacturer 2')).setValue(MANUFACTURER);
    await (await browser.$('aria/MPN 2')).setValue(LONG_MPN);
    await (await browser.$('#edit-reason')).setValue('Names as the supplier writes them — ±1 %');
    await clickButton('.edit-form', 'Save');
    await waitForText(browser, NOTICE, 'Saved. The part details changed; no revision was added.');
    await waitForHistory(2);
    assert.equal(await fieldValue('Description'), DESCRIPTION);
    assert.equal(await fieldValue('Value'), VALUE);
    assert.match(await textOf(browser, '.manufacturer-parts'), new RegExp(`${MANUFACTURER} ${LONG_MPN}$`));
    // The part header and the results list show the new description too.
    await waitForText(browser, '.part-description', DESCRIPTION);
    await waitForText(browser, '.results li .result-description', DESCRIPTION);
    const wide = await overflow();
    record({ event: 'overflow', state: 'details at 1280', ...wide });
    assert.deepEqual(wide, FITS);
    await browser.execute(() => {
      document.querySelector('.part-fields').scrollTop = 0;
    });
    await assertAccessible(browser, '56-team-long-values');
    await screenshot(browser, '56-team-long-values');
    // Searching finds the part by the new text, accents and all.
    await searchFor(browser, 'résistance µΩ', '1 matching part');
  });

  it('keeps long values inside the window at its narrowest width', async () => {
    const before = await browser.getWindowSize();
    try {
      // The window cannot be made narrower than 960 pixels.
      await browser.setWindowSize(960, before.height);
      await browser.waitUntil(async () => (await browser.execute(() => window.innerWidth)) < 1000, {
        timeout: 5_000,
        timeoutMsg: 'the window did not get narrower',
      });
      const width = await browser.execute(() => window.innerWidth);
      record({ event: 'narrow-window', width });
      const details = await overflow();
      record({ event: 'overflow', state: 'details', ...details });
      assert.deepEqual(details, FITS, 'details');
      await clickButton(COLUMN, 'History (2)');
      await waitForText(browser, '#revision-heading', 'What changed in the part details');
      const history = await overflow();
      record({ event: 'overflow', state: 'history', ...history });
      assert.deepEqual(history, FITS, 'history');
      await clickButton(COLUMN, 'Details');
      await clickButton(COLUMN, 'Edit part');
      await waitForText(browser, '#edit-heading', 'Edit part');
      const form = await overflow();
      record({ event: 'overflow', state: 'edit form', ...form });
      assert.deepEqual(form, FITS, 'edit form');
      await clickButton('.edit-form', 'Cancel');
    } finally {
      await browser.setWindowSize(before.width, before.height);
    }
  });

  it('saves nothing when only spaces were added', async () => {
    await clickButton(COLUMN, 'Edit part');
    await waitForText(browser, '#edit-heading', 'Edit part');
    await (await fieldInput('Value')).setValue(`  ${VALUE}  `);
    await (await browser.$('#edit-reason')).setValue('Tidy');
    await clickButton('.edit-form', 'Save');
    await waitForText(browser, NOTICE, 'Nothing changed, so nothing was saved.');
    assert.equal(await historyCount(), 2, 'no new history entry');
    await clickButton('.edit-form', 'Cancel');
    assert.equal(await fieldValue('Value'), VALUE);
  });

  it('archives a revision again after someone else unarchived it', async () => {
    await editAsSomeoneElse(server.url, otherToken, IPN, { Symbol: 'Device:R' }, "Use KiCad's resistor symbol");
    await waitForRevision(2);
    await waitForHistory(3);
    await clickButton(COLUMN, 'History (3)');
    await waitForText(browser, '#revision-heading', 'What changed in revision 2');
    await clickButton('.revision', 'Archive revision 2…');
    await (await browser.$('#archive-reason')).setValue('Wrong symbol for the assembly house');
    await clickButton('.archive-form', 'Archive revision 2');
    await waitForText(browser, NOTICE, 'Revision 2 is archived. The current revision is now revision 1.');
    await waitForRevision(1);
    await waitForHistory(4);
    // Someone else needs revision 2 after all.
    await archiveAsSomeoneElse(server.url, otherToken, IPN, 2, false, 'Needed for the prototype');
    await waitForRevision(2);
    await waitForHistory(5);
    // Archive it again, for the same reason as before.
    const entry = await browser.execute(
      () =>
        [...document.querySelectorAll('.history li')].findIndex((li) =>
          li.querySelector('.history-title')?.textContent.startsWith('Revision 2 (current) · Edited by'),
        ) + 1,
    );
    assert.ok(entry > 0, 'the history lists revision 2');
    await (await browser.$(`.history li:nth-child(${entry}) button`)).click();
    await waitForText(browser, '#revision-heading', 'What changed in revision 2');
    await clickButton('.revision', 'Archive revision 2…');
    await (await browser.$('#archive-reason')).setValue('Wrong symbol for the assembly house');
    await clickButton('.archive-form', 'Archive revision 2');
    await waitForText(browser, NOTICE, 'Revision 2 is archived. The current revision is now revision 1.');
    // The message is true: the server archived it again.
    await waitForHistory(6);
    await waitForRevision(1);
  });

  it('shows no conflict when someone else saved exactly the same change', async () => {
    await clickButton(COLUMN, 'Details');
    const count = await historyCount();
    await clickButton(COLUMN, 'Edit part');
    await waitForText(browser, '#edit-heading', 'Edit part');
    await (await fieldInput('Tolerance')).setValue('0.25%');
    await (await browser.$('#edit-reason')).setValue('Tighter tolerance');
    // Meanwhile someone else makes the same fix.
    await editAsSomeoneElse(server.url, otherToken, IPN, { Tolerance: '0.25%' }, 'Tighter tolerance');
    await waitForHistory(count + 1);
    await clickButton('.edit-form', 'Save');
    await waitForText(browser, NOTICE, 'These changes are already saved, so nothing more was saved.');
    assert.equal((await browser.$$('.conflict')).length, 0, 'no conflict is shown');
    assert.equal((await browser.$$('.edit-form')).length, 0, 'the edit form closed');
    assert.equal(await fieldValue('Tolerance'), '0.25%');
    assert.equal(await historyCount(), count + 1, 'nothing more was saved');
  });

  it('says plainly when the team server has stopped', async () => {
    await clickButton(COLUMN, 'Details');
    await server.stop();
    await clickButton(COLUMN, 'Edit part');
    await waitForText(browser, '#edit-heading', 'Edit part');
    await (await fieldInput('Tolerance')).setValue('0.5%');
    await (await browser.$('#edit-reason')).setValue('Tighter tolerance');
    await clickButton('.edit-form', 'Save');
    await waitForText(
      browser,
      PROBLEM,
      'Could not reach the team server. Check that it is running, then save again.',
    );
    // What was typed is still there, ready to save again.
    assert.equal(await (await fieldInput('Tolerance')).getValue(), '0.5%');
    assert.equal(await (await browser.$('#edit-reason')).getValue(), 'Tighter tolerance');
  });
});
