// Add part (kicad-parts-9693): choose a downloaded ZIP file (built here,
// in the layout SamacSys uses, from the repository's own capacitor symbol
// and footprint), see the guessed category with its reason and the next
// free IPN, save, and find the new part in the search with its drawings
// and history. Then a download whose MPN the Catalog already has shows
// that part, which opens.

import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { after, before, describe, it } from 'node:test';
import {
  FOOTPRINT,
  SYMBOL,
  assertAccessible,
  images,
  launchApp,
  record,
  repoRoot,
  screenshot,
  searchFor,
  startTeamServer,
  tempDir,
  textOf,
  waitForLiveUpdates,
  waitForText,
} from '../harness.mjs';

const MPN = 'CL10B104KB8NNNC';
const KNOWN_MPN = 'CL10C020CB8NNNC';
const IPN = 'CAP-0004-0001';
const BADGE = '.team-settings > summary';
const COLUMN = '.part-fields';
const NOTICE = '.team-notice';

// ---- A small ZIP writer (stored entries only), enough for a download ------

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

/** A ZIP file holding `entries` ([name, Buffer]) uncompressed. */
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

/** The repository's capacitor symbol and footprint, renamed, as a vendor ZIP. */
function download(folder, mpn) {
  const read = (file) => fs.readFileSync(path.join(repoRoot, file), 'utf8');
  const symbol = read('symbols/g-cap.kicad_sym')
    .replaceAll('(symbol "C_', '(symbol "C_New_')
    .replace('(symbol "C"', '(symbol "C_New"')
    .replace('"Value" ".Value"', '"Value" "100n"')
    .replace('"Datasheet" ".Datasheet"', '"Datasheet" "https://example.test/cl10b104.pdf"')
    .replace('"Description" "Unpolarized capacitor"', '"Description" "CAP CER 0.1UF 50V X7R 0603"')
    .replace(
      '\t\t(property "Reference"',
      `\t\t(property "Manufacturer_Name" "Samsung Electro-Mechanics" (at 0 0 0) (effects (hide yes)))\n` +
        `\t\t(property "Manufacturer_Part_Number" "${mpn}" (at 0 0 0) (effects (hide yes)))\n` +
        `\t\t(property "Mouser Part Number" "187-${mpn}" (at 0 0 0) (effects (hide yes)))\n` +
        '\t\t(property "Reference"',
    );
  const footprint = read('footprints/g-Capacitor_SMD.pretty/C_0603_1608Metric.kicad_mod')
    .replace('(footprint "C_0603_1608Metric"', '(footprint "C_0603_New"')
    .replace('${KICAD9_3DMODEL_DIR}/Capacitor_SMD.3dshapes/C_0603_1608Metric.step', '${KIPRJMOD}/3D/C_0603_New.stp');
  const zip = zipOf([
    ['CL10B104/KiCad/C_New.kicad_sym', Buffer.from(symbol, 'utf8')],
    ['CL10B104/KiCad/C_0603_New.kicad_mod', Buffer.from(footprint, 'utf8')],
    ['CL10B104/3D/C_0603_New.stp', Buffer.from('ISO-10303-21;\nEND-ISO-10303-21;\n')],
    ['CL10B104/Altium/C_New.SchLib', Buffer.from('not KiCad')],
  ]);
  const file = path.join(folder, `${mpn}.zip`);
  fs.writeFileSync(file, zip);
  return file;
}

