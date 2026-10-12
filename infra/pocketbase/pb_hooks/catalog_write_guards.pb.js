/// <reference path="../pb_data/types.d.ts" />

// Catalog records never change once written (seed kicad-parts-6046).
//
// The collection rules already stop members from writing, but superusers
// bypass rules. These hooks also stop superusers and any server code:
//
// - Nobody creates, updates or deletes a Catalog record through the record
//   API (`/api/collections/catalog_*/records`, the batch API or the
//   dashboard). The protected routes are the only way in:
//   POST /api/catalog/csv-imports, /api/catalog/edits,
//   /api/catalog/archives and /api/catalog/csv-reviews.
// - Revisions, saved part details, archive entries, CSV reviews, CAD items, blobs,
//   categories and their revisions, remembered matches, Changes, audit
//   entries, import problems and the command ledger refuse every update and
//   delete, whoever saves them.
// - A remembered match is made only with an edit that changes that part's
//   category to the match's category, by the person who made the edit;
//   never by an import.
// - A part refuses every delete. Its current details may only move to its
//   next details. Its current revision may only move to its newest revision
//   that is not archived (a new revision, or the one an archive or
//   unarchive makes current). Its IPN never changes.
// - New part revisions, saved part details, CAD item revisions and category
//   revisions must be numbered right after the newest one, archived or not,
//   name it as their predecessor and have the ID derived from their owner
//   and number.
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
  "catalog_archive_entries",
  "catalog_csv_reviews",
  "catalog_categories",
  "catalog_category_revisions",
  "catalog_category_matches",
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
  "catalog_archive_entries",
  "catalog_csv_reviews",
  "catalog_categories",
  "catalog_category_revisions",
  "catalog_category_matches",
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
  "catalog_archive_entries",
  "catalog_csv_reviews",
  "catalog_categories",
  "catalog_category_revisions",
  "catalog_category_matches",
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
  "catalog_archive_entries",
  "catalog_csv_reviews",
  "catalog_categories",
  "catalog_category_revisions",
  "catalog_category_matches",
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
  "catalog_archive_entries",
  "catalog_csv_reviews",
  "catalog_categories",
  "catalog_category_revisions",
  "catalog_category_matches",
  "catalog_audit_entries",
  "catalog_import_problems",
  "catalog_commands",
);

// The one update the Catalog makes: a part's current details move to the
// details saved right after them, and its current revision moves to its
// newest revision that is not archived, in the same transaction as the
// records that make it so.
onRecordUpdate((e) => {
  const lib = require(`${__hooks}/catalog_lib.js`);
  const part = e.record;
  // The stored row, not part.original(): a record saved again right after
  // it was created keeps an empty original. e.app is the transaction, so it
  // sees the records saved earlier in it (mx-3013a8).
  const before = e.app.findRecordById("catalog_parts", part.id);
  const ipn = before.getString("ipn");
  for (const name of ["ipn", "ipn_prefix", "variant", "first_change"]) {
    if (part.getString(name) !== before.getString(name)) {
      throw new Error(`a part's ${name} never changes (${ipn})`);
    }
  }
  const find = (collection, id) => {
    try {
      return e.app.findRecordById(collection, id);
    } catch (_) {
      return null;
    }
  };

  // Details: stay, or move to the details saved right after them.
  const details = part.getString("current_details");
  const detailsNumber = part.getInt("current_details_number");
  const previousDetails = before.getString("current_details");
  if (details !== previousDetails || detailsNumber !== before.getInt("current_details_number")) {
    const record = find("catalog_part_details", details);
    if (!record || record.getString("part") !== part.id || record.getInt("number") !== detailsNumber) {
      throw new Error(`the current details of ${ipn} must be one of its own, numbered ${detailsNumber}`);
    }
    // A new part has none until its first ones are saved.
    const previousNumber = previousDetails === "" ? 0 : before.getInt("current_details_number");
    if (detailsNumber !== previousNumber + 1 || record.getString("predecessor") !== previousDetails) {
      throw new Error(`the current details of ${ipn} can only move to the one saved right after it`);
    }
  }

  // Revision: stay, or move to the newest revision that is not archived.
  const revision = part.getString("current_revision");
  const revisionNumber = part.getInt("current_revision_number");
  if (revision !== before.getString("current_revision") || revisionNumber !== before.getInt("current_revision_number")) {
    const record = find("catalog_part_revisions", revision);
    if (!record || record.getString("part") !== part.id || record.getInt("number") !== revisionNumber) {
      throw new Error(`the current revision of ${ipn} must be one of its own, numbered ${revisionNumber}`);
    }
    const newest = lib.newestNotArchived(e.app, part.id, lib.archiveState(e.app, part.id));
    if (!newest || newest.id !== revision) {
      throw new Error(
        `the current revision of ${ipn} can only move to its newest revision that is not archived`,
      );
    }
  }
  return e.next();
}, "catalog_parts");

