// Categories (kicad-parts-ae44): the local team server imports each CSV
// file as one category (here the resistors, capacitors and regulators), so
// every part in it shows its category by name. Edit
// part offers every category; choosing another keeps the IPN and every
// field and adds the new category's fields, and the history says what
// changed in plain names.

import assert from 'node:assert/strict';
import { after, before, describe, it } from 'node:test';
import {
  FOOTPRINT,
  SYMBOL,
  assertAccessible,
  launchApp,
  record,
  screenshot,
  searchFor,
  selectResult,
  startTeamServer,
  textOf,
  waitForLiveUpdates,
  waitForPreviewImages,
  waitForText,
} from '../harness.mjs';

const IPN = 'RES-0000-0470';
const MPN = 'RC0603FR-0747RL';
const BADGE = '.team-settings > summary';
const COLUMN = '.part-fields';
const NOTICE = '.team-notice';
const CATEGORY = '#part-category';

describe('team catalog: categories', () => {
  let server;
  let browser;
  let editor;

  before(async () => {
    server = await startTeamServer({ csv: ['g-res', 'g-cap', 'g-reg'] });
    [editor] = server.accounts;
    ({ browser } = await launchApp({
      args: [`--team-server=${server.url}`, `--team-email=${editor.email}`],
    }));
  });

  after(async () => {
    await browser?.deleteSession();
    await server?.stop();
  });

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

  async function editLabels() {
    return browser.execute(() =>
      [...document.querySelectorAll('.edit-form fieldset:first-of-type .edit-row label')].map((l) =>
        l.textContent.trim(),
      ),
    );
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

  async function sourceFact() {
    return browser.execute(() => {
      const fact = [...document.querySelectorAll('.fact')].find((f) => f.querySelector('dt')?.textContent === 'Source');
      return fact?.querySelector('dd')?.textContent ?? null;
    });
  }

  async function waitForTeamPart() {
    await browser.waitUntil(async () => /^Team catalog, revision \d+ of this part$/.test((await sourceFact()) ?? ''), {
      timeout: 20_000,
      interval: 100,
      timeoutMsg: `the part never showed the team catalog; last: ${JSON.stringify(await sourceFact())}`,
    });
  }

  it('shows the category of a part from each CSV file', async () => {
    await (await browser.$(BADGE)).click();
    await (await browser.$('aria/Password')).setValue(editor.password);
    await (await browser.$('form[aria-label="Sign in to the team catalog"] button[type="submit"]')).click();
    await waitForText(browser, BADGE, `Team catalog · ${editor.name}`);
    await waitForLiveUpdates(browser);
    // A regulator, from g-reg.csv.
    await searchFor(browser, 'KF33BD', '1 matching part');
    await selectResult(browser, 'KF33BD-TR');
    await waitForTeamPart();
    await waitForText(browser, CATEGORY, 'Category: Voltage regulators');
    await searchFor(browser, MPN, '1 matching part');
    await selectResult(browser, MPN);
    await waitForTeamPart();
    await waitForText(browser, CATEGORY, 'Category: Resistors');
    // The drawings finish before the screenshot below.
    await waitForPreviewImages(browser, SYMBOL);
    await waitForPreviewImages(browser, FOOTPRINT);
  });

  it('chooses another category in Edit part, keeping the IPN and every field', async () => {
    await clickButton(COLUMN, 'Edit part');
    await waitForText(browser, '#edit-heading', 'Edit part');
    const select = await browser.$('#edit-category');
    assert.equal(await select.getValue(), 'res');
    const options = await browser.execute(() =>
      [...document.querySelectorAll('#edit-category option')].map((o) => o.textContent.trim()),
    );
    // One choice per imported CSV file, by name.
    assert.deepEqual(options, ['Capacitors', 'Resistors', 'Voltage regulators']);
    assert.deepEqual(await editLabels(), ['Category', 'Description', 'Value', 'Tolerance', 'Datasheet']);
    assert.equal(await browser.$('#edit-category-hint').isExisting(), false, 'no hint before a change');

    await select.selectByAttribute('value', 'cap');
    await waitForText(browser, '#edit-category-hint', 'The IPN stays the same. Fields of the new category are added below.');
    // Every field stays, and the capacitors' Voltage and Material are added.
    assert.deepEqual(await editLabels(), [
      'Category',
      'Description',
      'Value',
      'Tolerance',
      'Datasheet',
      'Voltage',
      'Material',
    ]);
    // Choosing back removes only the fields it added.
    await select.selectByAttribute('value', 'res');
    assert.deepEqual(await editLabels(), ['Category', 'Description', 'Value', 'Tolerance', 'Datasheet']);
    await select.selectByAttribute('value', 'cap');
    await (await fieldInput('Voltage')).setValue('50V');
    await (await browser.$('#edit-reason')).setValue('It is a capacitor');
    await browser.execute(() => {
      document.querySelector('.part-fields').scrollTop = 0;
    });
    await assertAccessible(browser, '60-team-category-choice');
    await screenshot(browser, '60-team-category-choice');

    await clickButton('.edit-form', 'Save');
    await waitForText(browser, NOTICE, 'Saved. The part details changed; no revision was added.');
    await waitForText(browser, CATEGORY, 'Category: Capacitors');
    assert.equal(await textOf(browser, '#fields-heading'), 'Part details (6)');
    assert.match(await textOf(browser, '.part-header'), new RegExp(IPN), 'the IPN stays the same');

    await clickButton(COLUMN, 'History (2)');
    await (await browser.$('.history li:nth-child(1) button')).click();
    await waitForText(browser, '#revision-heading', 'What changed in the part details');
    const rows = await browser.execute(() =>
      [...document.querySelectorAll('.revision .changes tbody tr')].map((tr) =>
        [...tr.children].map((cell) => cell.textContent.trim()),
      ),
    );
    record({ event: 'category-history', rows });
    assert.deepEqual(rows, [
      ['Category', 'Resistors', 'Capacitors'],
      ['Voltage', '(blank)', '50V'],
    ]);
  });
});
