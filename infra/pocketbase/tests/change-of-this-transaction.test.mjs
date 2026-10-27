// Unit tests for the team server's rule that every new Catalog record
// names the Change saved in its own transaction (catalog_lib.js,
// kicad-parts-2707). The rules need no PocketBase: fake apps stand in for a
// transaction and for the committed database. The integration tests in
// tooling/local-team/tests/pocketbase_change_guards.rs check the same
// rules through a real team server.
// Run: npm run js-tests --prefix apps/desktop/journeys

import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { describe, it } from 'node:test';

const require = createRequire(import.meta.url);
const lib = require('../pb_hooks/catalog_lib.js');

const CHANGES = 'changes-collection-id';

/** A fake Change record. */
function change(id, seq) {
  return { id, getInt: (name) => (name === 'seq' ? seq : 0) };
}

/** A fake app that sees `changes`, with the app store `store`. */
function fakeApp(changes, store = new Map()) {
  return {
    store: () => ({ get: (key) => store.get(key), set: (key, value) => store.set(key, value) }),
    findRecordsByFilter: (collection, filter, sort, limit, offset, params) => {
      assert.equal(collection, 'catalog_changes');
      const matching = filter === 'id = {:id}' ? changes.filter((record) => record.id === params.id) : changes;
      return [...matching].sort((a, b) => b.getInt('seq') - a.getInt('seq')).slice(0, limit);
    },
  };
}

/**
 * A transaction that sees `seen`, over a committed database that holds
 * `committed`. As in PocketBase, both share one app store.
 */
function transaction(committed, seen) {
  const store = new Map();
  lib.rememberCommittedApp(fakeApp(committed, store));
  return fakeApp(seen, store);
}

describe('the Change saved in this transaction', () => {
  const one = change('aaaaaaaaaaaaaa1', 1);
  const two = change('aaaaaaaaaaaaaa2', 2);

  it('is the newest Change, when the committed database cannot see it yet', () => {
    assert.deepEqual(lib.changesSeen(transaction([one], [one, two])), { newest: two, own: two });
  });

  it('is none when the newest Change is already committed, however new it is', () => {
    // The case "the highest seq" alone would miss: code that saved no
    // Change names the newest committed one.
    assert.deepEqual(lib.changesSeen(transaction([one, two], [one, two])), { newest: two, own: null });
  });

  it('is none when both apps read the same records, as outside a transaction', () => {
    const app = fakeApp([one, two]);
    lib.rememberCommittedApp(app);
    assert.deepEqual(lib.changesSeen(app), { newest: two, own: null });
  });

  it('is never guessed when the committed database cannot be read', () => {
    // During a restore restart, PocketBase drops the main app's database
    // handles. A read error must fail the write, not pass an old Change.
    const store = new Map();
    const broken = fakeApp([one], store);
    broken.findRecordsByFilter = () => {
      throw new Error('sql: database is closed');
    };
    lib.rememberCommittedApp(broken);
    assert.throws(() => lib.changesSeen(fakeApp([one, two], store)), /database is closed/);
  });

  it('is none before the first Change', () => {
    assert.deepEqual(lib.changesSeen(transaction([], [])), { newest: null, own: null });
  });
});

describe('the fields that name a Change', () => {
  /** A fake schema field. */
  function field(name, type, collectionId, required) {
    return { name, type: () => type, collectionId, required };
  }

  it('are the relation fields to catalog_changes, found in the schema', () => {
    const collection = {
      fields: [
        field('id', 'text', '', true),
        field('part', 'relation', 'parts-collection-id', true),
        field('change', 'relation', CHANGES, false),
        field('first_change', 'relation', CHANGES, true),
      ],
    };
    assert.deepEqual(lib.changeFieldsOf(collection, CHANGES), [
      { name: 'change', required: false },
      { name: 'first_change', required: true },
    ]);
  });
});

describe('a new record that names a Change', () => {
  const own = 'aaaaaaaaaaaaaa2';
  const named = (value, required = true) => [{ name: 'change', required, value }];

  it('passes when it names the Change saved in this transaction', () => {
    assert.equal(lib.changeFieldsProblem('catalog_part_details', 'bbbbbbbbbbbbbb1', named(own), own), '');
  });

  it('is refused when it names an older Change', () => {
    assert.equal(
      lib.changeFieldsProblem('catalog_part_details', 'bbbbbbbbbbbbbb1', named('aaaaaaaaaaaaaa1'), own),
      `the new catalog_part_details record bbbbbbbbbbbbbb1 names Change aaaaaaaaaaaaaa1 in its "change" field. It must name Change ${own}, which this transaction saved.`,
    );
  });

  it('is refused when the transaction saved no Change', () => {
    assert.match(
      lib.changeFieldsProblem('catalog_part_details', 'bbbbbbbbbbbbbb1', named('aaaaaaaaaaaaaa1'), ''),
      /this transaction saved no Change. Save a new Change first, in the same transaction.$/,
    );
  });

  it('may leave an optional field empty, as an import run that saved no Change does', () => {
    assert.equal(lib.changeFieldsProblem('catalog_audit_entries', 'bbbbbbbbbbbbbb1', named('', false), ''), '');
    assert.equal(lib.changeFieldsProblem('catalog_audit_entries', 'bbbbbbbbbbbbbb1', named('', false), own), '');
  });

  it('may never leave a required field empty', () => {
    assert.match(lib.changeFieldsProblem('catalog_parts', 'bbbbbbbbbbbbbb1', named('', true), own), /names no Change in its "change" field/);
  });
});

describe('a new Change', () => {
  it('is numbered right after the newest', () => {
    assert.equal(lib.newChangeProblem(1, null), '');
    assert.equal(lib.newChangeProblem(8, { id: 'a', seq: 7, own: false }), '');
  });

  it('is refused with a gap, which desktops would refuse too', () => {
    assert.match(lib.newChangeProblem(9, { id: 'a', seq: 7, own: false }), /must be numbered 8, right after the newest, not 9/);
    assert.match(lib.newChangeProblem(2, null), /must be numbered 1/);
  });

  it('is refused when its transaction already saved one', () => {
    assert.match(lib.newChangeProblem(8, { id: 'a', seq: 7, own: true }), /already saved Change a/);
  });
});
