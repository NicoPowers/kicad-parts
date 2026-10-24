// Library repo journey (kicad-parts-ab30): a client keeps its KiCad
// library in a git repo. The person links the client's team to it from
// their own clone. The app reads the clone's remote, makes a copy of its
// own, tries a push and says "Can read and write.". When the repo goes
// away, Check again says which step failed and what to check, with git's
// words under Details. The repo is a bare repo in a folder: the journey
// never reaches a git host or uses an SSH key, and git reads only the
// run's own empty settings file (run-journeys.mjs).

import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { after, before, describe, it } from 'node:test';
import { assertAccessible, launchApp, screenshot, startTeamServer, tempDir, waitForText } from '../harness.mjs';

const BADGE = '.team-settings > summary';
/** Server addresses hold ports that change from run to run. */
const ADDRESSES = ['.team-server'];
const REPO = '.team-row .team-repo';

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

describe('a team linked to its library repo', () => {
  let acme;
  let browser;
  let bare;
  let checkout;

  before(async () => {
    acme = await startTeamServer({ csv: ['g-res'], teamCode: 'AC' });
    // The client's repo, as on a git host, and the person's own clone.
    const repos = tempDir('repos');
    bare = path.join(repos, 'client-library.git');
    fs.mkdirSync(bare);
    git(bare, 'init', '-q', '--bare');
    const work = path.join(repos, 'work');
    fs.mkdirSync(path.join(work, 'symbols'), { recursive: true });
    fs.writeFileSync(path.join(work, 'symbols', 'Client.kicad_sym'), '(kicad_symbol_lib (version 20241209))\n');
    git(work, 'init', '-q');
    git(work, 'add', '.');
    git(work, 'commit', '-q', '-m', 'First symbols');
    git(work, 'push', '-q', bare, 'HEAD:main');
    checkout = path.join(repos, 'my-checkout');
    git(repos, 'clone', '-q', bare, checkout);
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

  it('links the team from the person\'s clone and says it can read and write', async () => {
    await waitForText(browser, '#result-count', '178 matching parts');
    await setMenuOpen(true);
    await (await browser.$('#team-password')).setValue(acme.accounts[0].password);
    await (await browser.$('form[aria-label="Sign in to the team catalog"] button[type="submit"]')).click();
    await waitForText(browser, BADGE, `Team catalog · ${acme.accounts[0].name}`);
    await setMenuOpen(true);
    await waitForText(browser, `${REPO} .team-repo-name`, 'Library repo: None yet');
    const add = await browser.$(`${REPO} .team-actions button`);
    assert.equal(await add.getText(), 'Add…');
    await add.click();
    const field = await browser.$(`${REPO} input[type="text"]`);
    assert.equal(
      await browser.execute(() => document.activeElement?.closest('.team-repo-form') !== null),
      true,
      'Add… puts the cursor in the field',
    );
    await field.setValue(checkout);
    await (await browser.$(`${REPO} button[type="submit"]`)).click();
    await waitForText(browser, `${REPO} .team-repo-state`, 'Can read and write.', 60_000);
    assert.equal(await (await browser.$(`${REPO} .team-repo-which`)).getText(), 'client-library');
    assert.equal(await (await browser.$(`${REPO} .team-repo-host`)).getText(), 'on this computer');
    // The keyboard goes on to Check again.
    assert.equal(await browser.execute(() => document.activeElement?.textContent), 'Check again');
    // The person's clone is untouched; the app's copy is its own.
    assert.equal(git(checkout, 'status', '--porcelain'), '');
    await screenshot(browser, '103-team-library-repo', { blankOut: ADDRESSES });
    await assertAccessible(browser, '103-team-library-repo');
  });

  it('says which step failed and what to check when the repo is gone', async () => {
    fs.renameSync(bare, `${bare}-moved`);
    const checkAgain = await browser.$(`${REPO} .team-actions button`);
    assert.equal(await checkAgain.getText(), 'Check again');
    await checkAgain.click();
    await waitForText(
      browser,
      `${REPO} .team-repo-state`,
      "Can't read the repo: the repo was not found. Check that its folder is there and holds the repo.",
      60_000,
    );
    // git's own words wait under Details, closed.
    const details = await browser.$(`${REPO} .team-repo-details`);
    assert.equal(await details.getProperty('open'), false);
    assert.match(await browser.execute(() => document.querySelector('.team-repo-details pre')?.textContent ?? ''), /does not appear to be a git repository/);
    await screenshot(browser, '104-team-library-repo-problem', { blankOut: ADDRESSES });
    await assertAccessible(browser, '104-team-library-repo-problem');
    // Remove unlinks it, and the row offers Add… again.
    await (await browser.$(`${REPO} button[aria-label="Remove the library repo"]`)).click();
    await waitForText(browser, `${REPO} .team-repo-name`, 'Library repo: None yet');
    await setMenuOpen(false);
  });
});
