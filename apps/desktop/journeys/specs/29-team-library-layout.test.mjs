// Library layout journey (kicad-parts-e9a3): a client's library repo is
// laid out its own way. Lib tables sit in a kicad/ folder and name the
// libraries with ${KIPRJMOD}, 3D models sit in a folder of their own behind
// ${ACME_3D}, and an old copy of the symbol library has the same name.
// After linking, the team's row offers "Review library layout". The
// review shows what the scan found and what to check. The person keeps
// the old copy left out, as the app suggests, and chooses "Use this
// layout". The Libraries browser then lists the client's libraries in a
// group with the team's mark, read from the app's own copy. The repo is a bare repo in a folder; git
// reads only the run's own empty settings file (run-journeys.mjs).

import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { after, before, describe, it } from 'node:test';
import { assertAccessible, launchApp, screenshot, startTeamServer, tempDir, textOf, waitForText } from '../harness.mjs';

const BADGE = '.team-settings > summary';
/** Server addresses hold ports that change from run to run. */
const ADDRESSES = ['.team-server'];
const REPO = '.team-row .team-repo';
const LAYOUT = 'dialog.layout-dialog';
const LIBRARIES = 'dialog.libraries-dialog';

/** Runs git with a fixture identity, with no window, and fails loudly. */
function git(cwd, ...args) {
  const result = spawnSync(
    'git',
    ['-c', 'user.name=fixture', '-c', 'user.email=fixture@example.test', '-c', 'commit.gpgsign=false', '-c', 'core.autocrlf=false', '-c', 'init.defaultBranch=main', ...args],
    { cwd, encoding: 'utf8', windowsHide: true },
  );
  assert.equal(result.status, 0, `git ${args.join(' ')}: ${result.stderr}`);
  return result.stdout.trim();
}

/** A small symbol with a body and two pins. */
function symbol(name) {
  const pin = (number, y) =>
    `(pin passive line (at -5.08 ${y} 0) (length 2.54) (name "~" (effects (font (size 1.27 1.27)))) (number "${number}" (effects (font (size 1.27 1.27)))))`;
  return `(symbol "${name}" (property "Reference" "R" (at 0 5.08 0) (effects (font (size 1.27 1.27)))) (property "Value" "${name}" (at 0 -5.08 0) (effects (font (size 1.27 1.27)))) (property "Footprint" "Acme_FP:R_0603" (at 0 0 0) (effects (font (size 1.27 1.27)) (hide yes))) (property "Description" "${name}, a client symbol" (at 0 0 0) (effects (font (size 1.27 1.27)) (hide yes))) (symbol "${name}_0_1" (rectangle (start -2.54 1.27) (end 2.54 -1.27) (stroke (width 0.254) (type default)) (fill (type none)))) (symbol "${name}_1_1" ${pin('1', 1.27)} ${pin('2', -1.27)}))`;
}

/** A footprint with two pads and its 3D model links. */
function footprint(name, links) {
  const models = links.map((link) => `\t(model "${link}")\n`).join('');
  return `(footprint "${name}"\n\t(version 20241229)\n\t(layer "F.Cu")\n\t(attr smd)\n\t(pad "1" smd rect (at -0.8 0) (size 0.8 0.9) (layers "F.Cu"))\n\t(pad "2" smd rect (at 0.8 0) (size 0.8 0.9) (layers "F.Cu"))\n${models})\n`;
}

/** Writes `files` ({ path: text }) under `root`. */
function writeFiles(root, files) {
  for (const [file, text] of Object.entries(files)) {
    const full = path.join(root, ...file.split('/'));
    fs.mkdirSync(path.dirname(full), { recursive: true });
    fs.writeFileSync(full, text);
  }
}

