/// <reference path="../pb_data/types.d.ts" />

// Catalog records never change once written (seed kicad-parts-6046).
//
// The collection rules already stop members from writing, but superusers
// bypass rules. These hooks also stop superusers and any server code:
//
// - Nobody creates, updates or deletes a Catalog record through the record
//   API (`/api/collections/catalog_*/records`, the batch API or the
//   dashboard). The protected routes are the only way in:
//   POST /api/catalog/csv-imports, /api/catalog/edits, /api/catalog/parts,
//   /api/catalog/archives and /api/catalog/csv-reviews.
// - Revisions, saved part details, archive entries, CSV reviews, CAD items, blobs,
//   categories and their revisions, remembered matches, Changes, audit
//   entries, import problems and the command ledger refuse every update and
//   delete, whoever saves them.
// - A remembered match is made only with an edit that changes that part's
//   category to the match's category, or with a new part in that category,
//   by the person who made it; never by an import.
// - A part refuses every delete. Its current details may only move to its
//   next details. Its current revision may only move to its newest revision
//   that is not archived (a new revision, or the one an archive or
//   unarchive makes current). Its IPN never changes.
// - New part revisions, saved part details, CAD item revisions and category
//   revisions must be numbered right after the newest one, archived or not,
//   name it as their predecessor and have the ID derived from their owner
//   and number.
// - A transaction saves at most one Change, numbered right after the
//   newest. Every new record that names a Change, in any collection, names
//   the one saved in the same transaction. A part's current details or
//   revision move only to what that Change saved. So every move reaches
//   every desktop (seed kicad-parts-2707; catalog_lib.js explains how).
//   Breaking this is a programming error: the save answers 500, and the
//   server logs it as an error.
//
// Raw SQL bypasses record hooks; only migrations and test fixtures that
// tamper with a stopped instance's data.db use it.
//
// Handlers cannot see variables outside themselves, so the shared rules
// live in catalog_lib.js, which each handler loads with require.

// The guards read committed records through the app PocketBase started
// with; inside a hook, e.app is the transaction.
onBootstrap((e) => {
  const lib = require(`${__hooks}/catalog_lib.js`);
  lib.rememberCommittedApp(e.app);
  return e.next();
});

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
// newest revision that is not archived. Either move must be made by the
// Change saved in the same transaction (catalog_lib.js
// requireMoveByOwnChange).
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
  const find = (collection, id) => lib.recordById(e.app, collection, id);
  // The details and revision the part moves to, if it moves.
  const moved = { details: null, revision: null };

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
    moved.details = record;
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
    moved.revision = record;
  }
  if (moved.details || moved.revision) {
    lib.requireMoveByOwnChange(e.app, part, moved);
  }
  return e.next();
}, "catalog_parts");

// A new part starts with no current details or revision. Its first ones
// are saved after it, in the same Change, and only the part-update guard
// above may move it to them. Otherwise a part could start out pointing at
// another part's records, which no desktop following Changes would see.
onRecordCreate((e) => {
  const lib = require(`${__hooks}/catalog_lib.js`);
  const part = e.record;
  if (part.getString("current_details") !== "" || part.getString("current_revision") !== "") {
    lib.refuseProgrammingError(
      e.app,
      `the new part ${part.getString("ipn")} must start with no current details or revision. Save the part first, then its first details and revision.`,
    );
  }
  return e.next();
}, "catalog_parts");

// New part revisions (archived or not), part details, CAD item revisions
// and category revisions must come right after their owner's newest one
// (catalog_lib.js numberedRecordProblem).
onRecordCreate(
  (e) => {
    const lib = require(`${__hooks}/catalog_lib.js`);
    const problem = lib.numberedRecordProblem(e.app, e.record);
    if (problem) {
      throw new Error(problem);
    }
    return e.next();
  },
  "catalog_part_revisions",
  "catalog_part_details",
  "catalog_cad_item_revisions",
  "catalog_category_revisions",
);

