// Unit tests for the team server's rule that tells a CSV row that may be
// another part from new values for a part added in the app (catalog_lib.js
// csvRowIsAnotherPart, kicad-parts-3b72). The rule needs no PocketBase, so
// Node loads catalog_lib.js as it is. The import tests in
// tooling/catalog-import/tests/pocketbase_ipn_given_twice.rs check the same
// cases through a real team server.
// Run: npm run js-tests --prefix apps/desktop/journeys

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

/** Whether a row with `rowMpns` may be another part than one with `partMpns`. */
function flagged(kind, partMpns, rowMpns, sameDetailsAndCad = false) {
  return lib.csvRowIsAnotherPart(
    { firstChangeKind: kind, manufacturerParts: partMpns },
    { manufacturerParts: rowMpns, sameDetailsAndCad },
  );
}

describe('a CSV row that may be another part', () => {
  it('is a row that shares no MPN with a part added in the app', () => {
    assert.equal(flagged(ADDED_IN_THE_APP, made('AR0603-1K'), made('ERJ-3EKF1001V')), true);
  });

  it('is never a row for a part an import made, whose IPN came from the CSV catalog', () => {
    assert.equal(flagged(IMPORTED, made('AR0603-1K'), made('ERJ-3EKF1001V')), false);
    assert.equal(flagged(IMPORTED, [], []), false);
  });

  it('is the same part when one MPN is shared, even if the others differ', () => {
    assert.equal(flagged(ADDED_IN_THE_APP, made('AR0603-1K', 'RC0603FR-071KL'), made('RC0603FR-071KL')), false);
    assert.equal(flagged(ADDED_IN_THE_APP, made('AR0603-1K'), made('ERJ-3EKF1001V', 'AR0603-1K')), false);
  });

  it('is the same part when its details and CAD are exactly the part, whatever its MPN', () => {
    // A test point with no MPN, copied exactly into g-art.csv.
    assert.equal(flagged(ADDED_IN_THE_APP, [], [], true), false);
    assert.equal(flagged(ADDED_IN_THE_APP, made('AR0603-1K'), made('ERJ-3EKF1001V'), true), false);
  });

  it('compares MPNs without regard to case or the spaces around them', () => {
    assert.equal(flagged(ADDED_IN_THE_APP, made('AR0603-1K'), made('  ar0603-1k ')), false);
    // Spaces inside an MPN are part of it.
    assert.equal(flagged(ADDED_IN_THE_APP, made('AR0603-1K'), made('AR0603 -1K')), true);
  });

  it('is flagged when either side has no MPN, because nothing shows they are one part', () => {
    assert.equal(flagged(ADDED_IN_THE_APP, [], made('ERJ-3EKF1001V')), true);
    assert.equal(flagged(ADDED_IN_THE_APP, made('AR0603-1K'), []), true);
    assert.equal(flagged(ADDED_IN_THE_APP, [], []), true);
    // A blank MPN is no MPN.
    assert.equal(flagged(ADDED_IN_THE_APP, made(' '), made(' ')), true);
  });

  it('takes a placeholder MPN for no MPN, so two placeholders never match', () => {
    for (const placeholder of ['-', 'N/A', 'n/a', 'TBD', 'tbc', 'None', '?', '—']) {
      assert.equal(flagged(ADDED_IN_THE_APP, made(placeholder), made(placeholder)), true, placeholder);
    }
    // A real MPN next to a placeholder still matches.
    assert.equal(flagged(ADDED_IN_THE_APP, made('TBD', 'AR0603-1K'), made('AR0603-1K')), false);
  });
});

describe('the message of a CSV row that may be another part', () => {
  const row = (...mpns) => ({ path: 'database/g-res.csv', manufacturerParts: made(...mpns) });
  const part = (...mpns) => ({ description: 'RES 1K 1% 1/10W 0603', manufacturerParts: made(...mpns) });

  it('names the row by its IPN and MPN, never says they differ, and gives both ways out', () => {
    assert.equal(
      lib.anotherPartMessage(row('ERJ-3EKF1001V'), 'RES-0001-0001', part('AR0603-1K')),
      'In database/g-res.csv, the row with IPN RES-0001-0001 has MPN ERJ-3EKF1001V. ' +
        'The team catalog already gave RES-0001-0001 to a part added in the app: RES 1K 1% 1/10W 0603. ' +
        "That part's MPN is AR0603-1K, so nothing shows they are one part. " +
        'The import left that part as it is and kept the row here. ' +
        'If the row is a different part, give it another IPN. ' +
        "If it is that part, put that part's MPN (AR0603-1K) in the row, or remove the row.",
    );
  });

  it('says what to do when one side has no MPN', () => {
    assert.match(lib.anotherPartMessage(row(), 'RES-0001-0001', part('AR0603-1K')), /the row with IPN RES-0001-0001 has no MPN\./);
    const noMpn = lib.anotherPartMessage(row('ERJ-3EKF1001V'), 'RES-0001-0001', part());
    assert.match(noMpn, /That part has no MPN, and the row differs from it, so nothing shows they are one part\./);
    assert.match(noMpn, /If it is that part, make the row match it exactly, or remove the row\.$/);
  });

  it('lists several MPNs, and leaves out a description the part lacks', () => {
    const message = lib.anotherPartMessage(row('A1', 'B2'), 'RES-0001-0001', {
      description: '',
      manufacturerParts: made('C3', 'D4'),
    });
    assert.match(message, /has MPNs A1, B2\. .* added in the app\. That part's MPNs are C3, D4,/);
  });

  it('keeps every sentence to 30 words or fewer', () => {
    const message = lib.anotherPartMessage(row('ERJ-3EKF1001V'), 'RES-0001-0001', part('AR0603-1K'));
    for (const sentence of message.split(/(?<=\.) (?=[A-Z])/)) {
      assert.ok(sentence.split(/\s+/).length <= 30, sentence);
    }
  });
});