describe('team catalog: add a part from a download', () => {
  let server;
  let browser;
  let editor;
  let files;

  before(async () => {
    server = await startTeamServer({ csv: ['g-res', 'g-cap'] });
    [editor] = server.accounts;
    files = tempDir('download');
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

  async function valueOf(selector) {
    return browser.execute((s) => document.querySelector(s)?.value ?? null, selector);
  }

  async function fieldValue(name) {
    return browser.execute(
      (label) => {
        const found = [...document.querySelectorAll('.add-form .edit-row label')].find(
          (l) => l.textContent.trim() === label,
        );
        return found ? document.getElementById(found.htmlFor)?.value ?? null : null;
      },
      name,
    );
  }

  async function waitForDrawings(selector) {
    await browser.waitUntil(
      async () => {
        const drawn = await images(browser, selector);
        return drawn.length > 0 && drawn.every((image) => image.loaded && image.svg);
      },
      { timeout: 60_000, interval: 100, timeoutMsg: `${selector} was never drawn` },
    );
  }

  it('signs in, and Add part is offered to someone who can edit', async () => {
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

  it('reads a ZIP file, guesses the category with its reason and offers the next free IPN', async () => {
    await (await browser.$('#add-files')).addValue(download(files, MPN));
    await waitForText(browser, '#add-guess', /^Best guess: Capacitors, because .*C_0603_New/);
    record({ event: 'guess', text: await textOf(browser, '#add-guess') });
    assert.equal(await valueOf('#add-category'), 'cap');
    assert.equal(await valueOf('#add-ipn'), IPN);
    await waitForText(browser, '#add-ipn-hint', 'The next free IPN in this category. An IPN is never reused.');
    // What the symbol says, in the vendor's spellings.
    assert.equal(await valueOf('#add-mpn'), MPN);
    assert.equal(await valueOf('#add-manufacturer'), 'Samsung Electro-Mechanics');
    assert.equal(await fieldValue('Description'), 'CAP CER 0.1UF 50V X7R 0603');
    assert.equal(await fieldValue('Value'), '100n');
    assert.match(await textOf(browser, '.add-unused'), /Mouser Part Number/);
    assert.equal(await valueOf('#add-symbol'), 'g-cap:C_New');
    assert.equal(await valueOf('#add-footprint'), 'g-Capacitor_SMD:C_0603_New');
    assert.equal(await valueOf('#add-from'), 'SamacSys (Component Search Engine)');
    // The checks: the Catalog has a footprint with the same pads; the pins
    // and pads agree; the license needs review.
    assert.match(await textOf(browser, '#add-known-footprint'), /g-Capacitor_SMD:C_0603_1608Metric \(the same pads\)/);
    assert.equal(await textOf(browser, '#add-pins'), "The symbol's 2 pins match the footprint's numbered pads.");
    assert.match(await textOf(browser, '#add-license'), /^License: Unknown, needs review/);
    assert.equal(await textOf(browser, '#add-download'), `From ${MPN}.zip: 3 KiCad files used, 1 other file left out.`);
    await waitForDrawings(`${SYMBOL} .file-preview img`);
    await waitForDrawings(`${FOOTPRINT} .file-preview img`);
    await browser.execute(() => {
      document.querySelector('.part-fields').scrollTop = 0;
    });
    await assertAccessible(browser, '61-team-add-part');
    await screenshot(browser, '61-team-add-part');
  });

  it('saves the part, which the search finds with its drawings and history', async () => {
    await (await browser.$('#add-reason')).setValue('New 100n capacitor for the sensor board');
    await clickButton('.add-form', 'Save');
    await waitForText(browser, NOTICE, `Added ${IPN} as revision 1.`, 30_000);
    await waitForText(browser, '#part-title', MPN);
    await waitForText(browser, `${SYMBOL} .provenance`, /^team catalog, symbol revision 1 · /, 60_000);
    await waitForText(browser, `${FOOTPRINT} .provenance`, /^team catalog, footprint revision 1 · /, 60_000);
    await waitForText(browser, '#part-category', 'Category: Capacitors');
    await searchFor(browser, MPN, '1 matching part');
    assert.match(await textOf(browser, 'ul[aria-label="Search results"]'), new RegExp(IPN));
    await clickButton(COLUMN, 'History (1)');
    await (await browser.$('.history li:nth-child(1) button')).click();
    const rows = await browser.execute(() =>
      [...document.querySelectorAll('.revision .changes tbody tr')].map((tr) =>
        [...tr.children].map((cell) => cell.textContent.trim()),
      ),
    );
    record({ event: 'added-history', rows });
    assert.deepEqual(rows[0], ['Downloaded from', `SamacSys (Component Search Engine): ${MPN}.zip`]);
    assert.ok(
      rows.some(([label, after]) => label === 'Footprint file' && after.startsWith('CL10B104/KiCad/C_0603_New.kicad_mod')),
      JSON.stringify(rows),
    );
  });

  it('shows a part that already has the MPN, and opens it', async () => {
    await (await browser.$('#add-part-open')).click();
    await waitForText(browser, '#add-heading', 'Add part');
    await (await browser.$('#add-files')).addValue(download(files, KNOWN_MPN));
    await waitForText(browser, '#add-same-mpn p', `The MPN ${KNOWN_MPN} is already in the Catalog:`);
    assert.match(await textOf(browser, '#add-same-mpn'), /CAP-0000-0020: CAP CER 2PF 50V C0G\/NP0 0603/);
    await clickButton('#add-same-mpn', 'Open CAP-0000-0020');
    await waitForText(browser, '#part-title', KNOWN_MPN);
    assert.equal(await browser.$('#add-heading').isExisting(), false);
  });
});
