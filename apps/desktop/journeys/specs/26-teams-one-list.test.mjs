// One part list over several teams (kicad-parts-8a71): the person groups
// the list by team, adds a part to the team they choose in Add part, and
// picks the team the library check and the KiCad libraries panel work on
// while the list shows every team. The list's team filter and grouping
// are kept after a restart. Sign-ins stay in the app's memory
// (--sign-ins), never the person's credential store.

import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { after, before, describe, it } from 'node:test';
import {
  FOOTPRINT,
  SYMBOL,
  assertAccessible,
  launchApp,
  repoRoot,
  screenshot,
  searchFor,
  startTeamServer,
  tempDir,
  textOf,
  waitForLibraryCheck,
  waitForText,
  zipOf,
} from '../harness.mjs';

const MPN = 'CL10B104KB8NNNC';
const IPN = 'CAP-0004-0001';
const BADGE = '.team-settings > summary';

/** The repository's capacitor symbol and footprint, renamed, as a vendor ZIP. */
function download(folder) {
  const read = (file) => fs.readFileSync(path.join(repoRoot, file), 'utf8');
  const symbol = read('symbols/g-cap.kicad_sym')
    .replaceAll('(symbol "C_', '(symbol "C_New_')
    .replace('(symbol "C"', '(symbol "C_New"')
    .replace('"Value" ".Value"', '"Value" "100n"')
    .replace('"Description" "Unpolarized capacitor"', '"Description" "CAP CER 0.1UF 50V X7R 0603"')
    .replace(
      '\t\t(property "Reference"',
      `\t\t(property "Manufacturer_Name" "Samsung Electro-Mechanics" (at 0 0 0) (effects (hide yes)))\n` +
        `\t\t(property "Manufacturer_Part_Number" "${MPN}" (at 0 0 0) (effects (hide yes)))\n` +
        '\t\t(property "Reference"',
    );
  const footprint = read('footprints/g-Capacitor_SMD.pretty/C_0603_1608Metric.kicad_mod')
    .replace('(footprint "C_0603_1608Metric"', '(footprint "C_0603_New"')
    .replace('${KICAD9_3DMODEL_DIR}/Capacitor_SMD.3dshapes/C_0603_1608Metric.step', '${KIPRJMOD}/3D/C_0603_New.stp');
  const file = path.join(folder, `${MPN}.zip`);
  fs.writeFileSync(
    file,
    zipOf([
      ['CL10B104/KiCad/C_New.kicad_sym', Buffer.from(symbol, 'utf8')],
      ['CL10B104/KiCad/C_0603_New.kicad_mod', Buffer.from(footprint, 'utf8')],
      ['CL10B104/3D/C_0603_New.stp', Buffer.from('ISO-10303-21;\nEND-ISO-10303-21;\n')],
    ]),
  );
  return file;
}

