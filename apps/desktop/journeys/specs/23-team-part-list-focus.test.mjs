// The part list stays where the person left it when a teammate saves
// (kicad-parts-0d99). Someone works from the keyboard in the part list:
// two sections open, the list scrolled, a part chosen and focused. Another
// member then changes that part. The same row keeps focus, and it shows the
// new words in place. The selection, the open sections and the scroll
// position stay put. The details show the change too.
//
// While searching, a part a teammate changes so it no longer matches
// leaves the list. Focus then moves to the row that took its place, never
// to the top of the page. A part that now matches appears, and focus
// stays where it is. When a section's last match leaves, focus moves to
// the section that took its place. When a save moves the focused row's
// section above another, the row keeps focus.
//
// Focus the person has put elsewhere, such as in the search box or in an
// Edit part field, stays there whatever the teammate's save removes.

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
  startTeamServer,
  teamSignIn,
  textOf,
  waitForLiveUpdates,
  waitForPreviewImages,
  waitForText,
} from '../harness.mjs';

const BADGE = '.team-settings > summary';
/** The part chosen and focused in the first test: RES 2K OHM. */
const CHOSEN = 'RES-0000-2001';
const CHANGED_DESCRIPTION = 'Thin film resistor, 2 kohm, 1%, changed by a teammate';

/**
 * Where the list is now: which element has focus (by the mark the journey
 * put on it, so a rebuilt row shows as a different element), the selected
 * part, the open sections and the list's scroll position.
 */
async function listState(browser) {
  return browser.execute(() => {
    const active = document.activeElement;
    const isRow = active?.matches('.results button.result') ?? false;
    const isToggle = active?.matches('.results .section-toggle') ?? false;
    return {
      focusedMark: active?.dataset.journeyMark ?? null,
      focusedIpn: isRow ? active.querySelector('.result-ipn').textContent : null,
      focusedSection: isToggle ? active.querySelector('.section-name').textContent : null,
      focusIsOnPage: active !== null && active !== document.body,
      selected: document.querySelector('.results button.result[aria-current="true"] .result-ipn')?.textContent ?? null,
      open: [...document.querySelectorAll('.results .section-toggle[aria-expanded="true"]')].map(
        (toggle) => toggle.querySelector('.section-name').textContent,
      ),
      scrollTop: Math.round(document.querySelector('.results').scrollTop),
    };
  });
}

/** Marks the element with focus, so a later check can tell it is the same one. */
async function markFocused(browser, mark) {
  await browser.execute((m) => {
    document.activeElement.dataset.journeyMark = m;
  }, mark);
}

/** Focuses the row of part `ipn` from the keyboard's point of view (no click). */
async function focusRow(browser, ipn) {
  const found = await browser.execute((wanted) => {
    const row = [...document.querySelectorAll('.results button.result')].find(
      (button) => button.querySelector('.result-ipn')?.textContent === wanted,
    );
    row?.focus();
    return document.activeElement === row;
  }, ipn);
  assert.ok(found, `the row of ${ipn} takes focus`);
}

/** The IPNs of the rows drawn in the section named `name`, in order. */
async function rowIpns(browser, name) {
  return browser.execute((wanted) => {
    const toggle = [...document.querySelectorAll('.results .section-toggle')].find(
      (t) => t.querySelector('.section-name').textContent === wanted,
    );
    const list = toggle && document.getElementById(toggle.getAttribute('aria-controls'));
    return list ? [...list.querySelectorAll('button.result .result-ipn')].map((ipn) => ipn.textContent) : null;
  }, name);
}

/** Waits until no section is loading its parts again. */
async function waitForListSettled(browser) {
  await browser.waitUntil(
    async () => browser.execute(() => document.querySelector('.results [aria-busy="true"]') === null),
    { timeout: 20_000, interval: 50, timeoutMsg: 'a section kept loading its parts' },
  );
  // Two frames, for the work the list does after drawing (focus and scroll).
  await browser.execute(
    () => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve(true)))),
  );
}

