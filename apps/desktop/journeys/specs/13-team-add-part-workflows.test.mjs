// Add part in real workflows (QA pass 2, 2026-10-12): the form at a narrow
// window (960 px), completed with the keyboard alone, using the Catalog's
// own capacitor symbol instead of the download's; then someone else edits
// the new part and the app shows it by itself.

import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { after, before, describe, it } from 'node:test';
import {
  FOOTPRINT,
  SYMBOL,
  assertAccessible,
  capture,
  editAsSomeoneElse,
  launchApp,
  record,
  repoRoot,
  screenshotsDir,
  startTeamServer,
  teamSignIn,
  tempDir,
  textOf,
  waitForLibraries,
  waitForLiveUpdates,
  waitForPreviewImages,
  waitForText,
} from '../harness.mjs';

const MPN = 'GRM188R71H104KA93D';
const IPN = 'CAP-0004-0001';
const BADGE = '.team-settings > summary';
const NOTICE = '.team-notice';

// ---- A small ZIP writer (stored entries only), as in journey 12 ----------

const CRC_TABLE = Array.from({ length: 256 }, (_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});

function crc32(bytes) {
  let crc = 0xffffffff;
  for (const byte of bytes) crc = CRC_TABLE[(crc ^ byte) & 0xff] ^ (crc >>> 8);
  return (crc ^ 0xffffffff) >>> 0;
}

function zipOf(entries) {
  const locals = [];
  const centrals = [];
  let offset = 0;
  for (const [name, data] of entries) {
    const nameBytes = Buffer.from(name, 'utf8');
    const crc = crc32(data);
    const local = Buffer.alloc(30);
    local.writeUInt32LE(0x04034b50, 0);
    local.writeUInt16LE(20, 4);
    local.writeUInt32LE(crc, 14);
    local.writeUInt32LE(data.length, 18);
    local.writeUInt32LE(data.length, 22);
    local.writeUInt16LE(nameBytes.length, 26);
    const central = Buffer.alloc(46);
    central.writeUInt32LE(0x02014b50, 0);
    central.writeUInt16LE(20, 4);
    central.writeUInt16LE(20, 6);
    central.writeUInt32LE(crc, 16);
    central.writeUInt32LE(data.length, 20);
    central.writeUInt32LE(data.length, 24);
    central.writeUInt16LE(nameBytes.length, 28);
    central.writeUInt32LE(offset, 42);
    locals.push(local, nameBytes, data);
    centrals.push(central, nameBytes);
    offset += local.length + nameBytes.length + data.length;
  }
  const directory = Buffer.concat(centrals);
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0);
  end.writeUInt16LE(entries.length, 8);
  end.writeUInt16LE(entries.length, 10);
  end.writeUInt32LE(directory.length, 12);
  end.writeUInt32LE(offset, 16);
  return Buffer.concat([...locals, directory, end]);
}

/** An Ultra Librarian style download with a long symbol name and file names. */
function download(folder) {
  const read = (file) => fs.readFileSync(path.join(repoRoot, file), 'utf8');
  const name = 'GRM188R71H104KA93D_Murata_Electronics_Chip_Monolithic_Ceramic_Capacitor';
  const symbol = read('symbols/g-cap.kicad_sym')
    .replaceAll('(symbol "C_', `(symbol "${name}_`)
    .replace('(symbol "C"', `(symbol "${name}"`)
    .replace('"Value" ".Value"', '"Value" "0.1uF"')
    .replace('"Description" "Unpolarized capacitor"', '"Description" "CAP CER 0.1UF 50V X7R 0603"')
    .replace(
      '\t\t(property "Reference"',
      `\t\t(property "Manufacturer" "Murata Electronics" (at 0 0 0) (effects (hide yes)))\n` +
        `\t\t(property "MPN" "${MPN}" (at 0 0 0) (effects (hide yes)))\n` +
        '\t\t(property "Reference"',
    );
  const footprint = read('footprints/g-Capacitor_SMD.pretty/C_0603_1608Metric.kicad_mod')
    .replace('(footprint "C_0603_1608Metric"', '(footprint "CAP_GRM18_MUR_0603_Long_Vendor_Name"')
    .replace(
      '${KICAD9_3DMODEL_DIR}/Capacitor_SMD.3dshapes/C_0603_1608Metric.step',
      'CAP_GRM18_MUR_0603_Long_Vendor_Name.step',
    );
  const zip = zipOf([
    [`KiCAD/${MPN}/${name}.kicad_sym`, Buffer.from(symbol, 'utf8')],
    [`KiCAD/${MPN}/footprints.pretty/CAP_GRM18_MUR_0603_Long_Vendor_Name.kicad_mod`, Buffer.from(footprint, 'utf8')],
    [`KiCAD/${MPN}/CAP_GRM18_MUR_0603_Long_Vendor_Name.step`, Buffer.from('ISO-10303-21;\nEND-ISO-10303-21;\n')],
  ]);
  const file = path.join(folder, `ul_${MPN}_Murata_Electronics_download_2026-10-12.zip`);
  fs.writeFileSync(file, zip);
  return file;
}

