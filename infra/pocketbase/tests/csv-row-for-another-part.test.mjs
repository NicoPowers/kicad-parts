// Unit tests for the team server's rule that tells a CSV row for another
// part from new values for a part added in the app (catalog_lib.js
// csvRowIsAnotherPart, kicad-parts-3b72). The rule needs no PocketBase, so
// Node loads catalog_lib.js as it is. The import tests in
// tooling/catalog-import/tests/pocketbase_ipn_given_twice.rs check the same
// cases through a real team server.
// Run: npm run camera-tests --prefix apps/desktop/journeys

import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { describe, it } from 'node:test';

const require = createRequire(import.meta.url);
const lib = require('../pb_hooks/catalog_lib.js');

const ADDED_IN_THE_APP = lib.CHANGE_KINDS.create;
const IMPORTED = lib.CHANGE_KINDS.csvImport;

/** Manufacturer parts with these MPNs, all from one maker. */
function made(...mpns) {
  return mpns.map((mpn) => ({ manufacturer: 'Yageo', mpn }));
}

describe('a CSV row for another part', () => {
  it('is a row that shares no MPN with a part added in the app', () => {
    assert.equal(lib.csvRowIsAnotherPart(ADDED_IN_THE_APP, made('AR0603-1K'), made('ERJ-3EKF1001V')), true);
  });

  it('is never a row for a part an import made, whose IPN came from the CSV catalog', () => {
    assert.equal(lib.csvRowIsAnotherPart(IMPORTED, made('AR0603-1K'), made('ERJ-3EKF1001V')), false);
    assert.equal(lib.csvRowIsAnotherPart(IMPORTED, [], []), false);
  });

  it('is the same part when one MPN is shared, even if the others differ', () => {
    assert.equal(
      lib.csvRowIsAnotherPart(ADDED_IN_THE_APP, made('AR0603-1K', 'RC0603FR-071KL'), made('RC0603FR-071KL')),
      false,
    );
    assert.equal(
      lib.csvRowIsAnotherPart(ADDED_IN_THE_APP, made('AR0603-1K'), made('ERJ-3EKF1001V', 'AR0603-1K')),
      false,
    );
  });

  it('compares MPNs without regard to case or the spaces around them', () => {
    assert.equal(lib.csvRowIsAnotherPart(ADDED_IN_THE_APP, made('AR0603-1K'), made('  ar0603-1k ')), false);
    // Spaces inside an MPN are part of it.
    assert.equal(lib.csvRowIsAnotherPart(ADDED_IN_THE_APP, made('AR0603-1K'), made('AR0603 -1K')), true);
  });

  it('is another part when either side has no MPN, because nothing shows they are one part', () => {
    assert.equal(lib.csvRowIsAnotherPart(ADDED_IN_THE_APP, [], made('ERJ-3EKF1001V')), true);
    assert.equal(lib.csvRowIsAnotherPart(ADDED_IN_THE_APP, made('AR0603-1K'), []), true);
    assert.equal(lib.csvRowIsAnotherPart(ADDED_IN_THE_APP, [], []), true);
    // A blank MPN is no MPN.
    assert.equal(lib.csvRowIsAnotherPart(ADDED_IN_THE_APP, made(' '), made(' ')), true);
  });
});

describe('the message of a CSV row for another part', () => {
  const row = (...mpns) => ({ rowNumber: 31, path: 'database/g-res.csv', manufacturerParts: made(...mpns) });
  const part = (...mpns) => ({ description: 'RES 1K 1% 1/10W 0603', manufacturerParts: made(...mpns) });

  it('names the row, the part with the IPN, the MPNs, and the one thing to do', () => {
    assert.equal(
      lib.anotherPartMessage(row('ERJ-3EKF1001V'), 'RES-0001-0001', part('AR0603-1K')),
      'Row 31 of database/g-res.csv has the IPN RES-0001-0001, but the team catalog already gave it to a part added in the app: RES 1K 1% 1/10W 0603. ' +
        "The row's MPN, ERJ-3EKF1001V, is not that part's (AR0603-1K), so they are different parts. " +
        'The import left that part as it is and kept the row here. Give this CSV row another IPN.',
    );
  });

  it('says why when one side has no MPN', () => {
    assert.match(lib.anotherPartMessage(row(), 'RES-0001-0001', part('AR0603-1K')), /The row has no MPN, so nothing shows it is that part\./);
    assert.match(
      lib.anotherPartMessage(row('ERJ-3EKF1001V'), 'RES-0001-0001', part()),
      /That part has no MPN, so nothing shows the row is that part\./,
    );
    assert.match(lib.anotherPartMessage(row(), 'RES-0001-0001', part()), /Neither has an MPN/);
  });

  it('lists several MPNs, and leaves out a description the part lacks', () => {
    const message = lib.anotherPartMessage(row('A1', 'B2'), 'RES-0001-0001', {
      description: '',
      manufacturerParts: made('C3', 'D4'),
    });
    assert.match(message, /added in the app\. The row's MPNs, A1, B2, are not that part's \(C3, D4\)/);
  });

  it('keeps every sentence to 30 words or fewer', () => {
    const message = lib.anotherPartMessage(row('ERJ-3EKF1001V'), 'RES-0001-0001', part('AR0603-1K'));
    for (const sentence of message.split(/(?<=\.) /)) {
      assert.ok(sentence.split(/\s+/).length <= 30, sentence);
    }
  });
});
