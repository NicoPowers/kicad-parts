// KiCad-missing journey: the app is told to search only an empty folder, so
// it finds no kicad-cli. It must say how to fix that, keep search working,
// reject a wrong path, and recover once the user points it at kicad-cli.

import assert from 'node:assert/strict';
import { after, before, describe, it } from 'node:test';
import {
  FOOTPRINT,
  SYMBOL,
  assertAccessible,
  images,
  installedKicadCli,
  launchApp,
  screenshot,
  searchFor,
  selectResult,
  tempDir,
  textOf,
  waitForPreview,
  waitForText,
} from '../harness.mjs';

const PROBLEM = 'section.kicad-problem[role="alert"]';

async function useKicadPath(browser, value) {
  const input = await browser.$('aria/kicad-cli path or KiCad install folder');
  assert.equal(await input.getTagName(), 'input');
  await input.setValue(value);
  await (await browser.$(`${PROBLEM} button[type="submit"]`)).click();
}

describe('KiCad is missing', () => {
  let browser;
  const emptyRoot = tempDir('no-kicad');

  before(async () => {
    ({ browser } = await launchApp({ args: [`--kicad-search-root=${emptyRoot}`] }));
  });

  after(async () => {
    await browser?.deleteSession();
  });

  it('explains how to install KiCad or point to kicad-cli', async () => {
    await waitForText(browser, `${PROBLEM} h2`, 'KiCad 10 was not found');
    assert.match(await textOf(browser, PROBLEM), /Install KiCad 10 from kicad\.org\/download/);
    assert.match(await textOf(browser, `${PROBLEM} .kicad-problem-details`), /no-kicad/);
    assert.equal(await textOf(browser, '.kicad-badge-problem'), 'KiCad needs attention');
    // Browsing the catalog does not need KiCad.
    await waitForText(browser, '#result-count', '178 matching parts');
    await assertAccessible(browser, '10-kicad-missing');
    await screenshot(browser, '10-kicad-missing');
  });

  it('tells the user previews need KiCad when a part is selected', async () => {
    await searchFor(browser, 'stm32h723', '1 matching part');
    await selectResult(browser, 'STM32H723ZGT6');
    await waitForPreview(browser, SYMBOL);
    await waitForPreview(browser, FOOTPRINT);
    assert.equal(await textOf(browser, `${SYMBOL} .problem.quiet .problem-title`), 'Previews need KiCad 10');
    // Only the KiCad panel is announced as an alert.
    assert.equal(await browser.execute(() => document.querySelectorAll('[role="alert"]').length), 1);
    assert.equal(await textOf(browser, `${FOOTPRINT} .problem-title`), 'Previews need KiCad 10');
    await assertAccessible(browser, '11-kicad-missing-part');
    await screenshot(browser, '11-kicad-missing-part');
  });

  it('rejects a path that is not kicad-cli without saving it', async () => {
    const wrong = `${emptyRoot}\\kicad-cli.exe`;
    await useKicadPath(browser, wrong);
    await waitForText(browser, `${PROBLEM} .override-rejected`, /^Not saved: .*does not exist$/);
    // Nothing changed: KiCad is still missing, nothing is saved, and the
    // typed path stays in the box for correction.
    assert.equal(await textOf(browser, `${PROBLEM} h2`), 'KiCad 10 was not found');
    assert.equal(await (await browser.$('#kicad-cli-path')).getValue(), wrong);
    assert.equal(await (await browser.$('button=Clear saved path')).isExisting(), false);
    await assertAccessible(browser, '12-override-rejected');
    await screenshot(browser, '12-override-rejected');
  });

  it('recovers when pointed at the installed kicad-cli', async () => {
    await useKicadPath(browser, installedKicadCli());
    await waitForText(browser, '.kicad-badge-ready', /^KiCad 10\.\d+\.\d+$/);
    assert.equal(await browser.execute((s) => document.querySelector(s), PROBLEM), null);
    // The selected part's previews reload by themselves.
    await waitForPreview(browser, SYMBOL);
    await waitForPreview(browser, FOOTPRINT);
    const units = await images(browser, `${SYMBOL} figure img`);
    assert.ok(units.length > 1 && units.every((unit) => unit.loaded));
    assert.ok((await images(browser, `${FOOTPRINT} figure img`))[0].loaded);
    await assertAccessible(browser, '13-override-recovered');
    await screenshot(browser, '13-override-recovered');
  });
});
