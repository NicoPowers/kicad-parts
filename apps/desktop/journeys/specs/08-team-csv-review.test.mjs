// New CSV values that an import did not apply: someone edits a part in the
// app, and an import of the unchanged CSV shows nothing (the CSV has no new
// values; only the part was edited). When the CSV then changes, the next
// import leaves the edited part alone and the app shows a notice by itself.
// Reviewing lists each field whose CSV value differs; accepting only the
// value saves the part details (no revision) and leaves the rest. The
// accepted values never show again; when the CSV changes again, the notice
// comes back, and keeping the current values is a choice the app records.
// A save that is half refused keeps only the values still open, so trying
// again sends just those (kicad-parts-75a5).

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

/**
 * A copy of the CSV catalog whose row for IPN has this value and tolerance
 * instead of 47 and 1%, with any other [from, to] replacements.
 */
function changedCatalog(value, tolerance, others = []) {
  const root = tempDir('changed-csv');
  for (const folder of ['database', 'symbols', 'footprints']) {
    fs.cpSync(path.join(repoRoot, folder), path.join(root, folder), { recursive: true });
  }
  const file = path.join(root, 'database', 'g-res.csv');
  const text = fs.readFileSync(file, 'utf8');
  const row = (v, t) => `"${v}","g-res:R_US","g-Resistor_SMD:R_0603_1608Metric","${t}"`;
  assert.equal(text.split(row('47', '1%')).length, 2, 'the row of the part is found once');
  let changed = text.replace(row('47', '1%'), row(value, tolerance));
  for (const [from, to] of others) {
    assert.equal(changed.split(from).length, 2, `${from} is found once`);
    changed = changed.replace(from, to);
  }
  fs.writeFileSync(file, changed);
  return root;
}