/** A download whose footprint has no package group in its name and pads no Catalog footprint has. */
function oddPads(folder) {
  const read = (file) => fs.readFileSync(path.join(repoRoot, file), 'utf8');
  const symbol = read('symbols/g-cap.kicad_sym')
    .replaceAll('(symbol "C_', '(symbol "C_OddPads_')
    .replace('(symbol "C"', '(symbol "C_OddPads"')
    .replace(
      '\t\t(property "Reference"',
      `\t\t(property "Manufacturer" "Acme" (at 0 0 0) (effects (hide yes)))\n` +
        `\t\t(property "MPN" "ODD-PADS-1" (at 0 0 0) (effects (hide yes)))\n` +
        '\t\t(property "Reference"',
    );
  const footprint = read('footprints/g-Capacitor_SMD.pretty/C_0603_1608Metric.kicad_mod')
    .replace('(footprint "C_0603_1608Metric"', '(footprint "CAP_ODD_PADS"')
    .replaceAll('(at -0.775 0)', '(at -0.9 0)');
  const file = path.join(folder, 'odd-pads.zip');
  fs.writeFileSync(
    file,
    zipOf([
      ['C_OddPads.kicad_sym', Buffer.from(symbol, 'utf8')],
      ['CAP_ODD_PADS.kicad_mod', Buffer.from(footprint, 'utf8')],
    ]),
  );
  return file;
}

