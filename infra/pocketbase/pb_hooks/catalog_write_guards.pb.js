/// <reference path="../pb_data/types.d.ts" />

// Catalog records never change once written (seed kicad-parts-6046).
//
// The collection rules already stop members from writing, but superusers
// bypass rules. These hooks also stop superusers and any server code:
//
// - Nobody creates, updates or deletes a Catalog record through the record
//   API (`/api/collections/catalog_*/records`, the batch API or the
//   dashboard). The protected routes are the only way in:
//   POST /api/catalog/csv-imports and /api/catalog/edits.
// - Revisions, saved part details, CAD items, blobs, Changes, audit
//   entries, import problems and the command ledger refuse every update
//   and delete, whoever saves them.
// - A part refuses every delete, and every update except moving its current
//   revision, or its current details, forward to its own next one. Its IPN
//   never changes.
// - New saved part details and new CAD item revisions must be numbered
//   right after the newest one, so the current one is always the newest.
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
  "catalog_part_details",
  "catalog_part_revisions",
  "catalog_audit_entries",
  "catalog_import_problems",
  "catalog_commands",
);

onRecordUpdateRequest(
  (e) => {
    throw new ForbiddenError(
      "Catalog records never change; publish an edit through the Catalog routes instead",
    );
  },
  "catalog_changes",
  "catalog_blobs",
  "catalog_cad_items",
  "catalog_cad_item_revisions",
  "catalog_parts",
  "catalog_part_details",
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
  "catalog_part_details",
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
  "catalog_part_details",
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
  "catalog_part_details",
  "catalog_part_revisions",
  "catalog_audit_entries",
  "catalog_import_problems",
  "catalog_commands",
);

// The one update the Catalog makes: a part's current revision or current
// details move to the one just saved after it, in the same transaction.
onRecordUpdate((e) => {
  const part = e.record;
  // The stored row, not part.original(): a record saved again right after
  // it was created keeps an empty original.
  const before = e.app.findRecordById("catalog_parts", part.id);
  const ipn = before.getString("ipn");
  for (const name of ["ipn", "ipn_prefix", "variant", "first_change"]) {
    if (part.getString(name) !== before.getString(name)) {
      throw new Error(`a part's ${name} never changes (${ipn})`);
    }
  }
  // Each pointer stays, or moves to the record saved right after it.
  const pointers = [
    ["current_revision", "current_revision_number", "catalog_part_revisions", "revision"],
    ["current_details", "current_details_number", "catalog_part_details", "details"],
  ];
  for (const [field, numberField, collection, what] of pointers) {
    const target = part.getString(field);
    const number = part.getInt(numberField);
    const previous = before.getString(field);
    if (target === previous && number === before.getInt(numberField)) {
      continue;
    }
    let record = null;
    try {
      record = e.app.findRecordById(collection, target);
    } catch (_) {
      record = null;
    }
    if (!record || record.getString("part") !== part.id || record.getInt("number") !== number) {
      throw new Error(`the current ${what} of ${ipn} must be one of its own, numbered ${number}`);
    }
    // A new part has neither until its first ones are saved.
    const previousNumber = previous === "" ? 0 : before.getInt(numberField);
    if (number !== previousNumber + 1 || record.getString("predecessor") !== previous) {
      throw new Error(`the current ${what} of ${ipn} can only move to the one saved right after it`);
    }
  }
  return e.next();
}, "catalog_parts");

// Saved part details are numbered from 1 per part: new ones must be the
// newest number + 1, name the newest as their predecessor, and have the ID
// derived from their part and number.
onRecordCreate((e) => {
  const lib = require(`${__hooks}/catalog_lib.js`);
  const details = e.record;
  const part = details.getString("part");
  const number = details.getInt("number");
  const found = e.app.findRecordsByFilter("catalog_part_details", "part = {:id}", "-number", 1, 0, {
    id: part,
  });
  const newest = found.length > 0 ? found[0] : null;
  const expected = newest ? newest.getInt("number") + 1 : 1;
  if (number !== expected || details.getString("predecessor") !== (newest ? newest.id : "")) {
    throw new Error(`new details of part ${part} must be number ${expected}, right after its current details`);
  }
  if (details.id !== lib.deriveId("part-details", part + "\n" + number)) {
    throw new Error(`details ${number} of part ${part} must have the ID derived from its part and number`);
  }
  return e.next();
}, "catalog_part_details");

// A CAD item's current revision is its newest, so a new CAD item revision
// must be numbered right after the newest stored one, name it as its
// predecessor, and have the ID derived from its CAD item and number.
onRecordCreate((e) => {
  const lib = require(`${__hooks}/catalog_lib.js`);
  const revision = e.record;
  const cadItem = revision.getString("cad_item");
  const number = revision.getInt("number");
  const found = e.app.findRecordsByFilter("catalog_cad_item_revisions", "cad_item = {:id}", "-number", 1, 0, {
    id: cadItem,
  });
  const newest = found.length > 0 ? found[0] : null;
  const expected = newest ? newest.getInt("number") + 1 : 1;
  if (number !== expected || revision.getString("predecessor") !== (newest ? newest.id : "")) {
    throw new Error(`a new revision of CAD item ${cadItem} must be number ${expected}, right after its current revision`);
  }
  if (revision.id !== lib.deriveId("cad-item-revision", cadItem + "\n" + number)) {
    throw new Error(`revision ${number} of CAD item ${cadItem} must have the ID derived from its CAD item and number`);
  }
  return e.next();
}, "catalog_cad_item_revisions");

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