describe('team catalog: reviewing new CSV values that were not applied', () => {
  let server;
  let browser;
  let editor;
  let other;
  let otherToken;

  before(async () => {
    server = await startTeamServer({ csv: ['g-res'] });
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
        csv: tr.querySelector('.csv-value').textContent.trim(),
        changedInCsv: tr.querySelector('.csv-same') === null,
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

  it('shows by itself when the CSV has new values the import did not apply', async () => {
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
    // Importing the unchanged CSV leaves the edited details alone and
    // offers nothing: the CSV has no new values.
    assert.equal(await server.reimport(), 'nothing_changed');
    await browser.pause(1_500);
    assert.ok(await noCsvNotice(), 'no notice while the CSV is unchanged');
    // The CSV now says 47R: the import still leaves the edited details
    // alone, and the app shows the new values by itself.
    assert.equal(await server.reimport(changedCatalog('47R', '1%')), 'nothing_changed');
    await waitForText(
      browser,
      CSV_NOTICE,
      /g-res\.csv has new values for this part\. They were not applied, because the part was edited in this app\./,
    );
    assert.equal(await textOf(browser, '#csv-notice-heading'), 'New values in the CSV');
    assert.equal(
      await browser.execute(() => document.querySelector('.csv-notice').getAttribute('role')),
      'status',
      'screen readers hear the notice when it appears',
    );
    assert.equal(await fieldValue('Value'), '47 ohm', 'nothing was applied');
    assert.equal(await browser.execute(() => document.querySelectorAll('[role="alert"]').length), 0);
    await assertAccessible(browser, '54-team-csv-changes');
    await screenshot(browser, '54-team-csv-changes');
  });

  it('lists each differing field and accepts only the ticked ones', async () => {
    await clickButton(CSV_NOTICE, 'Review CSV values');
    await waitForText(browser, '#csv-review-heading', 'Review CSV values');
    assert.equal(
      await browser.execute(() => document.activeElement?.id),
      'csv-review-heading',
      'focus moves to the form',
    );
    // The value the CSV changed comes first; the description is listed
    // too, marked, since accepting it would undo the edit.
    assert.deepEqual(await reviewRows(), [
      { field: 'Value', now: '47 ohm', csv: '47R', changedInCsv: true, ticked: false },
      {
        field: 'Description',
        now: 'Thick film resistor, 47 ohm',
        csv: 'RES 47 OHM 1% 1/10W 0603',
        changedInCsv: false,
        ticked: false,
      },
    ]);
    assert.match(await textOf(browser, `${FORM} tbody tr:last-child`), /\(not changed in the CSV\)/);
    assert.match(await textOf(browser, FORM), /g-res\.csv has new values for this part\. Tick the ones to accept\./);
    assert.equal(await textOf(browser, `label[for="csv-review-reason"]`), 'Reason (shown in the history)');
    assert.equal(await (await browser.$('#csv-review-reason')).getValue(), 'Accept CSV values from g-res.csv');
    const take = await browser.$(`${FORM} button[type="submit"]`);
    assert.equal(await take.getText(), 'Accept the ticked values');
    assert.equal(await take.isEnabled(), false, 'nothing is ticked yet');
    await tick('Value');
    assert.equal(await take.isEnabled(), true);
    await assertAccessible(browser, '55-team-csv-review');
    await screenshot(browser, '55-team-csv-review');

    await take.click();
    await waitForText(browser, NOTICE, 'Saved. The part details changed; no revision was added.');
    await waitForField('Value', '47R');
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

  it('never shows reviewed values again, and records keeping the current values', async () => {
    // The same CSV again: the description still differs, but those values
    // were reviewed, so nothing shows.
    const outcome = await server.reimport(changedCatalog('47R', '1%'));
    record({ event: 'reimport-after-review', outcome });
    await browser.pause(1_500);
    assert.ok(await noCsvNotice(), 'the reviewed values do not come back');

    // When the CSV changes again, the notice comes back; keeping the
    // current values closes it and says so.
    assert.equal(await server.reimport(changedCatalog('47R', '0.5%')), 'nothing_changed');
    await waitForText(browser, CSV_NOTICE, /has new values for this part/);
    await clickButton(CSV_NOTICE, 'Review CSV values');
    await waitForText(browser, '#csv-review-heading', 'Review CSV values');
    assert.deepEqual(
      (await reviewRows()).map((row) => [row.field, row.changedInCsv]),
      [
        ['Tolerance', true],
        ['Description', false],
      ],
    );
    await clickButton(FORM, 'Keep current values');
    await waitForText(
      browser,
      NOTICE,
      'Kept the current values. You are asked about these CSV values again only if the CSV changes.',
    );
    assert.ok(await noCsvNotice());
    assert.equal(await fieldValue('Tolerance'), '1%');
    assert.equal(await fieldValue('Description'), 'Thick film resistor, 47 ohm');
    await clickButton(COLUMN, 'History (4)');
    assert.match(await textOf(browser, '.history li:first-child .history-title'), /^CSV values · Kept current part details by /);
  });

  it('keeps only the values still open after a half-refused save', async () => {
    const ipn = 'RES-0000-1000';
    const mpn = 'RC0603FR-07100RL';
    await searchFor(browser, mpn, '1 matching part');
    await selectResult(browser, mpn);
    await browser.waitUntil(async () => (await sourceFact()) === 'Team catalog, revision 1 of this part', {
      timeout: 20_000,
      timeoutMsg: 'the second part never showed the team catalog',
    });
    // Someone else edits its description, then its symbol twice
    // (revisions 2 and 3).
    await editAsSomeoneElse(server.url, otherToken, ipn, { Description: 'Thick film resistor, 100 ohm' }, 'Plainer words');
    await editAsSomeoneElse(server.url, otherToken, ipn, { Symbol: 'Device:R' }, "KiCad's symbol");
    await editAsSomeoneElse(server.url, otherToken, ipn, { Symbol: 'Device:R_Small' }, 'Smaller symbol');
    await waitForField('Description', 'Thick film resistor, 100 ohm');
    // The CSV now says 100R and revision 2's symbol; then revision 2 is
    // archived, so accepting that symbol will be refused.
    const both = changedCatalog('47R', '0.5%', [
      ['"100","g-res:R_US","g-Resistor_SMD:R_0603_1608Metric","1%"', '"100R","Device:R","g-Resistor_SMD:R_0603_1608Metric","1%"'],
    ]);
    assert.equal(await server.reimport(both), 'nothing_changed');
    const archived = await fetch(`${server.url}/api/catalog/archives`, {
      method: 'POST',
      headers: { Authorization: otherToken, 'Content-Type': 'application/json' },
      body: JSON.stringify({
        correlationId: `journey-archive-${Date.now()}`,
        reason: 'Wrong symbol',
        ipn,
        revision: 2,
        archived: true,
      }),
    });
    assert.equal(archived.status, 200, 'revision 2 is archived');
    await waitForText(browser, CSV_NOTICE, /has new values for this part/);
    await clickButton(CSV_NOTICE, 'Review CSV values');
    await waitForText(browser, '#csv-review-heading', 'Review CSV values');
    assert.deepEqual(
      (await reviewRows()).map((row) => row.field),
      ['Value', 'Description', 'Symbol'],
    );
    await tick('Value');
    await tick('Symbol');
    await clickButton(FORM, 'Accept the ticked values');
    await waitForText(
      browser,
      '.team-problem',
      /^Your choice for the part details was saved\. Your choice for the symbol and footprint was not: .*unarchive revision 2 instead/,
    );
    // The saved details left the form; only the symbol is still open.
    await browser.waitUntil(async () => (await reviewRows()).length === 1, {
      timeout: 5_000,
      timeoutMsg: 'the saved details stayed in the form',
    });
    assert.deepEqual(
      (await reviewRows()).map((row) => row.field),
      ['Symbol'],
    );
    // Keeping the current symbol now sends only that group.
    await clickButton(FORM, 'Keep current values');
    await waitForText(
      browser,
      NOTICE,
      'Kept the current values. You are asked about these CSV values again only if the CSV changes.',
    );
    assert.ok(await noCsvNotice());
    assert.equal(await fieldValue('Value'), '100R');
    assert.equal(await sourceFact(), 'Team catalog, revision 3 of this part');
    // The review's own view, or a live update right after it, shows the
    // new history entry.
    await waitForText(browser, '#team-tab-history', 'History (7)');
  });
});
