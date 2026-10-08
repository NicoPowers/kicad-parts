// A saved kicad-cli that runs but cannot report its version (a damaged
// install) is reported as broken, with its own message and the fix.

import assert from 'node:assert/strict';
import { after, before, describe, it } from 'node:test';
import {
  assertAccessible,
  dataDirWithOverride,
  fakeKicadCli,
  launchApp,
  screenshot,
  textOf,
  waitForText,
} from '../harness.mjs';

const PROBLEM = 'section.kicad-problem[role="alert"]';

describe('kicad-cli is broken', () => {
  let browser;

  before(async () => {
    ({ browser } = await launchApp({ dataDir: dataDirWithOverride(fakeKicadCli(null)) }));
  });

  after(async () => {
    await browser?.deleteSession();
  });

  it('shows what kicad-cli said and how to fix it', async () => {
    await waitForText(browser, `${PROBLEM} h2`, 'kicad-cli did not respond');
    assert.match(await textOf(browser, PROBLEM), /Reinstall or repair KiCad 10/);
    assert.match(await textOf(browser, `${PROBLEM} .kicad-problem-details`), /this installation is damaged/);
    assert.ok(await (await browser.$('button=Clear saved path')).isExisting());
    await assertAccessible(browser, '21-kicad-broken');
    await screenshot(browser, '21-kicad-broken');
  });
});