describe('a client library repo laid out its own way', () => {
  let acme;
  let browser;
  let bare;

  before(async () => {
    acme = await startTeamServer({ teamCode: 'AC' });
    const repos = tempDir('repos');
    bare = path.join(repos, 'acme-library.git');
    fs.mkdirSync(bare);
    git(bare, 'init', '-q', '--bare');
    const work = path.join(repos, 'work');
    const library = (body) => `(kicad_symbol_lib (version 20241209) (generator "journey") ${body})\n`;
    writeFiles(work, {
      'kicad/sym-lib-table':
        '(sym_lib_table (version 7)\n  (lib (name "Acme")(type "KiCad")(uri "${KIPRJMOD}/../lib/acme_symbols.kicad_sym")(options "")(descr "")))\n',
      'kicad/fp-lib-table':
        '(fp_lib_table (version 7)\n  (lib (name "Acme_FP")(type "KiCad")(uri "${KIPRJMOD}/../lib/Acme.pretty")(options "")(descr "")))\n',
      'lib/acme_symbols.kicad_sym': library(symbol('Acme_R')),
      // An old copy, named like the library boards use.
      'archive/Acme.kicad_sym': library(symbol('Old_R')),
      'lib/Acme.pretty/R_0603.kicad_mod': footprint('R_0603', ['${ACME_3D}/R_0603.step']),
      'lib/Acme.pretty/C_0603.kicad_mod': footprint('C_0603', ['${ACME_3D}/C_0603.step']),
      'models/R_0603.step': 'ISO-10303-21;\nEND-ISO-10303-21;\n',
    });
    git(work, 'init', '-q');
    git(work, 'add', '.');
    git(work, 'commit', '-q', '-m', 'Client library');
    git(work, 'push', '-q', bare, 'HEAD:main');
    ({ browser } = await launchApp({
      args: [`--team-server=${acme.url}`, `--team-email=${acme.accounts[0].email}`],
    }));
  });

  after(async () => {
    await browser?.deleteSession();
    await acme?.stop();
  });

  async function setMenuOpen(open) {
    await browser.execute((wanted) => {
      document.querySelector('.team-settings').open = wanted;
    }, open);
  }

  it('links the repo, and the row offers to review its library layout', async () => {
    await waitForText(browser, '#result-count', 'Sign in to a team to see its parts.');
    await setMenuOpen(true);
    await (await browser.$('#team-password')).setValue(acme.accounts[0].password);
    await (await browser.$('form[aria-label="Sign in to the team catalog"] button[type="submit"]')).click();
    await waitForText(browser, BADGE, `Team catalog · ${acme.accounts[0].name}`);
    await waitForText(browser, '.team-row .team-name strong', 'AC');
    await setMenuOpen(true);
    await (await browser.$(`${REPO} .team-actions button`)).click();
    await (await browser.$(`${REPO} input[type="text"]`)).setValue(bare);
    await (await browser.$(`${REPO} button[type="submit"]`)).click();
    await waitForText(browser, `${REPO} .team-repo-state`, 'Can read and write.', 60_000);
    await waitForText(
      browser,
      `${REPO} .team-repo-layout .team-state`,
      'Library layout: not reviewed yet. Check where the repo keeps its libraries and 3D models.',
    );
    const review = await browser.$(`${REPO} .team-repo-layout button`);
    assert.equal(await review.getText(), 'Review library layout');
    await review.click();
  });

  it('shows what the scan found, and the person fixes only what it got wrong', async () => {
    await browser.waitUntil(async () => browser.execute((s) => document.querySelector(s)?.open === true, LAYOUT), {
      timeout: 10_000,
      timeoutMsg: 'Review library layout did not open',
    });
    await waitForText(browser, '#layout-title', 'Library layout of acme-library', 30_000);
    assert.equal(
      await textOf(browser, '.layout-summary'),
      '2 symbol libraries, 1 footprint library and 1 3D model in 1 folder.',
    );
    // The keyboard starts at the summary of what was found.
    assert.equal(await browser.execute(() => document.activeElement?.id), 'layout-summary');
    // The path variable points to the model folder the links match.
    assert.equal(
      await browser.execute(() => document.querySelector('#layout-variable-ACME_3D').selectedOptions[0].textContent),
      'models/',
    );
    const doubts = await browser.execute(() =>
      [...document.querySelectorAll('.layout-doubts h4')].map((h) => h.textContent),
    );
    assert.deepEqual(doubts, ['Libraries with the same name (1)', 'Missing 3D models (1)']);
    // Names stay as the repo's lib tables give them.
    const symbols = await browser.execute(() =>
      [...document.querySelectorAll('section[aria-labelledby="layout-symbols-title"] li')].map((li) => li.textContent),
    );
    assert.deepEqual(symbols, [
      'Acmearchive/Acme.kicad_sym',
      'Acmelib/acme_symbols.kicad_symnamed in a library table',
    ]);
    // The commit changes from run to run.
    await screenshot(browser, '107-team-library-layout-review', { blankOut: ['.layout-commit'] });
    await assertAccessible(browser, '107-team-library-layout-review');
    // The old copy, which no library table names, starts left out. Keeping
    // both is refused, since KiCad would use only one.
    assert.equal(await (await browser.$('#layout-symbols-0')).isSelected(), false);
    await (await browser.$('#layout-symbols-0')).click();
    await (await browser.$('#layout-use')).click();
    await waitForText(
      browser,
      '.layout-refused',
      '2 libraries are named Acme: archive/Acme.kicad_sym, lib/acme_symbols.kicad_sym. KiCad would use only one, so leave out the others.',
    );
    await (await browser.$('#layout-symbols-0')).click();
    await (await browser.$('#layout-use')).click();
    await browser.waitUntil(async () => browser.execute((s) => document.querySelector(s) === null, LAYOUT), {
      timeout: 30_000,
      timeoutMsg: 'Use this layout did not close the review',
    });
    await waitForText(
      browser,
      `${REPO} .team-repo-layout .team-state`,
      'Library layout: 1 symbol library and 1 footprint library.',
    );
    // The keyboard goes back to the button that opened the review.
    assert.equal(await browser.execute(() => document.activeElement?.textContent), 'Review library layout');
    await screenshot(browser, '108-team-library-layout-reviewed', { blankOut: ADDRESSES });
    await assertAccessible(browser, '108-team-library-layout-reviewed');
    await setMenuOpen(false);
  });

  it('lists the client\'s libraries in the Libraries browser, offline from the app\'s copy', async () => {
    // The host goes away: browsing reads the app's own copy.
    fs.renameSync(bare, `${bare}-moved`);
    await (await browser.$('#libraries-open')).click();
    await browser.waitUntil(async () => browser.execute((s) => document.querySelector(s)?.open === true, LIBRARIES), {
      timeout: 10_000,
      timeoutMsg: 'the Libraries browser did not open',
    });
    await waitForText(browser, '#library-group-team-0', 'Team AC acme-library (1)', 60_000);
    const groups = await browser.execute(() =>
      [...document.querySelectorAll('.library-group')].map((h) => h.textContent),
    );
    assert.match(groups[0], /^Ours \(\d+\)$/);
    assert.equal(groups[1], 'Team AC acme-library (1)');
    // The library keeps the name boards use.
    await browser.waitUntil(
      async () =>
        browser.execute(() => {
          const button = document.querySelector('ul[aria-labelledby="library-group-team-0"] .library-button');
          if (button?.querySelector('.library-count')?.textContent !== '1') return false;
          button.click();
          return button.querySelector('.library-name').textContent === 'Acme';
        }),
      { timeout: 60_000, interval: 250, timeoutMsg: 'the client library was never read' },
    );
    await browser.waitUntil(
      async () =>
        browser.execute(() => document.getElementById('library-items')?.getAttribute('aria-busy') === 'false'),
      { timeout: 20_000, interval: 25, timeoutMsg: 'the list never showed its answer' },
    );
    // KiCad's own libraries take a while to read, and until then the
    // count adds that more may appear; once every library is read, it is
    // the same each run.
    await browser.waitUntil(
      async () => browser.execute(() => document.querySelector('.libraries-progress')?.dataset.state === 'ready'),
      { timeout: 240_000, interval: 250, timeoutMsg: 'the libraries were never all read' },
    );
    await waitForText(browser, '#library-count', '1 symbol');
    const rows = await browser.execute(() =>
      [...document.querySelectorAll('#library-items .library-item-name')].map((n) => n.textContent),
    );
    assert.deepEqual(rows, ['Acme_R']);
    await screenshot(browser, '109-libraries-team-repo');
    await assertAccessible(browser, '109-libraries-team-repo');
    await (await browser.$('#libraries-close')).click();
  });
});