// Saving new part details or a new part revision moves the part there, in
// the same transaction, as saving an archive entry does. Desktops that
// follow Changes make whatever a Change saved current
// (crates/catalog/src/replica.rs, CatalogReplica::apply), so the server
// must too. The route's own move then changes nothing. A new revision has
// no archive entry, so it is always the newest that is not archived. The
// part-update guard checks the move.
onRecordCreate(
  (e) => {
    const saved = e.next();
    const record = e.record;
    const pointer = record.collection().name === "catalog_part_details" ? "current_details" : "current_revision";
    const part = e.app.findRecordById("catalog_parts", record.getString("part"));
    if (part.getString(pointer) !== record.id) {
      part.set(pointer, record.id);
      part.set(pointer + "_number", record.getInt("number"));
      e.app.save(part);
    }
    return saved;
  },
  "catalog_part_details",
  "catalog_part_revisions",
);

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
  const find = (collection, id) => lib.recordById(e.app, collection, id);
  const revision = find("catalog_part_revisions", entry.getString("part_revision"));
  if (!revision || revision.getString("part") !== part || revision.getInt("number") !== number) {
    throw new Error(`an archive entry must name revision ${number} of its own part ${part}`);
  }
  const change = find("catalog_changes", entry.getString("change"));
  // crates/catalog/src/replica.rs relies on this rule: it asks for archive
  // entries only in archive and unarchive Changes. A test there checks it.
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
  const find = (collection, id) => lib.recordById(e.app, collection, id);
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

// A category revision must hold a category with that category's code. A
// category's ID is derived from its code.
onRecordCreate((e) => {
  const lib = require(`${__hooks}/catalog_lib.js`);
  const revision = e.record;
  const category = revision.getString("category");
  const number = revision.getInt("number");
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
// of kind edit or create, copy its seq, actor and actor name, have the ID
// derived from the Change and its clue, and that Change must save the
// part's details in the match's category: changed from another category
// (an edit), or the new part's first details (a create, kicad-parts-9693).
// So an import, or a supplier category on its own, never makes one.
onRecordCreate((e) => {
  const lib = require(`${__hooks}/catalog_lib.js`);
  const match = e.record;
  const find = (collection, id) => lib.recordById(e.app, collection, id);
  const change = find("catalog_changes", match.getString("change"));
  if (
    !change ||
    (change.getString("kind") !== lib.CHANGE_KINDS.edit && change.getString("kind") !== lib.CHANGE_KINDS.create) ||
    change.getInt("seq") !== match.getInt("seq") ||
    change.getString("actor") !== match.getString("actor") ||
    change.getString("actor_name") !== match.getString("actor_name")
  ) {
    throw new Error("a remembered match must belong to an edit or a new part and copy its seq and who made it");
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
  // A new part's first details have no predecessor; an edit's must have
  // been in another category.
  const confirmed =
    change.getString("kind") === lib.CHANGE_KINDS.create
      ? details && details.getInt("number") === 1 && details.getString("predecessor") === ""
      : previous && categoryOf(previous) !== category;
  if (
    typeof category !== "string" ||
    lib.deriveId("category", category) !== match.getString("category") ||
    !confirmed
  ) {
    throw new Error(
      "a remembered match must come with an edit that moves its part to the match's category, or with a new part in it",
    );
  }
  return e.next();
}, "catalog_category_matches");

// ---- Every new record names its own transaction's Change -------------------
//
// These two run after each collection's own checks above, so those give
// their more specific messages first. catalog_lib.js explains the rules.

// A transaction saves at most one Change, numbered right after the newest.
onRecordCreate((e) => {
  const lib = require(`${__hooks}/catalog_lib.js`);
  lib.requireNextChange(e.app, e.record);
  return e.next();
}, "catalog_changes");

// Every new record that names a Change names the one saved in the same
// transaction. This hook runs for every collection and finds the fields
// that name a Change in the schema, so no collection can be missed.
onRecordCreate((e) => {
  const lib = require(`${__hooks}/catalog_lib.js`);
  lib.requireOwnChange(e.app, e.record);
  return e.next();
});

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
