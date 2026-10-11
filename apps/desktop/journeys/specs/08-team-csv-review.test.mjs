// CSV changes that an import did not apply: someone edits a part in the
// app, the next import of the CSV catalog leaves the edited part alone, and
// the app shows a notice by itself. Reviewing lists each field whose CSV
// value differs; taking only the value saves the part details (no
// revision) and leaves the rest. The taken change never shows again; when
// the CSV changes, the notice comes back, and keeping the current values is
// a choice the app records (kicad-parts-75a5).

import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { after, before, describe, it } from 'node:test';
import {
  assertAccessible,
  editAsSomeoneElse,
  launchApp,
  record,
  repoRoot,
  screenshot,
  searchFor,
  selectResult,
  startTeamServer,
  teamSignIn,
  tempDir,
  textOf,
  waitForLiveUpdates,
  waitForText,
} from '../harness.mjs';

const IPN = 'RES-0000-0470';
const MPN = 'RC0603FR-0747RL';
const BADGE = '.team-settings > summary';
const COLUMN = '.part-fields';
const NOTICE = '.team-notice';
const CSV_NOTICE = '.csv-notice';
const FORM = '.csv-review-form';

/** A copy of the CSV catalog whose row for IPN has a tighter tolerance. */
function changedCatalog() {
  const root = tempDir('changed-csv');
  for (const folder of ['database', 'symbols', 'footprints']) {
    fs.cpSync(path.join(repoRoot, folder), path.join(root, folder), { recursive: true });
  }
  const file = path.join(root, 'database', 'g-res.csv');
  const text = fs.readFileSync(file, 'utf8');
  const from = '"47","g-pas:R_US","g-res:R_0603_1608Metric","1%"';
  assert.equal(text.split(from).length, 2, 'the row of the part is found once');
  fs.writeFileSync(file, text.replace(from, '"47","g-pas:R_US","g-res:R_0603_1608Metric","0.5%"'));
  return root;
}

