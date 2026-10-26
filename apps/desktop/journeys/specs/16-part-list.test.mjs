// The part list groups a team's parts by category in sections that open
// and close: by hand (mouse or keyboard), by themselves while searching, and
// when a part is selected. Also: the drawing close-up closes on a click
// outside it, and focus goes back to the drawing that opened it. The team
// has the sample library's parts.

import assert from 'node:assert/strict';
import { after, before, describe, it } from 'node:test';
import {
  SYMBOL,
  assertAccessible,
  expectFocusKeptOnEnter,
  launchApp,
  screenshot,
  searchFor,
  selectResult,
  signInToTeam,
  startTeamServer,
  waitForPreview,
  waitForSelectedInList,
  waitForText,
} from '../harness.mjs';

/** Every section as [name, count, expanded], in list order. */
async function sections(browser) {
  return browser.execute(() =>
    [...document.querySelectorAll('.results .section-toggle')].map((toggle) => [
      toggle.querySelector('.section-name').textContent,
      toggle.querySelector('.section-count').textContent,
      toggle.getAttribute('aria-expanded'),
    ]),
  );
}

/** The parts drawn in the section named `name`. */
async function rowsIn(browser, name) {
  return browser.execute(
    (wanted) => {
      const toggle = [...document.querySelectorAll('.results .section-toggle')].find(
        (t) => t.querySelector('.section-name').textContent === wanted,
      );
      const list = toggle && document.getElementById(toggle.getAttribute('aria-controls'));
      return list ? list.querySelectorAll('button.result').length : null;
    },
    name,
  );
}

async function toggleOf(browser, name) {
  for (const toggle of await browser.$$('.results .section-toggle')) {
    if ((await toggle.$('.section-name').getText()) === name) return toggle;
  }
  assert.fail(`no section named ${name}`);
}

const opened = (list) => list.filter(([, , expanded]) => expanded === 'true').map(([name]) => name);

