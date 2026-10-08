// An older KiCad saved as the user's kicad-cli is reported as unsupported,
// with the version found and how to fix it.

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

describe('KiCad is too old', () => {
  let browser;

  before(async () => {
    // A stand-in kicad-cli that reports KiCad 9, saved earlier as the
    // user's choice (for example before KiCad was downgraded).
    ({ browser } = await launchApp({ dataDir: dataDirWithOverride(fakeKicadCli('9.0.7')) }));
  });

  after(async () => {
    await browser?.deleteSession();
  });

  it('names the unsupported version and the fix', async () => {
    await waitForText(browser, `${PROBLEM} h2`, 'KiCad 9.0.7 is not supported');
    assert.match(await textOf(browser, PROBLEM), /This app needs KiCad 10\. Install KiCad 10/);
    assert.match(await textOf(browser, `${PROBLEM} .kicad-problem-details`), /kicad-cli\.exe/);
    assert.ok(await (await browser.$('button=Clear saved path')).isExisting());
    await assertAccessible(browser, '20-kicad-unsupported');
    await screenshot(browser, '20-kicad-unsupported');
  });
});