describe('team catalog: reviewing CSV changes that were not applied', () => {
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

  async function fieldValue(name) {
    return browser.execute((label) => {
      const row = [...document.querySelectorAll('.part-fields tbody tr')].find(
        (tr) => tr.querySelector('th')?.textContent.trim() === label,
      );
      return row?.querySelector('td')?.textContent.trim() ?? null;
    }, name);
  }

  async function waitForField(name, value) {
    await browser.waitUntil(async () => (await fieldValue(name)) === value, {
      timeout: 20_000,
      interval: 100,
      timeoutMsg: `${name} never became ${value}; last: ${JSON.stringify(await fieldValue(name))}`,
    });
  }

  async function sourceFact() {
    return browser.execute(() => {
      const fact = [...document.querySelectorAll('.fact')].find((f) => f.querySelector('dt')?.textContent === 'Source');
      return fact?.querySelector('dd')?.textContent ?? null;
    });
  }

  /** The review form's rows: field, now, in the CSV, and whether ticked. */
  async function reviewRows() {
    return browser.execute(() =>
      [...document.querySelectorAll('.csv-review-form tbody tr')].map((tr) => ({
        field: tr.querySelector('th').textContent.trim(),
        now: tr.querySelectorAll('td')[1].textContent.trim(),
        csv: tr.querySelectorAll('td')[2].textContent.trim(),
        ticked: tr.querySelector('input[type="checkbox"]').checked,
      })),
    );
  }

  async function tick(field) {
    const id = await browser.execute(
      (name) =>
        [...document.querySelectorAll('.csv-review-form tbody tr')]
          .find((tr) => tr.querySelector('th').textContent.trim() === name)
          ?.querySelector('input[type="checkbox"]')?.id ?? null,
      field,
    );
    assert.ok(id, `the review lists ${field}`);
    await (await browser.$(`#${id}`)).click();
  }

  async function noCsvNotice() {
    return (await browser.$$(CSV_NOTICE)).length === 0;
  }

  it('shows by itself that the CSV has changes the import did not apply', async () => {
    await waitForText(browser, '#result-count', '178 matching parts');
    await browser.execute(() => (document.querySelector('.team-settings').open = true));
    await (await browser.$('aria/Password')).setValue(editor.password);
    await (await browser.$('form[aria-label="Sign in to the team catalog"] button[type="submit"]')).click();
    await waitForText(browser, BADGE, `Team catalog · ${editor.name}`);
    await waitForLiveUpdates(browser);
    await searchFor(browser, MPN, '1 matching part');
    await selectResult(browser, MPN);
    await browser.waitUntil(async () => (await sourceFact()) === 'Team catalog, revision 1 of this part', {
      timeout: 20_000,
      timeoutMsg: 'the part never showed the team catalog',
    });
    assert.ok(await noCsvNotice(), 'no notice before anything differs');

    // Someone else edits the description and the value in the app.
    await editAsSomeoneElse(
      server.url,
      otherToken,
      IPN,
      { Description: 'Thick film resistor, 47 ohm', Value: '47 ohm' },
      'Plainer words',
    );
    await waitForField('Value', '47 ohm');
    // The next import of the CSV catalog leaves the edited details alone.
    assert.equal(await server.reimport(), 'nothing_changed');
    await waitForText(
      browser,
      CSV_NOTICE,
      /The CSV catalog has changes for this part that were not applied, because the part was edited in this app\./,
    );
    assert.equal(await textOf(browser, '#csv-notice-heading'), 'CSV changes not applied');
    assert.equal(await fieldValue('Value'), '47 ohm', 'nothing was applied');
    assert.equal(await browser.execute(() => document.querySelectorAll('[role="alert"]').length), 0);
    await assertAccessible(browser, '54-team-csv-changes');
    await screenshot(browser, '54-team-csv-changes');
  });

  it('lists each differing field and takes only the ticked ones', async () => {
    await clickButton(CSV_NOTICE, 'Review CSV changes');
    await waitForText(browser, '#csv-review-heading', 'Review CSV changes');
    assert.equal(
      await browser.execute(() => document.activeElement?.id),
      'csv-review-heading',
      'focus moves to the form',
    );
    assert.deepEqual(await reviewRows(), [
      { field: 'Description', now: 'Thick film resistor, 47 ohm', csv: 'RES 47 OHM 1% 1/10W 0603', ticked: false },
      { field: 'Value', now: '47 ohm', csv: '47', ticked: false },
    ]);
    assert.match(await textOf(browser, FORM), /g-res\.csv has these values for this part\. Tick the ones to take\./);
    assert.equal(await (await browser.$('#csv-review-reason')).getValue(), 'Accept CSV changes from g-res.csv');
    const take = await browser.$(`${FORM} button[type="submit"]`);
    assert.equal(await take.getText(), 'Take the ticked values');
    assert.equal(await take.isEnabled(), false, 'nothing is ticked yet');
    await tick('Value');
    assert.equal(await take.isEnabled(), true);
    await assertAccessible(browser, '55-team-csv-review');
    await screenshot(browser, '55-team-csv-review');

    await take.click();
    await waitForText(browser, NOTICE, 'Saved. The part details changed; no revision was added.');
    await waitForField('Value', '47');
    assert.equal(await fieldValue('Description'), 'Thick film resistor, 47 ohm', 'not ticked, so it stays');
    assert.equal(await sourceFact(), 'Team catalog, revision 1 of this part', 'no revision was added');
    assert.ok(await noCsvNotice(), 'the notice is gone');
    assert.equal(
      await browser.execute(() => document.activeElement?.id),
      'fields-heading',
      'focus returns to the part details',
    );
    assert.match(
      await browser.execute(() => document.querySelector('#team-tab-history').textContent.trim()),
      /^History \(3\)$/,
    );
  });

  it('never shows a taken change again, and records keeping the current values', async () => {
    // The same CSV again: the description still differs, but it was
    // reviewed, so nothing shows.
    const outcome = await server.reimport();
    record({ event: 'reimport-after-review', outcome });
    await browser.pause(1_500);
    assert.ok(await noCsvNotice(), 'the reviewed change does not come back');

    // When the CSV changes, the notice comes back; keeping the current
    // values closes it and says so.
    assert.equal(await server.reimport(changedCatalog()), 'nothing_changed');
    await waitForText(browser, CSV_NOTICE, /were not applied/);
    await clickButton(CSV_NOTICE, 'Review CSV changes');
    await waitForText(browser, '#csv-review-heading', 'Review CSV changes');
    assert.deepEqual(
      (await reviewRows()).map((row) => row.field),
      ['Description', 'Tolerance'],
    );
    await clickButton(FORM, 'Keep current values');
    await waitForText(
      browser,
      NOTICE,
      'Kept the current values. These CSV changes show again only if the CSV changes.',
    );
    assert.ok(await noCsvNotice());
    assert.equal(await fieldValue('Tolerance'), '1%');
    assert.equal(await fieldValue('Description'), 'Thick film resistor, 47 ohm');
    await clickButton(COLUMN, 'History (4)');
    assert.match(await textOf(browser, '.history li:first-child .history-title'), /^CSV changes · Kept own values by /);
  });
});