describe('part list grouped by category', () => {
  let server;
  let browser;

  before(async () => {
    server = await startTeamServer();
    ({ browser } = await launchApp());
    await signInToTeam(browser, server);
  });

  after(async () => {
    await browser?.deleteSession();
    await server?.stop();
  });

  it('opens with every category closed, each with its number of parts', async () => {
    await waitForText(browser, '#result-count', '16 matching parts');
    const list = await sections(browser);
    // Each of the seven sample categories has parts, so each is a section.
    assert.equal(list.length, 7);
    assert.deepEqual(opened(list), []);
    const names = list.map(([name]) => name);
    assert.deepEqual(names, [...names].sort((a, b) => a.toLowerCase().localeCompare(b.toLowerCase())));
    assert.equal(list.reduce((sum, [, count]) => sum + Number(count.replace(/\D/g, '')), 0), 16);
    assert.ok(names.includes('Resistors') && names.includes('Voltage regulators'), names.join(', '));
    assert.equal(await browser.execute(() => document.querySelectorAll('.results button.result').length), 0);
    // Each header is a real button that names its section and count.
    const resistors = await toggleOf(browser, 'Resistors');
    assert.equal(await resistors.getTagName(), 'button');
    assert.match(await resistors.getAttribute('aria-controls'), /^parts-in-res$/);
    assert.match(await browser.execute((el) => el.textContent, resistors), /^Resistors \d+ parts$/);
  });

  it('opens and closes sections from the keyboard', async () => {
    const resistors = await toggleOf(browser, 'Resistors');
    await browser.execute((el) => el.focus(), resistors);
    await browser.keys('Enter');
    await browser.waitUntil(async () => (await resistors.getAttribute('aria-expanded')) === 'true', {
      timeout: 5_000,
      timeoutMsg: 'Enter did not open Resistors',
    });
    // Its parts load when it opens.
    const count = Number((await resistors.$('.section-count').getText()).replace(/\D/g, ''));
    await browser.waitUntil(async () => (await rowsIn(browser, 'Resistors')) === count, {
      timeout: 5_000,
      timeoutMsg: 'the opened section did not show its parts',
    });
    // Choosing a part with the keyboard keeps focus on it, even after the
    // list hears where the selected part is.
    await expectFocusKeptOnEnter(browser, '#parts-in-res li:nth-child(3) button.result');

    const capacitors = await toggleOf(browser, 'Capacitors');
    await browser.execute((el) => el.focus(), capacitors);
    await browser.keys(' ');
    await browser.waitUntil(async () => (await capacitors.getAttribute('aria-expanded')) === 'true', {
      timeout: 5_000,
      timeoutMsg: 'Space did not open Capacitors',
    });
    await browser.keys(' ');
    await browser.waitUntil(async () => (await capacitors.getAttribute('aria-expanded')) === 'false', {
      timeout: 5_000,
      timeoutMsg: 'Space did not close Capacitors',
    });
    assert.equal(await rowsIn(browser, 'Capacitors'), 0);
    assert.deepEqual(opened(await sections(browser)), ['Resistors']);
  });

  it('opens the sections with matches while searching, and goes back when cleared', async () => {
    await searchFor(browser, 'sample-mcu', '1 matching part');
    assert.deepEqual(await sections(browser), [['Microcontrollers', '1 part', 'true']]);

    await searchFor(browser, '0603', /^\d+ matching parts$/);
    const found = await sections(browser);
    assert.ok(found.length > 1, JSON.stringify(found));
    assert.deepEqual(opened(found), found.map(([name]) => name), 'every section with matches is open');
    for (const [name, count] of found) {
      // Up to 25 matches each, then "Show all".
      const n = Number(count.replace(/\D/g, ''));
      assert.equal(await rowsIn(browser, name), Math.min(n, 25), `${name} shows its first matches`);
    }
    await assertAccessible(browser, '65-part-list-search');
    await screenshot(browser, '65-part-list-search');

    // Closed by hand, a section stays closed during this search.
    const first = found[0][0];
    await (await toggleOf(browser, first)).click();
    assert.deepEqual(opened(await sections(browser)), found.slice(1).map(([name]) => name));

    // Clearing the search goes back to what was open before it.
    await searchFor(browser, '', '16 matching parts');
    assert.deepEqual(opened(await sections(browser)), ['Resistors']);
  });

  it('opens the section of a part selected while searching', async () => {
    await searchFor(browser, 'KF33BD', '1 matching part');
    await selectResult(browser, 'KF33BD-TR');
    await waitForText(browser, '#part-title', 'KF33BD-TR');
    // Closed by hand during the search, the section still opens once the
    // search is cleared, with the part scrolled into view.
    await (await toggleOf(browser, 'Voltage regulators')).click();
    await searchFor(browser, '', '16 matching parts');
    await waitForSelectedInList(browser, 'KF33BD-TR', 'Voltage regulators');
    assert.deepEqual(opened(await sections(browser)).sort(), ['Resistors', 'Voltage regulators']);
    await waitForPreview(browser, SYMBOL);
    await assertAccessible(browser, '64-part-list-sections');
    await screenshot(browser, '64-part-list-sections');
  });

  it('closes the drawing close-up on a click outside it', async () => {
    const drawing = await browser.$(`${SYMBOL} figure button`);
    const dialog = `${SYMBOL} dialog`;
    const isOpen = () => browser.execute((s) => document.querySelector(s)?.open === true, dialog);
    const focusIsOnDrawing = () => browser.execute((el) => document.activeElement === el, drawing);
    for (const how of ['backdrop', 'Escape', 'Close']) {
      await drawing.click();
      await browser.waitUntil(isOpen, { timeout: 5_000, timeoutMsg: 'the close-up did not open' });
      if (how === 'backdrop') {
        // A click inside the close-up keeps it open, and so does a press
        // inside that is released outside (a drag out of the picture).
        await browser.$(`${dialog} h4`).click();
        assert.equal(await isOpen(), true, 'a click inside keeps the close-up open');
        await browser
          .action('pointer')
          .move({ origin: await browser.$(`${dialog} h4`) })
          .down()
          .move({ x: 8, y: 8, origin: 'viewport' })
          .up()
          .perform();
        assert.equal(await isOpen(), true, 'a drag out of the close-up keeps it open');
        await browser.action('pointer').move({ x: 8, y: 8, origin: 'viewport' }).down().up().perform();
      } else if (how === 'Escape') {
        await browser.keys('Escape');
      } else {
        await browser.$(`${dialog} .zoom-header button`).click();
      }
      await browser.waitUntil(async () => !(await isOpen()), {
        timeout: 5_000,
        timeoutMsg: `${how} did not close the close-up`,
      });
      assert.equal(await focusIsOnDrawing(), true, `after ${how}, focus is back on the drawing`);
    }
  });
});
