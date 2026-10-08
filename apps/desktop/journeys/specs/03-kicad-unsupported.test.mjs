// An older KiCad saved as the user's kicad-cli is reported as unsupported,
// with the version found and how to fix it.

import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { after, before, describe, it } from 'node:test';
import { assertAccessible, launchApp, screenshot, tempDir, textOf, waitForText } from '../harness.mjs';

const PROBLEM = 'section.kicad-problem[role="alert"]';

describe('KiCad is too old', () => {
  let browser;

  before(async () => {
    // A stand-in kicad-cli that reports KiCad 9, saved as the user's choice.
    const fake = path.join(tempDir('kicad9'), 'kicad-cli.cmd');
    fs.writeFileSync(fake, '@echo 9.0.7\r\n');
    const dataDir = tempDir('data');
    fs.mkdirSync(path.join(dataDir, 'config'), { recursive: true });
    fs.writeFileSync(
      path.join(dataDir, 'config', 'settings.json'),
      JSON.stringify({ kicad_cli_override: fake }),
    );
    ({ browser } = await launchApp({ dataDir }));
  });

  after(async () => {
    await browser?.deleteSession();
  });

  it('names the unsupported version and the fix', async () => {
    await waitForText(browser, `${PROBLEM} h2`, 'KiCad 9.0.7 is not supported');
    assert.match(await textOf(browser, PROBLEM), /This app needs KiCad 10\. Install KiCad 10/);
    assert.match(await textOf(browser, `${PROBLEM} .kicad-problem-details`), /kicad-cli\.cmd/);
    assert.ok(await (await browser.$('button=Clear saved path')).isExisting());
    await assertAccessible(browser, '20-kicad-unsupported');
    await screenshot(browser, '20-kicad-unsupported');
  });
});