describe('one part list over several teams', () => {
  let acme;
  let beta;
  let app;
  let browser;

  before(async () => {
    [acme, beta] = await Promise.all([
      startTeamServer({ csv: ['g-res', 'g-cap'], teamCode: 'AC' }),
      startTeamServer({ csv: ['g-res', 'g-cap'], teamCode: 'BE' }),
    ]);
    app = await launchApp({ args: [`--team-server=${acme.url}`, `--team-email=${acme.accounts[0].email}`] });
    ({ browser } = app);
    await signInToBoth();
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

  async function signInWithForm(server) {
    const account = server.accounts[0];
    await (await browser.$('#team-server')).setValue(server.url);
    await (await browser.$('#team-email')).setValue(account.email);
    await (await browser.$('#team-password')).setValue(account.password);
    await (await browser.$('form[aria-label="Sign in to the team catalog"] button[type="submit"]')).click();
  }

  /** Signs in to acme, then adds beta, and waits until both are live. */
  async function signInToBoth() {
    await waitForText(browser, '#result-count', /matching parts$/);
    await setMenuOpen(true);
    await signInWithForm(acme);
    await waitForText(browser, BADGE, `Team catalog · ${acme.accounts[0].name}`);
    await setMenuOpen(true);
    await (await browser.$('#team-add')).click();
    await signInWithForm(beta);
    await waitForText(browser, BADGE, 'Teams · 2 live', 30_000);
    await setMenuOpen(false);
    await waitForLibraryCheck(browser);
  }

  /** The sections of the part list, as `name (count)`. */
  async function sections() {
    return browser.execute(() =>
      [...document.querySelectorAll('.results .section-toggle')].map(
        (toggle) =>
          `${toggle.querySelector('.section-name').textContent} (${toggle.querySelector('.section-count').firstChild.textContent})`,
      ),
    );
  }

  /** Chooses `value` in the list box `id` from the keyboard, as a person would. */
  async function choose(id, value) {
    const { from, to } = await browser.execute(
      (select, wanted) => {
        const box = document.getElementById(select);
        box.focus();
        const values = [...box.options].map((option) => option.value);
        return { from: values.indexOf(box.value), to: values.indexOf(wanted) };
      },
      id,
      value,
    );
    assert.ok(to >= 0, `${id} has no option ${value}`);
    for (let step = 0; step < Math.abs(to - from); step += 1) {
      await browser.keys(to > from ? 'ArrowDown' : 'ArrowUp');
    }
    await browser.waitUntil(async () => (await (await browser.$(`#${id}`)).getValue()) === value, {
      timeout: 5_000,
      timeoutMsg: `${id} never chose ${value}`,
    });
    assert.equal(await browser.execute(() => document.activeElement?.id), id, `${id} keeps the focus`);
  }

  it('groups the parts by team, one section each, from the keyboard', async () => {
    assert.equal(await (await browser.$('label[for="group-by"]')).getText(), 'Group by');
    assert.equal(await (await browser.$('#group-by')).getValue(), 'category');
    await choose('group-by', 'team');
    // One section per team, in the order of the Teams menu, and the parts
    // only the CSV files have.
    await browser.waitUntil(async () => (await sections())[0]?.startsWith('AC ('), {
      timeout: 10_000,
      timeoutMsg: 'the list never grouped by team',
    });
    const grouped = await sections();
    assert.deepEqual(
      grouped.map((section) => section.replace(/ \(.*/, '')),
      ['AC', 'BE', 'CSV files'],
    );
    assert.equal(grouped[0].replace('AC', ''), grouped[1].replace('BE', ''), 'both teams have the same parts');
    // A team section opens like a category's. Its name says the team, so
    // its rows repeat no team mark.
    await (await browser.$('.results .section-toggle')).click();
    await browser.waitUntil(
      async () => (await browser.execute(() => document.querySelectorAll('.results button.result').length)) > 0,
      { timeout: 10_000, timeoutMsg: "AC's section never opened" },
    );
    assert.equal(await browser.execute(() => document.querySelectorAll('.results .team-chip').length), 0);
    // The header says where the list's parts come from.
    assert.equal(await textOf(browser, '.catalog-summary'), 'Parts from 2 teams and 21 CSV files');
    await assertAccessible(browser, '100-teams-group-by-team');
    await screenshot(browser, '100-teams-group-by-team', { blankOut: ['.team-server'] });
    // Back to categories, and the AC section closed again.
    await (await browser.$('.results .section-toggle')).click();
    await choose('group-by', 'category');
  });

  it('asks which team a new part goes into', async () => {
    await (await browser.$('#add-part-open')).click();
    await waitForText(browser, '#add-heading', 'Add part');
    // Every team the person can add to; the first one at first.
    assert.equal(await (await browser.$('label[for="add-team"]')).getText(), 'Team');
    assert.deepEqual(
      await browser.execute(() => [...document.querySelectorAll('#add-team option')].map((o) => o.textContent)),
      ['AC', 'BE'],
    );
    assert.equal(await (await browser.$('#add-team')).getValue(), acme.url);
    // Files chosen, then another team at once, before they are read: they
    // are read for the new team, and the form never sticks on Reading….
    await (await browser.$('#add-files')).addValue(download(tempDir('download')));
    await choose('add-team', beta.url);
    await waitForText(browser, '#add-guess', /^Best guess: Capacitors, because /);
    await browser.waitUntil(
      async () => !(await browser.execute(() => document.querySelector('.add-files [aria-busy="true"]'))),
      { timeout: 20_000, timeoutMsg: 'the form stayed on Reading…' },
    );
    // BE's next free IPN.
    assert.equal(await browser.execute(() => document.querySelector('#add-ipn').value), IPN);
    await browser.waitUntil(
      async () =>
        browser.execute(
          (sections) => sections.every((s) => document.querySelector(`${s} .file-preview img`)),
          [SYMBOL, FOOTPRINT],
        ),
      { timeout: 60_000, timeoutMsg: 'the drawings never showed' },
    );
    await browser.execute(() => {
      document.querySelector('.part-fields').scrollTop = 0;
    });
    await assertAccessible(browser, '101-team-add-part-team');
    await screenshot(browser, '101-team-add-part-team');
    // Saved to BE only: the list has it once, marked BE.
    await (await browser.$('#add-reason')).setValue('New 100n capacitor for the BE board');
    await browser.execute(() =>
      [...document.querySelectorAll('.add-form button')].find((b) => b.textContent.trim() === 'Save').click(),
    );
    await waitForText(browser, '.team-notice', `Added ${IPN} as revision 1.`, 30_000);
    await searchFor(browser, MPN, '1 matching part');
    assert.equal(
      await browser.execute(() => document.querySelector('.results button.result').dataset.team),
      beta.url,
    );
    // Add part starts on the team used last.
    await (await browser.$('#add-part-open')).click();
    await waitForText(browser, '#add-heading', 'Add part');
    assert.equal(await (await browser.$('#add-team')).getValue(), beta.url);
    await browser.execute(() =>
      [...document.querySelectorAll('.add-form button')].find((b) => b.textContent.trim() === 'Cancel')?.click(),
    );
    await searchFor(browser, '', /matching parts$/);
  });

  it('lets the library check and the KiCad libraries choose their team', async () => {
    // With every team shown, the check names the team it checked, and its
    // view has a Team list box of its own.
    await waitForText(browser, '#library-check-open', /^AC library check: /);
    await (await browser.$('#library-check-open')).click();
    await waitForText(browser, '#library-check-title', 'Library check');
    await choose('library-check-team', beta.url);
    await waitForText(browser, '#library-check-open', /^BE library check: /, 30_000);
    await (await browser.$('#library-check-close')).click();
    // The KiCad libraries panel too.
    await browser.execute(() => {
      document.querySelector('.kicad-settings').open = true;
    });
    assert.deepEqual(
      await browser.execute(() =>
        [...document.querySelectorAll('#kicad-libraries-team option')].map((o) => o.textContent),
      ),
      ['AC', 'BE'],
    );
    await browser.execute(() => {
      document.querySelector('.kicad-settings').open = false;
    });
    // One team chosen in the part list: both use it, with no list box.
    await choose('team-filter', acme.url);
    await waitForText(browser, '#library-check-open', /^AC library check: /, 30_000);
    assert.equal(await browser.execute(() => document.querySelector('#kicad-libraries-team')), null);
  });

  it('keeps the team filter and grouping after a restart', async () => {
    await choose('team-filter', beta.url);
    await choose('group-by', 'team');
    await browser.deleteSession();
    ({ browser } = await launchApp({ dataDir: app.dataDir }));
    // Sign-ins stayed in the last run's memory: sign in to both again.
    for (const [server, code] of [
      [acme, 'AC'],
      [beta, 'BE'],
    ]) {
      await setMenuOpen(true);
      await browser.waitUntil(
        async () =>
          browser.execute(
            (wanted) =>
              [...document.querySelectorAll('.team-row')]
                .find((row) => row.querySelector('.team-name strong')?.textContent === wanted)
                ?.querySelector('.team-actions button')?.textContent.trim() === 'Sign in',
            code,
          ),
        { timeout: 20_000, timeoutMsg: `${code} never offered Sign in` },
      );
      await browser.execute(
        (wanted) =>
          [...document.querySelectorAll('.team-row')]
            .find((row) => row.querySelector('.team-name strong')?.textContent === wanted)
            .querySelector('.team-actions button')
            .click(),
        code,
      );
      // The form fills in the address; wait until it is this team's.
      await browser.waitUntil(async () => (await (await browser.$('#team-server')).getValue()) === server.url, {
        timeout: 10_000,
        timeoutMsg: `the form never offered ${code}`,
      });
      await (await browser.$('#team-password')).setValue(server.accounts[0].password);
      await (await browser.$('form[aria-label="Sign in to the team catalog"] button[type="submit"]')).click();
      await setMenuOpen(true);
      await browser.waitUntil(
        async () =>
          (
            await browser.execute(
              (wanted) =>
                [...document.querySelectorAll('.team-row')]
                  .find((row) => row.querySelector('.team-name strong')?.textContent === wanted)
                  ?.querySelector('.team-state')?.textContent ?? '',
              code,
            )
          ).startsWith('Signed in as'),
        { timeout: 30_000, timeoutMsg: `${code} never signed in again` },
      );
    }
    await browser
      .waitUntil(async () => (await textOf(browser, BADGE)) === 'Teams · 2 live', { timeout: 30_000 })
      .catch(async () =>
        assert.fail(
          `both teams never signed in again: ${JSON.stringify(
            await browser.execute(() => [...document.querySelectorAll('.team-row')].map((row) => row.textContent)),
          )}`,
        ),
      );
    await setMenuOpen(false);
    assert.equal(await (await browser.$('#team-filter')).getValue(), beta.url);
    assert.equal(await (await browser.$('#group-by')).getValue(), 'team');
    await browser.waitUntil(async () => (await sections()).length === 1, {
      timeout: 10_000,
      timeoutMsg: `only BE's section should show: ${JSON.stringify(await sections())}`,
    });
    assert.match((await sections())[0], /^BE \(/);
    assert.equal(await textOf(browser, '.results .section-name'), 'BE');
  });

  it("keeps a half-filled part on its team while that team is offline", async () => {
    await (await browser.$('#add-part-open')).click();
    await waitForText(browser, '#add-heading', 'Add part');
    // The part list shows BE, so Add part starts on BE.
    assert.equal(await (await browser.$('#add-team')).getValue(), beta.url);
    await (await browser.$('#add-files')).addValue(download(tempDir('download')));
    await waitForText(browser, '#add-guess', /^Best guess: Capacitors, because /);
    const betaUrl = beta.url;
    await beta.stop();
    beta = null;
    await waitForText(browser, '#add-team-problem', 'BE is offline. You can save when it is back.', 30_000);
    // The form stays on BE, and Save waits for it.
    assert.equal(await (await browser.$('#add-team')).getValue(), betaUrl);
    assert.equal(
      await browser.execute(() =>
        [...document.querySelectorAll('.add-form button[type="submit"]')].every((button) => button.disabled),
      ),
      true,
    );
  });
});
