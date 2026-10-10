/// <reference path="../pb_data/types.d.ts" />

// Catalog records never change once written (seed kicad-parts-6046).
//
// The collection rules already stop members from writing, but superusers
// bypass rules. These hooks also stop superusers and any server code:
//
// - Nobody creates, updates or deletes a Catalog record through the record
//   API (`/api/collections/catalog_*/records`, the batch API or the
//   dashboard). The protected routes are the only way in:
//   POST /api/catalog/csv-imports, /api/catalog/edits and
//   /api/catalog/restores.
// - Revisions, CAD items, blobs, Changes, audit entries, import problems
//   and the command ledger refuse every update and delete, whoever saves
//   them.
// - A part refuses every delete, and every update except moving its current
//   revision forward to its own next revision. Its IPN never changes.
//
// Raw SQL bypasses record hooks; only migrations and test fixtures that
// tamper with a stopped instance's data.db use it.
//
// Handlers cannot see variables outside themselves, so each one repeats
// what it needs.

onRecordCreateRequest(
  (e) => {
    throw new ForbiddenError(
      "Catalog records are created only by the protected Catalog routes, such as POST /api/catalog/edits",
    );
  },
  "catalog_changes",
  "catalog_blobs",
  "catalog_cad_items",
  "catalog_cad_item_revisions",
  "catalog_parts",
  "catalog_part_revisions",
  "catalog_audit_entries",
  "catalog_import_problems",
  "catalog_commands",
);

onRecordUpdateRequest(
  (e) => {
    throw new ForbiddenError(
      "Catalog records never change; publish an edit or a restore through the Catalog routes instead",
    );
  },
  "catalog_changes",
  "catalog_blobs",
  "catalog_cad_items",
  "catalog_cad_item_revisions",
  "catalog_parts",
  "catalog_part_revisions",
  "catalog_audit_entries",
  "catalog_import_problems",
  "catalog_commands",
);

onRecordDeleteRequest(
  (e) => {
    throw new ForbiddenError("Catalog records are never deleted");
  },
  "catalog_changes",
  "catalog_blobs",
  "catalog_cad_items",
  "catalog_cad_item_revisions",
  "catalog_parts",
  "catalog_part_revisions",
  "catalog_audit_entries",
  "catalog_import_problems",
  "catalog_commands",
);

onRecordDelete(
  (e) => {
    throw new Error(`${e.record.collection().name} records are never deleted`);
  },
  "catalog_changes",
  "catalog_blobs",
  "catalog_cad_items",
  "catalog_cad_item_revisions",
  "catalog_parts",
  "catalog_part_revisions",
  "catalog_audit_entries",
  "catalog_import_problems",
  "catalog_commands",
);

onRecordUpdate(
  (e) => {
    throw new Error(`${e.record.collection().name} records never change once written`);
  },
  "catalog_changes",
  "catalog_blobs",
  "catalog_cad_items",
  "catalog_cad_item_revisions",
  "catalog_part_revisions",
  "catalog_audit_entries",
  "catalog_import_problems",
  "catalog_commands",
);

// The one update the Catalog makes: a part's current revision moves to the
// revision just saved after it, in the same transaction.
onRecordUpdate((e) => {
  const part = e.record;
  // The stored row, not part.original(): a record saved again right after
  // it was created keeps an empty original.
  const before = e.app.findRecordById("catalog_parts", part.id);
  for (const name of ["ipn", "ipn_prefix", "variant", "first_change"]) {
    if (part.getString(name) !== before.getString(name)) {
      throw new Error(`a part's ${name} never changes (${before.getString("ipn")})`);
    }
  }
  const target = part.getString("current_revision");
  const number = part.getInt("current_revision_number");
  const previous = before.getString("current_revision");
  if (target === previous && number === before.getInt("current_revision_number")) {
    return e.next();
  }
  let revision = null;
  try {
    revision = e.app.findRecordById("catalog_part_revisions", target);
  } catch (_) {
    revision = null;
  }
  if (!revision || revision.getString("part") !== part.id || revision.getInt("number") !== number) {
    throw new Error(`the current revision of ${before.getString("ipn")} must be one of its own revisions, numbered ${number}`);
  }
  // A new part has no current revision until its first one is saved.
  const previousNumber = previous === "" ? 0 : before.getInt("current_revision_number");
  if (number !== previousNumber + 1 || revision.getString("predecessor") !== previous) {
    throw new Error(
      `the current revision of ${before.getString("ipn")} can only move to the revision saved right after it`,
    );
  }
  return e.next();
}, "catalog_parts");