describe('team catalog: Add part in real workflows', () => {
  let server;
  let browser;
  let editor;
  let otherToken;
  let files;

  before(async () => {
    server = await startTeamServer({ csv: ['g-res', 'g-cap'] });
    const [first, other] = server.accounts;
    editor = first;
    otherToken = await teamSignIn(server.url, other);
    files = tempDir('download');
    ({ browser } = await launchApp({
      args: [`--team-server=${server.url}`, `--team-email=${editor.email}`],
    }));
  });

  after(async () => {
    await browser?.deleteSession();
    await server?.stop();
  });

  async function valueOf(selector) {
    return browser.execute((s) => document.querySelector(s)?.value ?? null, selector);
  }

  async function activeId() {
    return browser.execute(() => {
      const el = document.activeElement;
      if (!el) return null;
      return el.id || `${el.tagName.toLowerCase()}:${(el.textContent || '').trim().slice(0, 30)}`;
    });
  }

  /** Presses Tab until `done(id)` holds; returns every stop on the way. */
  async function tabUntil(done, most = 80) {
    const stops = [];
    for (let i = 0; i < most; i++) {
      await browser.keys(['Tab']);
      const id = await activeId();
      stops.push(id);
      if (done(id)) return stops;
    }
    assert.fail(`Tab never reached the target; stops: ${stops.join(', ')}`);
  }

  it('signs in and opens Add part', async () => {
    await (await browser.$(BADGE)).click();
    await (await browser.$('aria/Password')).setValue(editor.password);
    await (await browser.$('form[aria-label="Sign in to the team catalog"] button[type="submit"]')).click();
    await waitForText(browser, BADGE, `Team catalog · ${editor.name}`);
    await waitForLiveUpdates(browser);
    await browser.execute(() => {
      document.querySelector('.team-settings').open = false;
    });
    await (await browser.$('#add-part-open')).click();
    await waitForText(browser, '#add-heading', 'Add part');
  });

  it('fits a 960 px window with long file and symbol names', async () => {
    await (await browser.$('#add-files')).addValue(download(files));
    await waitForText(browser, '#add-guess', /^Best guess: Capacitors/);
    // Check the layout people see once kicad-cli has drawn both files. The
    // drawings are the tallest part of the left column. Before they arrive,
    // a section's size says nothing about whether they fit (kicad-parts-da76).
    await waitForPreviewImages(browser, SYMBOL);
    await waitForPreviewImages(browser, FOOTPRINT);
    const outer = await browser.getWindowSize();
    const inner = await browser.execute(() => window.innerWidth);
    await browser.setWindowSize(960 + (outer.width - inner), outer.height);
    await browser.waitUntil(async () => (await browser.execute(() => window.innerWidth)) === 960, {
      timeout: 5_000,
      timeoutMsg: 'the window did not become 960 px wide',
    });
    const layout = await browser.execute(() => {
      const wide = (el) => el && el.scrollWidth > el.clientWidth + 1;
      const form = document.querySelector('.add-form');
      const offRight = [...document.querySelectorAll('.add-form input, .add-form select, .add-form textarea, .add-form button')]
        .filter((el) => el.offsetParent !== null)
        .filter((el) => el.getBoundingClientRect().right > window.innerWidth + 1)
        .map((el) => el.id || el.textContent.trim());
      // A drawing taller than its section spills over the next one: the
      // symbol over the Footprint heading, the footprint over the form.
      const previewsSpillOver = [...document.querySelectorAll('.part-previews .preview')]
        .filter((section) => section.scrollHeight > section.clientHeight + 1)
        .map((section) => section.getAttribute('aria-labelledby'));
      return {
        pageScrollsSideways: wide(document.scrollingElement),
        formScrollsSideways: wide(form),
        offRight,
        previewsSpillOver,
      };
    });
    record({ event: 'add-part-960', layout });
    fs.mkdirSync(screenshotsDir, { recursive: true });
    await capture(browser, path.join(screenshotsDir, '13-add-part-960-for-review.png'));
    assert.deepEqual(layout, { pageScrollsSideways: false, formScrollsSideways: false, offRight: [], previewsSpillOver: [] });
    await assertAccessible(browser, '13-add-part-960');
    await browser.setWindowSize(outer.width, outer.height);
    await browser.waitUntil(async () => (await browser.execute(() => window.innerWidth)) === 1280, {
      timeout: 5_000,
      timeoutMsg: 'the window did not go back to 1280 px',
    });
  });

  it("is completed with the keyboard alone, using the Catalog's own symbol", async () => {
    assert.equal(await valueOf('#add-ipn'), IPN);
    await browser.execute(() => document.getElementById('add-files').focus());
    // No CAD name is typed (kicad-parts-bc6f): Choose... opens the Libraries
    // browser for symbols, where the keyboard alone finds and chooses the
    // Catalog's capacitor symbol.
    const toSymbol = await tabUntil((id) => id === 'add-symbol-choose');
    record({ event: 'tab-stops-to-symbol', stops: toSymbol });
    await browser.keys(['Enter']);
    await waitForLibraries(browser);
    await browser.waitUntil(async () => (await browser.execute(() => document.activeElement?.id)) === 'library-search', {
      timeout: 5_000,
      timeoutMsg: 'the search box did not take the focus',
    });
    await browser.keys('Unpolarized capacitor'.split(''));
    // To the list, then down to g-cap's C, and Enter chooses it.
    await tabUntil((id) => id === 'library-items');
    let stops = 0;
    while (
      (await browser.execute(() => {
        const row = document.getElementById(document.getElementById('library-items').getAttribute('aria-activedescendant') ?? '');
        return row ? `${row.querySelector('.library-item-library')?.textContent}:${row.querySelector('.library-item-name').textContent}` : null;
      })) !== 'g-cap:C'
    ) {
      assert.ok(stops++ < 60, 'g-cap:C is not in the list');
      await browser.keys(['ArrowDown']);
    }
    await browser.keys(['Enter']);
    await waitForLibraries(browser, { open: false });
    await browser.waitUntil(async () => (await valueOf('#add-symbol')) === 'g-cap:C', {
      timeout: 20_000,
      timeoutMsg: `the symbol did not become g-cap:C: ${await valueOf('#add-symbol')}`,
    });
    // Focus is back on Choose..., ready for the next field.
    await browser.waitUntil(async () => (await browser.execute(() => document.activeElement?.id)) === 'add-symbol-choose', {
      timeout: 10_000,
      timeoutMsg: 'focus did not come back to Choose…',
    });
    // The vendor's footprint name says no package group; the Catalog's
    // footprint with the same pads gives one.
    assert.equal(await valueOf('#add-footprint'), 'g-Capacitor_SMD:CAP_GRM18_MUR_0603_Long_Vendor_Name');
    assert.doesNotMatch(await textOf(browser, '.add-form'), /Type the footprint's library/);
    await tabUntil((id) => id === 'add-reason');
    await browser.keys('Same part, our symbol'.split(''));
    const toSave = await tabUntil((id) => id === 'button:Save');
    record({ event: 'tab-stops-to-save', stops: toSave });
    await browser.keys(['Enter']);
    await waitForText(browser, NOTICE, `Added ${IPN} as revision 1.`, 30_000);
    await waitForText(browser, '#part-title', MPN);
    await waitForText(browser, `${SYMBOL} .provenance`, /^team catalog, symbol revision 1 · /, 60_000);
    const symbolRow = await browser.execute(() => {
      const row = [...document.querySelectorAll('.part-fields tr')].find((tr) => tr.querySelector('th')?.textContent.trim() === 'Symbol');
      return row?.querySelector('td')?.textContent.trim() ?? null;
    });
    assert.match(symbolRow ?? (await textOf(browser, '.part-fields')), /g-cap:C\b/);
  });

  it('shows by itself what someone else changes on the new part', async () => {
    await editAsSomeoneElse(server.url, otherToken, IPN, { Voltage: '50V' }, 'From the datasheet');
    await browser.waitUntil(
      async () =>
        (await browser.execute(() => document.querySelector('#team-tab-history')?.textContent.trim())) === 'History (2)',
      { timeout: 20_000, interval: 100, timeoutMsg: 'the new part never showed the other change' },
    );
    assert.match(await textOf(browser, '.part-fields'), /50V/);
  });

  it("suggests the footprint's group from the category chosen when its name says none", async () => {
    await (await browser.$('#add-part-open')).click();
    await waitForText(browser, '#add-heading', 'Add part');
    await (await browser.$('#add-files')).addValue(oddPads(files));
    await waitForText(browser, '#add-guess', /^Best guess: Capacitors/);
    assert.equal(await valueOf('#add-footprint'), 'g-Capacitor_SMD:CAP_ODD_PADS');
    // Another category, another group; a group typed by hand stays.
    await (await browser.$('#add-category')).selectByAttribute('value', 'res');
    await browser.waitUntil(async () => (await valueOf('#add-footprint')) === 'g-Resistor_SMD:CAP_ODD_PADS', {
      timeout: 5_000,
      timeoutMsg: `the footprint group did not follow the category: ${await valueOf('#add-footprint')}`,
    });
    // The footprint's library is chosen from our libraries, never typed.
    assert.equal(await browser.execute(() => document.getElementById('add-footprint').readOnly), true);
    await (await browser.$('#add-footprint-library')).selectByAttribute('value', 'g-Package_SO');
    await browser.waitUntil(async () => (await valueOf('#add-footprint')) === 'g-Package_SO:CAP_ODD_PADS', {
      timeout: 5_000,
      timeoutMsg: `the library chosen did not show: ${await valueOf('#add-footprint')}`,
    });
    await (await browser.$('#add-category')).selectByAttribute('value', 'cap');
    await waitForText(browser, '#add-ipn-hint', 'The next free IPN in this category. An IPN is never reused.');
    assert.equal(await valueOf('#add-footprint'), 'g-Package_SO:CAP_ODD_PADS');
  });
});