// A new part revision must be numbered right after the part's newest
// revision (archived or not), name it as its predecessor, and have the ID
// derived from its part and number.
onRecordCreate((e) => {
  const lib = require(`${__hooks}/catalog_lib.js`);
  const revision = e.record;
  const part = revision.getString("part");
  const number = revision.getInt("number");
  const newest = lib.newestRevision(e.app, part);
  const expected = newest ? newest.getInt("number") + 1 : 1;
  if (number !== expected || revision.getString("predecessor") !== (newest ? newest.id : "")) {
    throw new Error(`a new revision of part ${part} must be number ${expected}, right after its newest revision`);
  }
  if (revision.id !== lib.deriveId("part-revision", part + "\n" + number)) {
    throw new Error(`revision ${number} of part ${part} must have the ID derived from its part and number`);
  }
  return e.next();
}, "catalog_part_revisions");

// An archive entry must be exactly what the archive route saves: for one of
// the part's own revisions, in a Change of the matching kind whose seq it
// copies, under the ID derived from that Change's correlation ID; it must
// really change the revision's state, leave at least one revision not
// archived, and name the newest such revision as current afterwards.
// Saving it moves the part's current revision there.
onRecordCreate((e) => {
  const lib = require(`${__hooks}/catalog_lib.js`);
  const entry = e.record;
  const part = entry.getString("part");
  const number = entry.getInt("revision_number");
  const archived = entry.getBool("archived");
  const find = (collection, id) => {
    try {
      return e.app.findRecordById(collection, id);
    } catch (_) {
      return null;
    }
  };
  const revision = find("catalog_part_revisions", entry.getString("part_revision"));
  if (!revision || revision.getString("part") !== part || revision.getInt("number") !== number) {
    throw new Error(`an archive entry must name revision ${number} of its own part ${part}`);
  }
  const change = find("catalog_changes", entry.getString("change"));
  const kind = archived ? "archive" : "unarchive";
  if (!change || change.getString("kind") !== kind || change.getInt("seq") !== entry.getInt("seq")) {
    throw new Error(`an archive entry must belong to a Change of kind ${kind} and copy its seq`);
  }
  if (entry.id !== lib.deriveId("archive-entry", change.getString("correlation_id"))) {
    throw new Error("an archive entry must have the ID derived from its Change's correlation ID");
  }
  const state = lib.archiveState(e.app, part);
  if ((state[number] === true) === archived) {
    throw new Error(`an archive entry must change the state of revision ${number}`);
  }
  state[number] = archived;
  const current = lib.newestNotArchived(e.app, part, state);
  if (!current) {
    throw new Error(`an archive entry cannot archive the only revision of part ${part} that is not archived`);
  }
  if (
    entry.getString("current_revision_after") !== current.id ||
    entry.getInt("current_revision_number_after") !== current.getInt("number")
  ) {
    throw new Error(
      `an archive entry must name revision ${current.getInt("number")}, the newest not archived, as current afterwards`,
    );
  }
  const saved = e.next();
  // Saving the entry moves the part to that revision, in the same
  // transaction, so no code can leave the part on an archived or older
  // revision. The part-update guard checks the move.
  const record = e.app.findRecordById("catalog_parts", part);
  if (record.getString("current_revision") !== current.id) {
    record.set("current_revision", current.id);
    record.set("current_revision_number", current.getInt("number"));
    e.app.save(record);
  }
  return saved;
}, "catalog_archive_entries");

// A CSV review must be exactly what the review route saves: for one of the
// part's own csv-details-differ-from-edited-details,
// csv-cad-differs-from-edited-revision or csv-cad-differs-from-edited-cad-item
// import problems, copying its code and
// the SHA-256 of the CSV document it recorded, in a Change of kind
// csv-review whose seq it copies, under the ID derived from that Change's
// correlation ID, with a list of field names.
onRecordCreate((e) => {
  const lib = require(`${__hooks}/catalog_lib.js`);
  const review = e.record;
  const find = (collection, id) => {
    try {
      return e.app.findRecordById(collection, id);
    } catch (_) {
      return null;
    }
  };
  const part = find("catalog_parts", review.getString("part"));
  const problem = find("catalog_import_problems", review.getString("problem"));
  const kind = problem ? lib.CSV_REVIEW_KINDS[problem.getString("code")] : undefined;
  if (!part || !problem || !kind || problem.getString("subject") !== part.getString("ipn")) {
    throw new Error("a CSV review must name a CSV difference of its own part");
  }
  if (review.getString("code") !== problem.getString("code")) {
    throw new Error("a CSV review must copy its problem's code");
  }
  let recorded = null;
  try {
    const detail = JSON.parse(problem.getString("detail"));
    recorded = lib.recordedCsvDocument(problem.getString("code"), detail);
  } catch (_) {
    recorded = null;
  }
  if (typeof recorded !== "string" || review.getString("csv_sha256") !== $security.sha256(recorded)) {
    throw new Error("a CSV review must name the SHA-256 of the CSV document its problem recorded");
  }
  const change = find("catalog_changes", review.getString("change"));
  if (!change || change.getString("kind") !== "csv-review" || change.getInt("seq") !== review.getInt("seq")) {
    throw new Error("a CSV review must belong to a Change of kind csv-review and copy its seq");
  }
  if (review.id !== lib.deriveId("csv-review", change.getString("correlation_id"))) {
    throw new Error("a CSV review must have the ID derived from its Change's correlation ID");
  }
  let accepted = null;
  try {
    accepted = JSON.parse(review.getString("accepted"));
  } catch (_) {
    accepted = null;
  }
  if (!Array.isArray(accepted) || !accepted.every((name) => typeof name === "string" && name.length > 0)) {
    throw new Error("a CSV review must list the field names it took, as JSON");
  }
  return e.next();
}, "catalog_csv_reviews");

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