// ---- Collections -----------------------------------------------------------
//
// Superusers can also change collections through the API, which could drop
// a field (wiping it on every record), loosen a rule, drop a unique index or
// delete a whole collection. These request hooks refuse that for the
// catalog_* collections and for the parts of `users` the edit routes rely
// on (see catalog_lib.js collectionChangeProblem). Migrations do not go
// through the API, so schema changes, including the down migration, still
// work.

onCollectionUpdateRequest((e) => {
  const lib = require(`${__hooks}/catalog_lib.js`);
  let before = null;
  try {
    before = e.app.findCollectionByNameOrId(e.collection.id);
  } catch (_) {
    before = null;
  }
  if (before && lib.guardedCollection(before.name)) {
    const problem = lib.collectionChangeProblem(before, e.collection);
    if (problem) {
      throw new ForbiddenError(`${problem}; change the Catalog schema with a migration`);
    }
  }
  return e.next();
});

onCollectionDeleteRequest((e) => {
  const lib = require(`${__hooks}/catalog_lib.js`);
  if (lib.guardedCollection(e.collection.name)) {
    throw new ForbiddenError(`${e.collection.name} cannot be deleted; the Catalog depends on it`);
  }
  return e.next();
});

onCollectionsImportRequest((e) => {
  const lib = require(`${__hooks}/catalog_lib.js`);
  const imported = e.collectionsData || [];
  for (const existing of e.app.findAllCollections()) {
    if (!lib.guardedCollection(existing.name)) {
      continue;
    }
    const data = imported.find((item) => item && item.id === existing.id);
    if (!data) {
      if (e.deleteMissing) {
        throw new ForbiddenError(`the import would delete ${existing.name}; the Catalog depends on it`);
      }
      if (imported.some((item) => item && item.name === existing.name)) {
        throw new ForbiddenError(`the import would replace ${existing.name}; the Catalog depends on it`);
      }
      continue;
    }
    const problem = lib.collectionChangeProblem(existing, data);
    if (problem) {
      throw new ForbiddenError(`${problem}; change the Catalog schema with a migration`);
    }
  }
  return e.next();
});

// A member's name is shown as the author of their Changes, so it must not
// hide characters (see catalog_lib.js hasInvisible), and it must not look
// like the words the desktop uses for authors who are not members.
onRecordCreateRequest((e) => {
  const lib = require(`${__hooks}/catalog_lib.js`);
  if (lib.hasInvisible(e.record.getString("name"))) {
    throw new BadRequestError("a team member's name cannot contain invisible characters");
  }
  if (lib.isReservedName(e.record.getString("name"))) {
    throw new BadRequestError(`a team member cannot be called "${e.record.getString("name")}"`);
  }
  return e.next();
}, "users");

onRecordUpdateRequest((e) => {
  const lib = require(`${__hooks}/catalog_lib.js`);
  if (lib.hasInvisible(e.record.getString("name"))) {
    throw new BadRequestError("a team member's name cannot contain invisible characters");
  }
  if (lib.isReservedName(e.record.getString("name"))) {
    throw new BadRequestError(`a team member cannot be called "${e.record.getString("name")}"`);
  }
  return e.next();
}, "users");