describe('team catalog: the part list stays put when a teammate saves', () => {
  let server;
  let browser;
  let editor;
  let teammate;
  let teammateToken;

  before(async () => {
    server = await startTeamServer();
    [editor, teammate] = server.accounts;
    teammateToken = await teamSignIn(server.url, teammate);
    ({ browser } = await launchApp({
      args: [`--team-server=${server.url}`, `--team-email=${editor.email}`],
    }));
  });

  after(async () => {
    await browser?.deleteSession();
    await server?.stop();
  });

  /** Publishes the teammate's change to `ipn`'s description. */
  async function teammateDescribes(ipn, description) {
    await editAsSomeoneElse(server.url, teammateToken, ipn, { Description: description }, 'Plainer description');
  }

  /** Publishes the teammate's change to `ipn`'s main MPN. */
  async function teammateRenames(ipn, mpn) {
    await editAsSomeoneElse(server.url, teammateToken, ipn, {}, 'Correct MPN', { mpn });
  }

  /** The section names, in list order. */
  async function sectionOrder() {
    return browser.execute(() =>
      [...document.querySelectorAll('.results .section-name')].map((name) => name.textContent),
    );
  }

  /** What has focus, and its text when it is a text box. */
  async function focusedBox() {
    return browser.execute(() => {
      const active = document.activeElement;
      return { mark: active?.dataset.journeyMark ?? null, id: active?.id ?? null, value: active?.value ?? null };
    });
  }

  it('keeps focus, selection, open sections and scroll, and shows the change', async () => {
    await (await browser.$(BADGE)).click();
    await (await browser.$('aria/Password')).setValue(editor.password);
    await (await browser.$('form[aria-label="Sign in to the team catalog"] button[type="submit"]')).click();
    await waitForText(browser, BADGE, `Team catalog · ${editor.name}`);
    await waitForLiveUpdates(browser);
    await waitForText(browser, '#result-count', /^\d+ matching parts$/);

    // Open both sections from the keyboard: the sample team's three
    // capacitors and six resistors.
    for (const [id, count] of [
      ['parts-in-cap', 3],
      ['parts-in-res', 6],
    ]) {
      await browser.execute((controls) => document.querySelector(`[aria-controls="${controls}"]`).focus(), id);
      await browser.keys('Enter');
      await browser.waitUntil(
        async () =>
          browser.execute(
            (controls, wanted) => document.getElementById(controls).querySelectorAll('button.result').length === wanted,
            id,
            count,
          ),
        { timeout: 10_000, timeoutMsg: `${id} did not open with its parts` },
      );
    }

    // Choose a part from the keyboard, further down the list.
    await focusRow(browser, CHOSEN);
    await browser.keys('Enter');
    await waitForText(browser, '#part-title', 'RC0603FR-072KL');
    await waitForListSettled(browser);
    // Scroll so the chosen row sits mid-list; focus stays on it.
    await browser.execute((wanted) => {
      const row = [...document.querySelectorAll('.results button.result')].find(
        (button) => button.querySelector('.result-ipn')?.textContent === wanted,
      );
      row.scrollIntoView({ block: 'center' });
    }, CHOSEN);
    await markFocused(browser, 'chosen-row');
    const before = await listState(browser);
    assert.deepEqual(
      { ...before, scrollTop: undefined },
      {
        focusedMark: 'chosen-row',
        focusedIpn: CHOSEN,
        focusedSection: null,
        focusIsOnPage: true,
        selected: CHOSEN,
        open: ['Capacitors', 'Resistors'],
        scrollTop: undefined,
      },
    );
    assert.ok(before.scrollTop > 0, `the list is scrolled down (${before.scrollTop} px)`);

    await teammateDescribes(CHOSEN, CHANGED_DESCRIPTION);

    // The row shows the new words in place, and so do the details.
    const rowShowsChange = () =>
      browser.execute(
        (ipn, expected) =>
          [...document.querySelectorAll('.results button.result')].some(
            (row) =>
              row.querySelector('.result-ipn')?.textContent === ipn &&
              row.querySelector('.result-description')?.textContent === expected,
          ),
        CHOSEN,
        CHANGED_DESCRIPTION,
      );
    await browser
      .waitUntil(rowShowsChange, { timeout: 20_000, interval: 50 })
      .catch(async () => assert.fail(`the row never showed the change: ${JSON.stringify(await listState(browser))}`));
    await waitForText(browser, '.part-description', CHANGED_DESCRIPTION);
    await waitForListSettled(browser);
    assert.deepEqual(await listState(browser), before, 'the list stays where the person left it');

    // The drawings finish before the screenshot.
    await waitForPreviewImages(browser, SYMBOL);
    await waitForPreviewImages(browser, FOOTPRINT);
    await assertAccessible(browser, '94-part-list-teammate-saved');
    await screenshot(browser, '94-part-list-teammate-saved');
  });

  it('moves focus to the next row when the focused part leaves the search', async () => {
    // A teammate marks two resistors and one capacitor with a new word.
    for (const [ipn, description] of [
      ['RES-0000-1000', 'RES 100 OHM 1% 1/10W 0603 zeta'],
      ['RES-0000-1500', 'RES 150 OHM 1% 1/10W 0603 zeta'],
      ['CAP-0000-0100', 'CAP CER 10pF 50V C0G 0603 zeta'],
    ]) {
      await teammateDescribes(ipn, description);
    }
    await searchFor(browser, 'zeta', '3 matching parts');
    await waitForListSettled(browser);

    // Focus the first resistor that matches; remember the row after it
    // (or before it, when it is the last).
    const resistors = await rowIpns(browser, 'Resistors');
    const leaving = resistors.find((ipn) => ipn === 'RES-0000-1000' || ipn === 'RES-0000-1500');
    const at = resistors.indexOf(leaving);
    const takesItsPlace = resistors[at + 1] ?? resistors[at - 1];
    await focusRow(browser, leaving);
    const before = await listState(browser);

    await teammateDescribes(leaving, 'RES no longer matching, changed by a teammate');
    await waitForText(browser, '#result-count', '2 matching parts');
    await browser.waitUntil(async () => !(await rowIpns(browser, 'Resistors')).includes(leaving), {
      timeout: 20_000,
      timeoutMsg: `${leaving} stayed in the list`,
    });
    await waitForListSettled(browser);
    const after = await listState(browser);
    assert.equal(after.focusedIpn, takesItsPlace, `focus moves to the row that took its place: ${JSON.stringify(after)}`);
    // Screen readers hear why focus moved.
    assert.equal(await textOf(browser, '#list-focus-note'), `${leaving} left the list.`);
    assert.deepEqual(
      { selected: after.selected, open: after.open, scrollTop: after.scrollTop },
      { selected: before.selected, open: before.open, scrollTop: before.scrollTop },
    );

    // A part that now matches appears; focus stays on the same row.
    await markFocused(browser, 'next-row');
    await teammateDescribes('RES-0000-1800', 'RES 180 OHM 1% 1/10W 0603 zeta');
    await waitForText(browser, '#result-count', '3 matching parts');
    await browser.waitUntil(async () => (await rowIpns(browser, 'Resistors')).includes('RES-0000-1800'), {
      timeout: 20_000,
      timeoutMsg: 'the part that now matches never appeared',
    });
    await waitForListSettled(browser);
    const added = await listState(browser);
    assert.deepEqual(
      { mark: added.focusedMark, ipn: added.focusedIpn },
      { mark: 'next-row', ipn: takesItsPlace },
      'focus stays on the same row when a part appears',
    );
  });

  it('moves focus to the next section when its last match leaves', async () => {
    await focusRow(browser, 'CAP-0000-0100');
    await teammateDescribes('CAP-0000-0100', 'CAP CER 10pF 50V C0G 0603');
    await waitForText(browser, '#result-count', '2 matching parts');
    await browser.waitUntil(
      async () => browser.execute(() => document.getElementById('parts-in-cap') === null),
      { timeout: 20_000, timeoutMsg: 'the Capacitors section stayed' },
    );
    await waitForListSettled(browser);
    const after = await listState(browser);
    assert.equal(after.focusedSection, 'Resistors', `focus moves to the section that took its place: ${JSON.stringify(after)}`);
    assert.equal(await textOf(browser, '#list-focus-note'), 'CAP-0000-0100 left the list.');
    await assertAccessible(browser, 'part-list-section-left');
  });

  it('keeps focus on a row when a save moves its section above another', async () => {
    // Both sections match equally well, so they are in name order.
    await teammateDescribes('CAP-0000-0100', 'CAP CER 10pF 50V C0G 0603 zeta');
    await waitForText(browser, '#result-count', '3 matching parts');
    await browser.waitUntil(async () => (await sectionOrder()).join() === 'Capacitors,Resistors', {
      timeout: 20_000,
      timeoutMsg: 'Capacitors never came back above Resistors',
    });
    await waitForListSettled(browser);
    await focusRow(browser, 'RES-0000-1500');
    await markFocused(browser, 'moved-row');
    const before = await listState(browser);

    // An MPN that starts with the search is a better match, so Resistors
    // moves above Capacitors. The browser moves the focused row with it.
    await teammateRenames('RES-0000-1800', 'ZETA-1800');
    await browser.waitUntil(async () => (await sectionOrder()).join() === 'Resistors,Capacitors', {
      timeout: 20_000,
      timeoutMsg: 'Resistors never moved above Capacitors',
    });
    await waitForListSettled(browser);
    assert.deepEqual(
      await listState(browser),
      { ...before, open: ['Resistors', 'Capacitors'] },
      'the moved row keeps focus',
    );
  });

  it('leaves focus in the search box when the row it came from leaves', async () => {
    await focusRow(browser, 'RES-0000-1500');
    await (await browser.$('#part-search')).click();
    await markFocused(browser, 'search-box');
    const before = await focusedBox();
    assert.deepEqual(before, { mark: 'search-box', id: 'part-search', value: 'zeta' });

    await teammateDescribes('RES-0000-1500', 'RES 150 OHM 1% 1/10W 0603');
    await waitForText(browser, '#result-count', '2 matching parts');
    await browser.waitUntil(async () => !(await rowIpns(browser, 'Resistors')).includes('RES-0000-1500'), {
      timeout: 20_000,
      timeoutMsg: 'RES-0000-1500 stayed in the list',
    });
    await waitForListSettled(browser);
    assert.deepEqual(await focusedBox(), before, 'the search box keeps focus and its text');
  });

  it('leaves focus in an Edit part field when the row it came from leaves', async () => {
    // Choose the capacitor from the keyboard and start editing it.
    await focusRow(browser, 'CAP-0000-0100');
    await browser.keys('Enter');
    await waitForText(browser, '#part-title', 'CL10C100JB8NNNC');
    // Edit part shows once the team catalog's details have loaded.
    await browser.waitUntil(
      async () =>
        browser.execute(() => {
          const button = [...document.querySelectorAll('.part-fields button')].find(
            (b) => b.textContent.trim() === 'Edit part',
          );
          button?.click();
          return button !== undefined;
        }),
      { timeout: 20_000, interval: 100, timeoutMsg: 'the capacitor never offered Edit part' },
    );
    await waitForText(browser, '#edit-heading', 'Edit part');

    // Focus a resistor row, then move to the Description field and type.
    await focusRow(browser, 'RES-0000-1800');
    const fieldId = await browser.execute(
      () =>
        [...document.querySelectorAll('.edit-form .edit-row label')].find(
          (label) => label.textContent.trim() === 'Description',
        )?.htmlFor ?? null,
    );
    assert.ok(fieldId, 'the edit form has a Description field');
    const field = await browser.$(`#${fieldId}`);
    await field.click();
    await browser.keys('End');
    await browser.keys(' draft');
    await markFocused(browser, 'edit-field');
    const before = await focusedBox();
    assert.deepEqual(before, { mark: 'edit-field', id: fieldId, value: 'CAP CER 10pF 50V C0G 0603 zeta draft' });

    // The teammate's change takes the last resistor, and its section, away.
    await editAsSomeoneElse(
      server.url,
      teammateToken,
      'RES-0000-1800',
      { Description: 'RES 180 OHM 1% 1/10W 0603' },
      'Back to the CSV',
      { mpn: 'RC0603FR-0718KL' },
    );
    await waitForText(browser, '#result-count', '1 matching part');
    await browser.waitUntil(async () => browser.execute(() => document.getElementById('parts-in-res') === null), {
      timeout: 20_000,
      timeoutMsg: 'the Resistors section stayed',
    });
    await waitForListSettled(browser);
    assert.deepEqual(await focusedBox(), before, 'the Edit part field keeps focus and its text');
  });
});