// A category's current revision is its newest, so a new category revision
// must be numbered right after the newest stored one, name it as its
// predecessor, and have the ID derived from its category and number. A
// category's ID is derived from its code.
onRecordCreate((e) => {
  const lib = require(`${__hooks}/catalog_lib.js`);
  const revision = e.record;
  const category = revision.getString("category");
  const number = revision.getInt("number");
  const found = e.app.findRecordsByFilter("catalog_category_revisions", "category = {:id}", "-number", 1, 0, {
    id: category,
  });
  const newest = found.length > 0 ? found[0] : null;
  const expected = newest ? newest.getInt("number") + 1 : 1;
  if (number !== expected || revision.getString("predecessor") !== (newest ? newest.id : "")) {
    throw new Error(`a new revision of category ${category} must be number ${expected}, right after its current revision`);
  }
  if (revision.id !== lib.deriveId("category-revision", category + "\n" + number)) {
    throw new Error(`revision ${number} of category ${category} must have the ID derived from its category and number`);
  }
  let parsed = null;
  try {
    parsed = JSON.parse(revision.getString("snapshot"));
  } catch (_) {
    parsed = null;
  }
  const document = lib.categoryDocument(parsed);
  if (typeof document === "string" || lib.deriveId("category", document.code) !== category) {
    throw new Error(`revision ${number} of category ${category} must be a category with that category's code`);
  }
  return e.next();
}, "catalog_category_revisions");

onRecordCreate((e) => {
  const lib = require(`${__hooks}/catalog_lib.js`);
  if (e.record.id !== lib.deriveId("category", e.record.getString("code"))) {
    throw new Error("a category must have the ID derived from its code");
  }
  return e.next();
}, "catalog_categories");

// A remembered match is a person's confirmation: it must belong to a Change
// of kind edit, copy its seq, actor and actor name, have the ID derived from
// the Change and its clue, and that Change must save the part's details in
// the match's category, changed from another category. So an import, or a
// supplier category on its own, never makes one.
onRecordCreate((e) => {
  const lib = require(`${__hooks}/catalog_lib.js`);
  const match = e.record;
  const find = (collection, id) => {
    try {
      return e.app.findRecordById(collection, id);
    } catch (_) {
      return null;
    }
  };
  const change = find("catalog_changes", match.getString("change"));
  if (
    !change ||
    change.getString("kind") !== lib.CHANGE_KINDS.edit ||
    change.getInt("seq") !== match.getInt("seq") ||
    change.getString("actor") !== match.getString("actor") ||
    change.getString("actor_name") !== match.getString("actor_name")
  ) {
    throw new Error("a remembered match must belong to an edit and copy its seq and who made it");
  }
  if (match.id !== lib.deriveId("category-match", change.id + "\n" + match.getString("clue"))) {
    throw new Error("a remembered match must have the ID derived from its Change and clue");
  }
  const saved = e.app.findRecordsByFilter(
    "catalog_part_details",
    "part = {:part} && change = {:change}",
    "",
    1,
    0,
    { part: match.getString("part"), change: change.id },
  );
  const details = saved.length > 0 ? saved[0] : null;
  const previous = details ? find("catalog_part_details", details.getString("predecessor")) : null;
  const categoryOf = (record) => {
    try {
      return JSON.parse(record.getString("snapshot")).category;
    } catch (_) {
      return null;
    }
  };
  const category = details ? categoryOf(details) : null;
  if (
    typeof category !== "string" ||
    lib.deriveId("category", category) !== match.getString("category") ||
    !previous ||
    categoryOf(previous) === category
  ) {
    throw new Error("a remembered match must come with an edit that moves its part to the match's category");
  }
  return e.next();
}, "catalog_category_matches");

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
